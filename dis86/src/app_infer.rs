use std::collections::HashSet;

use crate::asm::decode::Decoder;
use crate::asm::instr::{self, Instr as RawInstr, Operand, Reg};
use crate::asm::instr::Opcode::*;
use crate::binary::Binary;
use crate::config::Config;
use crate::decompile::ir::IR;
use crate::decompile::{abi, ir_build, opt, sym};
use crate::segoff::SegOff;
use crate::spec::Spec;
use crate::types::Type;

struct BuiltFunc {
  name: String,
  start: SegOff,
  end: SegOff,
  annotated_ret: Option<Type>,
  ir: IR,
  raw: Vec<RawInstr>,
}

struct SpecSeed {
  name: String,
  func_name: Option<String>,
  start: SegOff,
  end: SegOff,
}

pub fn run(cfg: &Config, binary: &Binary, specs: Vec<Spec<'_>>) -> i32 {
  let seeds: Vec<SpecSeed> = specs.into_iter().map(|spec| SpecSeed {
    name: spec.name,
    func_name: spec.func.map(|f| f.name.clone()),
    start: spec.start,
    end: spec.end,
  }).collect();

  // Regargs propagate caller-ward: a caller pins a callee's declared register
  // inputs, so rebuilding with the inferred declarations reaches a fixpoint.
  let mut working = cfg.clone();
  let mut built = vec![];
  for _ in 0..8 {
    let specs: Vec<Spec<'_>> = seeds.iter().map(|seed| match &seed.func_name {
      Some(name) => Spec::from_config_name(&working, name).unwrap_or_else(|| Spec {
        func: None,
        name: seed.name.clone(),
        start: seed.start,
        end: seed.end,
      }),
      None => Spec {
        func: None,
        name: seed.name.clone(),
        start: seed.start,
        end: seed.end,
      },
    }).collect();
    built = match build_all(&working, binary, specs) {
      Ok(built) => built,
      Err((name, err)) => {
        eprintln!("Error: {}: {}", name, err);
        return 1;
      }
    };

    let mut changed = false;
    for f in &working.funcs {
      let Some(bf) = built.iter().find(|bf| bf.name == f.name) else { continue; };
      let regs = abi::infer_register_inputs(&bf.ir);
      let new = if regs.is_empty() { None } else { Some(regs) };
      if f.regargs != new {
        changed = true;
        break;
      }
    }
    if !changed { break; }
    for bf in &built {
      let regs = abi::infer_register_inputs(&bf.ir);
      let new = if regs.is_empty() { None } else { Some(regs) };
      if let Some(f) = working.funcs.iter_mut().find(|f| f.name == bf.name) {
        f.regargs = new;
      }
    }
  }

  let mut dx_reads = HashSet::new();
  for caller in &built {
    raw_dx_reads(cfg, caller, &mut dx_reads);
  }

  let args = infer_args(cfg, &built);
  println!("name\tstart\tend\targs\tdont_pop_args\tret\tregargs\tnote");
  for f in built {
    let ret = if !abi::has_reachable_ret(&f.ir)
      || matches!(f.annotated_ret, Some(Type::Void)) {
      "void"
    } else if dx_reads.contains(&f.name) {
      "u32"
    } else {
      "u16"
    };
    let regs: Vec<&str> = abi::infer_register_inputs(&f.ir)
      .iter().map(|reg| reg.info().name).collect();
    let regs = if regs.is_empty() { "-".to_string() } else { regs.join(",") };
    let a = args.get(&f.name);
    let nargs = a.and_then(|a| a.args).map(|n| n.to_string()).unwrap_or_else(|| "?".to_string());
    let dont_pop = a.map(|a| a.dont_pop_args).unwrap_or(false);
    let note = a.map(|a| a.note.as_str()).unwrap_or("no call-site evidence");
    println!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
             f.name, f.start, f.end, nargs, dont_pop, ret, regs, note);
  }
  0
}

struct ArgsInfo {
  args: Option<u16>,
  dont_pop_args: bool,
  note: String,
}

