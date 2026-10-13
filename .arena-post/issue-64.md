<!-- arena-triage-verify -->
Verified on current `main` (`54d2d41`, after #70) — confirmed, and it reproduces **without** the private binary: a minimal IR with a live `Ref::Instr` operand into a block that has zero preds panics at exactly the reported site, both through the reported pass sequence and through the full `optimize()`.

## Repro (no `<target.exe>` needed)

Added to the existing `mod tests` in `dis86/src/decompile/opt.rs` (reuses its helpers — `append_typed`, `Rc`, `TypeDatabase`):

```rust
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
fn issue64_stack_ptr_accumulation_panics_on_dangling_ref() {
  let (mut ir, _entry, dead) = issue64_fixture();
  deadblock_elimination(&mut ir);
  assert!(ir.blocks[dead.0].is_none(), "dead block must have been removed");
  stack_ptr_accumulation(&mut ir); // panics in IR::block
}
```

Observed:

```text
test decompile::opt::tests::issue64_stack_ptr_accumulation_panics_on_dangling_ref ...
ISSUE64: dead block removed; operand Ref::Instr(BlockRef(1), 0) is now dangling
thread '...' panicked at src/decompile/ir/methods.rs:78:36:
called `Option::unwrap()` on a `None` value
```

A second variant that calls `optimize()` on the same fixture panics at the same site.

Confirmations of the report's analysis:

- `deadblock_elimination` (`opt.rs:450-481`) repairs only the exit blocks' `preds` and Phi operands before `block_remove`; no other operand form is swept.
- `IR::block` is still a bare `.unwrap()` (`ir/methods.rs:77-79`) and `IR::instr` dereferences through it (`methods.rs:193-199`).
- `block_remove`'s "Caller is responsible for ensuring there are no references…" comment (`methods.rs:85-89`) is exactly the contract being violated.

## Triage notes

1. **Symptom shape**: `app::run` wraps each spec in `catch_unwind` (`app.rs:263-286`), so the failure is contained — the panic text still appears on stderr, but the run then prints `Error: Panicked while decompiling <fn>; continuing with the rest.`, exits 1, and the rest of a `--codeseg-name` batch still decompiles. So this is "a valid range yields no code for one function" rather than a mid-batch crash; worth reflecting in the fix's risk framing.
2. **Provenance**: the builder pushes `preds` for every edge it appends (`append_jmp`/`append_jne`/`append_jmptbl`, `ir_build.rs:238-268`), so a *live operand into a zero-pred block* is an invariant violation introduced later (my fixture constructs it directly to keep the repro minimal). The report's third suggestion — a debug assertion/diagnostic at `block_remove` — is the cheapest way to localize the introduction point on the real binary, and it names the referencing instruction(s) at the moment they matter.
3. **Fix order I'd suggest**: (a) sweep live IR on removal and repair/drop operands targeting the removed block (`Ref::Instr` and `Ref::Block`, in Phis and branches) via a small `for_each_operand_mut`-style helper so future operand kinds are covered by construction; (b) keep `block()` panicking but name the erased `BlockRef` in the message. Making `block()`/`instr()` tolerant everywhere (the report's option 2) works but trades a panic for silent miscompiles unless every call site is audited.

---
_Posted from CI on behalf of @evaluating-cli (this session's GitHub token can push code but not comment); verification evidence and repro scripts are archived on branch `arena/737d7972-dis86`._
