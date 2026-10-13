#!/usr/bin/env bash
# TEMPORARY: arena research harness. Runs the published repros for issues
# #64/#65/#66/#67/#69 against this branch's HEAD and captures every output
# into .arena-verify/out/ so it can be reviewed outside the runner.
set -uo pipefail

D=./dis86/target/debug/dis86
OUT=.arena-verify/out
mkdir -p "$OUT" /tmp/verify

echo "HEAD: $(git rev-parse HEAD)"
git rev-parse HEAD > "$OUT/head.txt"

section() {
  echo
  echo "==================== $* ===================="
}

# ---------------------------------------------------------------- issue #66
{
  section "#66 default --emit-code (expect bare INT(0x21) with no register setup)"
  $D --config /tmp/verify/repro66.bsl --binary-exe /tmp/verify/synth_66.exe --name main --emit-code /tmp/verify/66.c
  echo "exit=$?"
  cat /tmp/verify/66.c

  section "#66 --emit-ir-final (shows what happens to the setup)"
  $D --config /tmp/verify/repro66.bsl --binary-exe /tmp/verify/synth_66.exe --name main --emit-ir-final /tmp/verify/66.ir
  echo "exit=$?"
  cat /tmp/verify/66.ir

  section "#66 --build-pin-all --emit-code (existing CLI workaround?)"
  $D --config /tmp/verify/repro66.bsl --binary-exe /tmp/verify/synth_66.exe --name main --emit-code /tmp/verify/66_pin.c --build-pin-all
  echo "exit=$?"
  cat /tmp/verify/66_pin.c
} > "$OUT/66.txt" 2>&1

# ---------------------------------------------------------------- issue #67
{
  section "#67 decompile A (range [0000:0000, 0000:0009); 'jmp 0x000a' targets B)"
  $D --config /tmp/verify/repro67.bsl --binary-exe /tmp/verify/synth_67.exe --name A --emit-code /tmp/verify/67a.c 2>/tmp/verify/67a.err
  echo "exit=$?"
  echo "--- stdout: emitted A ---"
  cat /tmp/verify/67a.c
  echo "--- stderr (any diagnostic?) ---"
  cat /tmp/verify/67a.err
  echo "--- stderr byte count ---"
  wc -c < /tmp/verify/67a.err

  section "#67 decompile B for comparison"
  $D --config /tmp/verify/repro67.bsl --binary-exe /tmp/verify/synth_67.exe --name B --emit-code /tmp/verify/67b.c
  echo "exit=$?"
  cat /tmp/verify/67b.c

  section "#67 emit-ctrlflow for A"
  $D --config /tmp/verify/repro67.bsl --binary-exe /tmp/verify/synth_67.exe --name A --emit-ctrlflow /tmp/verify/67a.cf
  echo "exit=$?"
  cat /tmp/verify/67a.cf
} > "$OUT/67.txt" 2>&1

# ---------------------------------------------------------------- issue #65
{
  section "#65 --analyze suggestion for the 75-byte synth"
  $D --config /tmp/verify/repro65.bsl --binary-exe /tmp/verify/synth_65.exe --analyze
  echo "analyze exit=$?"

  section "#65 --codeseg-name with the analyze-suggested range verbatim"
  $D --config /tmp/verify/repro65.bsl --binary-exe /tmp/verify/synth_65.exe --codeseg-name CODE --emit-code /tmp/verify/65.c
  echo "exit=$?"

  section "#65 --codeseg-name with the corrected start (0000:0006)"
  $D --config /tmp/verify/repro65_fixed.bsl --binary-exe /tmp/verify/synth_65.exe --codeseg-name CODE --emit-code /tmp/verify/65f.c
  echo "exit=$?"
  echo "--- emitted (corrected) ---"
  cat /tmp/verify/65f.c
} > "$OUT/65.txt" 2>&1

# ---------------------------------------------------------------- issue #69
{
  section "#69 source under test: adc ax,bx; sbb ax,ax; rol al,1; rep movsb; stc; ret"
  $D --config /tmp/verify/repro69.bsl --binary-exe /tmp/verify/synth_69.exe --name main --emit-code /tmp/verify/69.c
  echo "exit=$?"
  cat /tmp/verify/69.c

  section "#69 UNIMPL/UNIMPL_FLAGS marker counts in the emitted C"
  grep -c "UNIMPL(" /tmp/verify/69.c || true
  grep -c "UNIMPL_FLAGS(" /tmp/verify/69.c || true

  section "#69 lowering sites at this HEAD"
  grep -n "ins.rep.is_some()\|OP_SBB =>\|OP_RCL |\|OP_ADC =>\|OP_INS =>\|OP_OUTS =>\|OP_STD =>\|OP_STC |\|OP_IN =>\|OP_OUT =>\|Unimpl," dis86/src/decompile/ir_build.rs

  section "#69 hydra runtime semantics of the markers"
  sed -n '14,33p' hydra/src/header.h
} > "$OUT/69.txt" 2>&1

# ---------------------------------------------------------------- issue #64
{
  section "#64 unit repro: deadblock_elimination + stack_ptr_accumulation"
  cargo test --manifest-path dis86/Cargo.toml --locked --lib issue64_stack_ptr_accumulation_panics_on_dangling_ref -- --nocapture --test-threads=1
  echo "test exit=$?"

  section "#64 unit repro: full optimize()"
  cargo test --manifest-path dis86/Cargo.toml --locked --lib issue64_full_optimize_panics_on_dangling_ref -- --nocapture --test-threads=1
  echo "test exit=$?"

  section "#64 block()/instr() unwrap sites"
  sed -n '76,80p' dis86/src/decompile/ir/methods.rs
  grep -n "block_remove" -A 4 dis86/src/decompile/ir/methods.rs | head -12
} > "$OUT/64.txt" 2>&1

# ---------------------------------------------------------------- misc
{
  section "#68 mzfile invocation (path vs subcommand)"
  $D --help 2>&1 | head -5
  ./dis86/target/debug/mzfile /tmp/verify/synth_65.exe
  echo "mzfile(path) exit=$?"
  ./dis86/target/debug/mzfile info /tmp/verify/synth_65.exe
  echo "mzfile(info) exit=$?"

  section "#68 confgen loader with a .src extension"
  printf 'CodeSegments=[]\nFunctions=[]\nStructures=[]\nDataSection=[]\nTextSection=[]\nCallstack=[]\n' > /tmp/verify/annotations.src
  cp /tmp/verify/annotations.src /tmp/verify/annotations.py
  PYTHONPATH=confgen python3 confgen/generate.py /tmp/verify/annotations.src --dis86-conf
  echo "src exit=$?"
  PYTHONPATH=confgen python3 confgen/generate.py /tmp/verify/annotations.py --dis86-conf
  echo "py exit=$?"
} > "$OUT/68.txt" 2>&1

echo "evidence written to $OUT"
ls -la "$OUT"