fn infer_args(cfg: &Config, built: &[BuiltFunc]) -> std::collections::HashMap<String, ArgsInfo> {
  let mut cleanups: std::collections::HashMap<String, Vec<u16>> = std::collections::HashMap::new();
  for caller in built {
    for (i, ins) in caller.raw.iter().enumerate() {
      if !matches!(ins.opcode, OP_CALL) { continue; }
      let Some(target) = direct_call_target(ins) else { continue; };
      let Some(callee) = cfg.func_lookup(target) else { continue; };
      let Some(bytes) = cleanup_bytes(&caller.raw, i + 1) else { continue; };
      if bytes % 2 != 0 { continue; }
      cleanups.entry(callee.name.clone()).or_default().push(bytes / 2);
    }
  }

  let mut out = std::collections::HashMap::new();
  for f in built {
    if let Some(nargs) = ret_imm_args(&f.raw) {
      out.insert(f.name.clone(), ArgsInfo {
        args: Some(nargs),
        dont_pop_args: true,
        note: format!("ret {} (callee pops)", nargs * 2),
      });
      continue;
    }
    if let Some(values) = cleanups.get(&f.name) {
      let mut uniq = values.clone();
      uniq.sort(); uniq.dedup();
      if uniq.len() == 1 {
        out.insert(f.name.clone(), ArgsInfo {
          args: Some(uniq[0]),
          dont_pop_args: false,
          note: format!("caller cleans {} byte(s)", uniq[0] * 2),
        });
      } else {
        out.insert(f.name.clone(), ArgsInfo {
          args: Some(0),
          dont_pop_args: false,
          note: format!("variadic/cleanup varies: {:?}", uniq),
        });
      }
    }
  }
  out
}

fn ret_imm_args(raw: &[RawInstr]) -> Option<u16> {
  let mut values = std::collections::BTreeSet::new();
  for ins in raw {
    if !matches!(ins.opcode, OP_RET | OP_RETF) { continue; }
    for op in &ins.operands {
      if let Operand::Imm(imm) = op {
        if imm.val % 2 == 0 { values.insert(imm.val / 2); }
      }
    }
  }
  if values.len() == 1 { values.into_iter().next() } else { None }
}

fn cleanup_bytes(raw: &[RawInstr], start: usize) -> Option<u16> {
  let mut j = start;
  while j < raw.len() {
    let ins = &raw[j];
    if ins.opcode == OP_ADD && first_is_sp(ins) {
      if let Some(Operand::Imm(imm)) = ins.operands.get(1) { return Some(imm.val); }
    }
    if matches!(ins.opcode, OP_PUSH | OP_POP | OP_CALL | OP_CALLF | OP_INT | OP_INTO
      | OP_RET | OP_RETF | OP_JMP | OP_JMPF | OP_HLT
      | OP_JA | OP_JAE | OP_JB | OP_JBE | OP_JCXZ | OP_JE | OP_JG | OP_JGE
      | OP_JL | OP_JLE | OP_JNE | OP_JNO | OP_JNP | OP_JNS | OP_JO | OP_JP
      | OP_JS | OP_LOOP | OP_LOOPE | OP_LOOPNE) {
      return None;
    }
    j += 1;
  }
  None
}

fn first_is_sp(ins: &RawInstr) -> bool {
  matches!(ins.operands.get(0), Some(Operand::Reg(instr::OperandReg(Reg::SP))))
}

