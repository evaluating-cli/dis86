#!/usr/bin/env python3
"""Insert the #64 minimal-IR fixture into a checkout of main's opt.rs tests.

Usage: insert_issue64_test.py <path-to-opt.rs>

The fixture is the one posted in issue #64's verification comment: a live
`Ref::Instr` operand pointing into a block with zero preds.  On pre-fix main
`stack_ptr_accumulation` panics in `IR::block`; on current main the
`deadblock_elimination` repair must keep the run panic-free.
"""
import pathlib
import sys

SNIPPET = r'''
  // ---- arena verification (temporary; inserted by .arena-verify2) ----
  fn issue64_fixture() -> (IR, BlockRef, BlockRef) {
    let mut ir = IR::new(Rc::new(TypeDatabase::new()));
    let entry = ir.add_block("entry");
    let dead = ir.add_block("dead"); // never targeted: zero preds
    let seven = ir.const_new(7);
    let dead_val = append_typed(&mut ir, dead, Type::U16, Opcode::Add, vec![seven, seven]);
    let k = ir.const_new(4);
    ir.block_instr_append(entry, Instr {
      typ: Type::U16,
      compare_width: None,
      attrs: Attribute::STACK_PTR,
      opcode: Opcode::Add,
      operands: vec![dead_val, k],
    });
    (ir, entry, dead)
  }

  #[test]
  fn issue64_verify() {
    let (mut ir, entry, dead) = issue64_fixture();
    deadblock_elimination(&mut ir);
    eprintln!("ISSUE64: dead block removed = {}", ir.blocks[dead.0].is_none());
    for r in ir.iter_instrs(entry).collect::<Vec<_>>() {
      match ir.instr(r) {
        Some(i) => eprintln!("ISSUE64: entry {:?} {:?} operands {:?}", r, i.opcode, i.operands),
        None => eprintln!("ISSUE64: entry {:?} <dangling>", r),
      }
    }
    stack_ptr_accumulation(&mut ir);
    eprintln!("ISSUE64: stack_ptr_accumulation completed (no panic)");

    let (mut ir2, _e2, _d2) = issue64_fixture();
    optimize(&mut ir2);
    eprintln!("ISSUE64: optimize() completed (no panic)");
  }
'''


def main() -> int:
    path = pathlib.Path(sys.argv[1])
    text = path.read_text()
    if "issue64_verify" in text:
        print(f"{path}: fixture already present")
        return 0
    idx = text.rstrip().rfind("}")
    if idx < 0:
        print(f"{path}: no closing brace found", file=sys.stderr)
        return 1
    path.write_text(text[:idx] + SNIPPET + text[idx:])
    print(f"{path}: inserted issue64 fixture before offset {idx}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
