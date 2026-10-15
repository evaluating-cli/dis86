// Regression tests for INT lowering (issue #66): the handler's register
// inputs must stay live through DCE, and post-INT flags must degrade to the
// loud UNIMPL_FLAGS marker instead of folding against pre-INT flags.
use dis86::asm::decode::Decoder;
use dis86::asm::instr::Instr;
use dis86::config::Config;
use dis86::decompile::ast::Function;
use dis86::decompile::control_flow::ControlFlow;
use dis86::decompile::gen::{self, Flavor};
use dis86::decompile::{ir, ir_build, opt, sym};
use dis86::segoff::{Off, Seg, SegOff};
use dis86::spec::Spec;
use dis86::types::TypeDatabase;

fn empty_config(types: std::rc::Rc<TypeDatabase>) -> Config {
  Config {
    types,
    structs: vec![],
    code_segs: vec![],
    funcs: vec![],
    indirects: vec![],
    globals: vec![],
    text_section: vec![],
  }
}

// Runs the pipeline app.rs uses, up to and including C generation.
fn decompile_raw(code: &[u8]) -> String {
  let types = std::rc::Rc::new(TypeDatabase::new());
  let cfg = empty_config(types);
  let binary = dis86::binary::Binary::from_raw(code, None);
  let spec = Spec::from_start_and_end(
    Some(SegOff { seg: Seg::Normal(0), off: Off(0) }),
    Some(SegOff { seg: Seg::Normal(0), off: Off(code.len() as u16) }),
  );

  let mut decoder = Decoder::new(binary.region_iter(spec.start, spec.end));
  let mut instr_list: Vec<Instr> = vec![];
  while let Some((ins, _raw)) = decoder.try_next().expect("decode") {
    instr_list.push(ins);
  }

  let mut ir = ir_build::build_from_instrs(&instr_list, &cfg, &spec, &binary, false, false);
  opt::optimize(&mut ir);
  sym::symbolize(&mut ir, &cfg);
  opt::forward_store_to_load(&mut ir);
  opt::optimize(&mut ir);
  opt::mem_symbol_to_ref(&mut ir);
  opt::optimize(&mut ir);
  dis86::decompile::fuse::fuse_mem(&mut ir);
  opt::optimize(&mut ir);
  ir::fin::finalize(&mut ir);

  let cf = ControlFlow::from_ir(&ir);
  let ast = Function::from_ir(&cfg, "main", None, &ir, &cf).expect("ast");
  gen::generate(&ast, Flavor::Standard).expect("codegen")
}

// mov ah,9; mov dx,0xb; int 21h; mov ax,0x4c00; int 21h; ret
// (the setup for the first INT must not be dead-code-eliminated).
#[test]
fn int_register_setup_survives_dce() {
  let code = [
    0xB4, 0x09,       // mov ah,9
    0xBA, 0x0B, 0x00, // mov dx,0xb
    0xCD, 0x21,       // int 21h
    0xB8, 0x00, 0x4C, // mov ax,0x4c00
    0xCD, 0x21,       // int 21h
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(source.contains("INT(0x21)"), "interrupt must be emitted:\n{source}");
  assert!(
    source.contains("DX = 11"),
    "the `mov dx,0xb` setup for INT 21h/AH=09 must survive DCE:\n{source}"
  );
  assert!(
    source.contains("AX = 0x4c00"),
    "the `mov ax,0x4c00` setup for the terminating INT must survive DCE:\n{source}"
  );
  // The AH=9 setup recomposes AX from the unknown low byte; it must reach
  // the first INT rather than being dropped.
  assert!(
    source.contains("9 << 8") || source.contains("9<<8"),
    "the `mov ah,9` setup must be materialized as a guest AX write:\n{source}"
  );
}

// mov cx,0x1234; int 21h; mov ax,cx; ret
// The handler may clobber any GPR (e.g. AH=2Ch returns time in CX/DX), so a
// post-INT read must resolve to the live register, not stale pre-INT setup.
#[test]
fn post_int_register_reads_see_live_values() {
  let code = [
    0xB9, 0x34, 0x12, // mov cx,0x1234
    0xCD, 0x21,       // int 21h
    0x89, 0xC8,       // mov ax,cx
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(source.contains("INT(0x21)"), "interrupt must be emitted:\n{source}");
  assert!(
    source.contains("CX = 0x1234"),
    "the pre-INT CX setup must reach the handler as a guest write:\n{source}"
  );
  assert!(
    source.contains("MAKE_32(DX, CX)"),
    "the post-INT `mov ax,cx` must read the live CX, not the stale 0x1234:\n{source}"
  );
}
// mov bx,ax; mov ah,6; int 21h; mov ax,bx; ret (issue #78)
// BX must capture AX before the AH=6 recompose overwrites it: the guest
// writes must read `BX = AX` (entry) ahead of `AX = (6 << 8) | ...`, so the
// post-INT restore returns the saved value, not 0x06xx.
#[test]
fn int_materialization_preserves_register_copy_order() {
  let code = [
    0x89, 0xC3,       // mov bx,ax
    0xB4, 0x06,       // mov ah,6
    0xCD, 0x21,       // int 21h
    0x89, 0xD8,       // mov ax,bx
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(source.contains("INT(0x21)"), "interrupt must be emitted:\n{source}");
  let bx = source.find("BX = AX").expect("copy BX = AX(entry) must be emitted:\n{source}");
  let ax_setup = source.find("6 << 8").expect("AH=6 recompose must be emitted:\n{source}");
  assert!(
    bx < ax_setup,
    "BX must capture AX before the AH=6 overwrite (BX={bx}, AX={ax_setup}):\n{source}"
  );
}

// mov cl,al; mov ax,0x1234; int 21h; ret (issue #78, fragment aliasing)
// AL aliases AX storage, so the CX copy (which reads entry AL) must be
// materialized before the AX overwrite, not after.
#[test]
fn int_materialization_orders_byte_fragment_copies_first() {
  let code = [
    0x88, 0xC1,       // mov cl,al
    0xB8, 0x34, 0x12, // mov ax,0x1234
    0xCD, 0x21,       // int 21h
    0xC3,             // ret
  ];
  let source = decompile_raw(&code);

  assert!(source.contains("INT(0x21)"), "interrupt must be emitted:\n{source}");
  let cx = source.find("CX =").expect("CX copy must be emitted:\n{source}");
  let ax_setup = source.find("AX = 0x1234").expect("AX setup must be emitted:\n{source}");
  assert!(
    cx < ax_setup,
    "CX must capture entry AL before the AX overwrite (CX={cx}, AX={ax_setup}):\n{source}"
  );
}
// sub ax,ax; int 21h; jz; nop; ret
// The flags after INT belong to the handler, so the branch must not fold
// against the pre-INT `sub`.
#[test]
fn flags_after_int_do_not_fold_against_pre_int_producer() {
  let code = [
    0x2B, 0xC0, // sub ax,ax
    0xCD, 0x21, // int 21h
    0x74, 0x01, // jz +1
    0x90,       // nop
    0xC3,       // ret
  ];
  let source = decompile_raw(&code);

  assert!(source.contains("INT(0x21)"), "interrupt must be emitted:\n{source}");
  assert!(
    !source.contains("AX == AX"),
    "post-INT flags must not fold to the pre-INT `sub ax,ax` result:\n{source}"
  );
  assert!(
    source.contains("UNIMPL_FLAGS"),
    "post-INT flags must degrade to the loud UNIMPL_FLAGS marker:\n{source}"
  );
}