fn build_all(cfg: &Config, binary: &Binary, specs: Vec<Spec<'_>>)
  -> Result<Vec<BuiltFunc>, (String, String)> {
  let mut built = vec![];
  for spec in specs {
    match build(cfg, binary, &spec) {
      Ok((ir, raw)) => built.push(BuiltFunc {
        name: spec.name,
        start: spec.start,
        end: spec.end,
        annotated_ret: spec.func.and_then(|f| f.ret.clone()),
        ir,
        raw,
      }),
      Err(err) => return Err((spec.name, err)),
    }
  }
  Ok(built)
}

fn build(cfg: &Config, binary: &Binary, spec: &Spec<'_>) -> Result<(IR, Vec<RawInstr>), String> {
  let start = spec.start;
  let end = spec.end;
  let mut instrs = vec![];
  let mut decoder = Decoder::new(binary.region_iter(start, end));
  loop {
    match decoder.try_next() {
      Ok(Some((instr, _raw))) => instrs.push(instr),
      Ok(None) => break,
      Err(err) => return Err(format!("decode failed at {}: {}", start, err)),
    }
  }

  let overlay = spec.start.is_overlay_addr();
  let mut ir = ir_build::build_from_instrs(&instrs, cfg, spec, binary, overlay, false);
  opt::optimize(&mut ir);
  sym::symbolize(&mut ir, cfg);
  opt::forward_store_to_load(&mut ir);
  opt::optimize(&mut ir);
  opt::mem_symbol_to_ref(&mut ir);
  opt::optimize(&mut ir);
  Ok((ir, instrs))
}

fn raw_dx_reads(cfg: &Config, caller: &BuiltFunc, out: &mut HashSet<String>) {
  for (i, ins) in caller.raw.iter().enumerate() {
    if !matches!(ins.opcode, OP_CALL) { continue; }
    let Some(target) = direct_call_target(ins) else { continue; };
    let Some(callee) = cfg.func_lookup(target) else { continue; };
    let mut j = i + 1;
    while j < caller.raw.len() {
      match dx_effect(&caller.raw[j]) {
        DxEffect::Read => {
          out.insert(callee.name.clone());
          break;
        }
        DxEffect::Write | DxEffect::Clobber => break,
        DxEffect::None => j += 1,
      }
    }
  }
}

fn direct_call_target(ins: &RawInstr) -> Option<crate::segoff::SegOff> {
  for op in &ins.operands {
    if let Operand::Rel(rel) = op { return Some(ins.rel_addr(rel)); }
  }
  None
}

#[derive(Debug, PartialEq, Eq)]
enum DxEffect { None, Read, Write, Clobber }

/// Whether a raw instruction reads or writes the full `DX` register. The scan
/// stops at the first write (or clobbering call) after a call; a read before
/// that is the caller consuming the high half.
fn dx_effect(ins: &RawInstr) -> DxEffect {
  match ins.opcode {
    OP_CALL | OP_CALLF | OP_INT | OP_INTO | OP_IRET | OP_RET | OP_RETF
      | OP_JMP | OP_JMPF | OP_HLT => DxEffect::Clobber,
    OP_JA | OP_JAE | OP_JB | OP_JBE | OP_JCXZ | OP_JE | OP_JG | OP_JGE
      | OP_JL | OP_JLE | OP_JNE | OP_JNO | OP_JNP | OP_JNS | OP_JO | OP_JP
      | OP_JS | OP_LOOP | OP_LOOPE | OP_LOOPNE => DxEffect::Clobber,
    OP_CWD => DxEffect::Write,
    OP_MUL | OP_IMUL | OP_IMUL_TRUNC | OP_DIV | OP_IDIV => {
      if any_reads_dx(ins) { DxEffect::Read } else { DxEffect::Write }
    }
    OP_POP => {
      if first_is_dx(ins) { DxEffect::Write } else { DxEffect::None }
    }
    OP_PUSH => {
      if any_reads_dx(ins) { DxEffect::Read } else { DxEffect::None }
    }
    OP_INC | OP_DEC | OP_NEG | OP_NOT | OP_RCL | OP_RCR
      | OP_ROL | OP_ROR | OP_SHL | OP_SHR | OP_SAR => {
      if any_reads_dx(ins) { DxEffect::Read } else { DxEffect::None }
    }
    OP_MOV | OP_LEA | OP_LDS | OP_LES => {
      if first_is_dx(ins) {
        if source_reads_dx(ins) { DxEffect::Read } else { DxEffect::Write }
      } else if source_reads_dx(ins) {
        DxEffect::Read
      } else {
        DxEffect::None
      }
    }
    OP_XCHG => {
      if any_reads_dx(ins) { DxEffect::Read } else { DxEffect::None }
    }
    OP_ADD | OP_ADC | OP_SUB | OP_SBB | OP_AND | OP_OR | OP_XOR => {
      if first_is_dx(ins) {
        if read_modify_zeroing(ins) { DxEffect::Write } else { DxEffect::Read }
      } else if source_reads_dx(ins) {
        DxEffect::Read
      } else {
        DxEffect::None
      }
    }
    OP_CMP | OP_TEST => {
      if any_reads_dx(ins) { DxEffect::Read } else { DxEffect::None }
    }
    _ => {
      if source_reads_dx(ins) { DxEffect::Read } else { DxEffect::None }
    }
  }
}

fn first_is_dx(ins: &RawInstr) -> bool {
  matches!(ins.operands.get(0), Some(Operand::Reg(instr::OperandReg(Reg::DX))))
}

fn source_reads_dx(ins: &RawInstr) -> bool {
  ins.operands.as_slice().iter().skip(1).any(|op| operand_reads_dx(*op))
}

fn any_reads_dx(ins: &RawInstr) -> bool {
  ins.operands.as_slice().iter().any(|op| operand_reads_dx(*op))
}

fn operand_reads_dx(op: Operand) -> bool {
  match op {
    Operand::Reg(instr::OperandReg(Reg::DX)) | Operand::Reg(instr::OperandReg(Reg::DH)) => true,
    Operand::Mem(mem) => mem.reg1 == Some(Reg::DX) || mem.reg2 == Some(Reg::DX),
    _ => false,
  }
}

fn read_modify_zeroing(ins: &RawInstr) -> bool {
  let mut ops = ins.operands.as_slice().iter();
  let Some(Operand::Reg(instr::OperandReg(Reg::DX))) = ops.next() else { return false; };
  matches!(ops.next(), Some(Operand::Reg(instr::OperandReg(Reg::DX))))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn decode(code: &[u8]) -> Vec<RawInstr> {
    let binary = crate::binary::Binary::from_raw(code, None);
    let mut decoder = Decoder::new(binary.region_iter(
      crate::segoff::SegOff { seg: crate::segoff::Seg::Normal(0), off: crate::segoff::Off(0) },
      crate::segoff::SegOff { seg: crate::segoff::Seg::Normal(0), off: crate::segoff::Off(code.len() as u16) }));
    let mut out = vec![];
    while let Some((ins, _raw)) = decoder.try_next().expect("decode") { out.push(ins); }
    out
  }

  #[test]
  fn ret_imm_is_callee_cleaned_args() {
    assert_eq!(ret_imm_args(&decode(&[0xC2, 0x04, 0x00])), Some(2));
    assert_eq!(ret_imm_args(&decode(&[0xC3])), None);
  }

  #[test]
  fn mixed_ret_immediates_are_not_trusted() {
    let code = [0xC2, 0x04, 0x00, 0xC2, 0x06, 0x00];
    assert_eq!(ret_imm_args(&decode(&code)), None);
  }

  #[test]
  fn caller_stack_cleanup_is_measured() {
    let raw = decode(&[0xE8, 0x00, 0x00, 0x83, 0xC4, 0x04]);
    assert_eq!(cleanup_bytes(&raw, 1), Some(4));
  }

  #[test]
  fn dx_effect_distinguishes_reads_writes_and_zeroing() {
    assert_eq!(dx_effect(&decode(&[0x89, 0xD0])[0]), DxEffect::Read);  // mov ax,dx
    assert_eq!(dx_effect(&decode(&[0x89, 0xD8])[0]), DxEffect::None);  // mov ax,bx
    assert_eq!(dx_effect(&decode(&[0xBA, 0x04, 0x00])[0]), DxEffect::Write); // mov dx,4
    assert_eq!(dx_effect(&decode(&[0x5A])[0]), DxEffect::Write);       // pop dx
    assert_eq!(dx_effect(&decode(&[0x52])[0]), DxEffect::Read);        // push dx
    assert_eq!(dx_effect(&decode(&[0x31, 0xD2])[0]), DxEffect::Write); // xor dx,dx
  }
}
