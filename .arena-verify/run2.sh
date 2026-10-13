#!/usr/bin/env bash
# TEMPORARY: second-round arena research harness (register writeback idiom,
# flags after INT). Evidence goes to .arena-verify/out/.
set -uo pipefail

D=./dis86/target/debug/dis86
OUT=.arena-verify/out
mkdir -p "$OUT" /tmp/verify2

section() {
  echo
  echo "==================== $* ===================="
}

{
  section "A: caller sets AX, calls callee annotated 'regargs ax' (register writeback idiom)"
  $D --config /tmp/verify2/cfga.bsl --binary-exe /tmp/verify2/regargs.exe --name caller --emit-code /tmp/verify2/caller.c
  echo "exit=$?"
  cat /tmp/verify2/caller.c

  section "A2: same, but Hydra-flavored codegen"
  $D --config /tmp/verify2/cfga.bsl --binary-exe /tmp/verify2/regargs.exe --name caller --emit-code /tmp/verify2/caller_h.c --codegen-hydra
  echo "exit=$?"
  cat /tmp/verify2/caller_h.c

  section "B: 'mov bx,5; ret' - does a callee-saved register write reach the guest?"
  $D --config /tmp/verify2/cfgb.bsl --binary-exe /tmp/verify2/movbx.exe --name main --emit-code /tmp/verify2/movbx.c
  echo "exit=$?"
  cat /tmp/verify2/movbx.c

  section "B2: IR-final for the same function"
  $D --config /tmp/verify2/cfgb.bsl --binary-exe /tmp/verify2/movbx.exe --name main --emit-ir-final /tmp/verify2/movbx.ir
  echo "exit=$?"
  cat /tmp/verify2/movbx.ir

  section "C: 'sub ax,ax; int 21h; jz +1' - flags after INT"
  $D --config /tmp/verify2/cfgc.bsl --binary-exe /tmp/verify2/flags.exe --name main --emit-code /tmp/verify2/flags.c
  echo "exit=$?"
  cat /tmp/verify2/flags.c

  section "C2: IR-final for the same function"
  $D --config /tmp/verify2/cfgc.bsl --binary-exe /tmp/verify2/flags.exe --name main --emit-ir-final /tmp/verify2/flags.ir
  echo "exit=$?"
  cat /tmp/verify2/flags.ir
} > "$OUT/round2.txt" 2>&1

echo "evidence written to $OUT/round2.txt"
