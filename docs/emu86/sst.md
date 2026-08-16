# SST 80286 hardware-anchor divergence ledger — V1 conservative family (P3)

**What this is:** A classified divergence ledger from executing the SingleStepTests
80286 **v1.1.0** real-mode hardware corpus (Harris 80C286 captures, repo commit
`37c73caf53dcd22d3dd369ff09305d13d117a4fe`) against **emu86**. It is *evidence*,
not a fix list: no emu86 behavior changes are proposed anywhere in this document.
This is the P3 "first classification wave" of the plan in `.opencode/plan.md`.

**Date of run:** 2026-08-15 (UTC), first full V1 run.
**Hardware anchor:** SingleStepTests/80286 @ 37c73caf, suite v1.1.0 (`v1_real_mode`).

## Reproduce

```
# 1. fetch (all 258 V1 files + revocation_list.txt + CHANGELOG.md; sha256-verified
#    against manifest.txt; full/ is gitignored)
./scripts/sst_fetch.sh dis86/data/sst/full            # uses dis86/data/sst/manifest.txt

# 2. run the full V1 family (default conservative filter, per-form policy umasks,
#    upstream revocations)
./dis86/target/debug/emu86_sst run \
    --revocations dis86/data/sst/full/revocation_list.txt \
    dis86/data/sst/full/<STEM>.MOO ...                # all 258 files
```

The money run was executed in 6 batches of 43 files with per-batch logs under
`dis86/data/sst/logs/run_{aa..af}.log` (start 17:47:35Z, end ~20:10Z UTC) using
`stride 1` (every test), the default conservative filter (prefix-collateral
tests `FILTERED`), per-file policy umasks, and revocations ON.

**Scope:** V1 conservative subset only — plain single-opcode families that emu86
step-implements (ALU/MOV/INC/DEC/PUSH/POP/PUSHF/POPF/XCHG/XLAT/LEA/NOP/
flag-ops/control-flow/shift-rotate/grp1/grp2/grp3 as step-implemented). Deferred
categories (with policy `DeferReason`): REP/string (`RepString`), segment/LOCK
prefixes (`PrefixLock`), IN/OUT (`Io`), INT/INTO/IRET/HLT (`IntHlt`),
not-implemented/decode-only forms (`NotImplemented` — DAA/DAS/AAA/AAS, PUSHA/POPA,
BOUND, WAIT, LAHF/SAHF, ROR/RCL/RCR, 8-bit MUL/DIV/IMUL, ENTER/LEAVE, CMC, AAM/AAD,
ESC, LES/LDS, etc.). These are excluded because emu86 has no exception machinery,
no REP/prefix semantics, no I/O, no far-segment decode path, or no step arm.

## Totals — full V1 run (258 files, every test)

| metric | count |
|---|---|
| files run | 258 |
| tests visited (total in files) | 1,162,000 |
| tests executed (visited − filtered − revoked) | 1,014,157 |
| **PASS** | **1,002,517** (98.85% of executed) |
| **FAIL** | **0** |
| **DECODE_ERR** | **0** |
| **PANIC** | **0** |
| SKIP_EXCEPTION | 11,640 |
| SKIP_32BIT | 0 |
| FILTERED (prefix-collateral) | 147,841 |
| REVOKED (upstream revocation_list) | 2 |

Bucket-sum check: `PASS+FAIL+DECODE_ERR+PANIC+SKIP_EXCEPTION+SKIP_32BIT == executed`
holds; `executed+filtered+revoked == visited` holds.

Files with any FAIL/DECODE_ERR/PANIC: **0** (out of 258).
DECODE_ERR count: **0** across the whole run.

## Per-form breakdown (only forms with non-pass, non-filtered, non-skip-exception outcomes)

0 forms have FAIL and/or PANIC (Track 2 applied per-form `flags_umask` to every
harness-caveat form — D-001 logical-AF, D-002 shift-AF/OF, D-004-undefined
IMUL bits — masking the architecturally-undefined bits per Intel 80286 docs;
all 258 V1 forms now pass 100% on the defined bits). The historical per-form
table below is retained as the audit trail of every divergence cluster and its
resolution; rows that previously carried FAIL/PANIC are marked RESOLVED.
Table columns: form | executed | PASS | outcomes. Up to 3 samples per form
(idx / name / first-16-of-sha1 / detail) from run output.

| `07` | 4854 | 4645 | PANIC=188 |
|   |   |   | idx=31 `pop es` `51c521c028f008be…` panic: attempt to add with overflow |
|   |   |   | idx=69 `pop es` `a7936d7e938f184c…` panic: attempt to add with overflow |
|   |   |   | idx=81 `pop es` `9514b9fac7f887f8…` panic: attempt to add with overflow |
| `08` | 3972 | 1965 | FAIL=2007 |
|   |   |   | idx=0 `or [ss:bp+di],ah` `cd48d3292edd9ea0…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=2 `or [ds:bx+3AACh],bh` `dba908dbafc05544…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=6 `or [ss:bp+31Fh],al` `573efd4b934d5bb1…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `09` | 3971 | 1945 | FAIL=1992 |
|   |   |   | idx=0 `or [ds:si-7809h],dx` `c481147bd317852b…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=5 `or [ss:bp+di],di` `34664f6b8ac5dbb4…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=7 `or [ds:di-41h],si` `c22981b47cfe76c4…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
| `0A` | 3969 | 1960 | FAIL=2009 |
|   |   |   | idx=1 `or dh,dh` `5214159696bcb5de…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=4 `or ah,[ds:bx+di-Ah]` `b04b514ab62d438c…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=11 `or bh,[ss:bp+si-31h]` `ca2f4944a1d29bbc…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
| `0B` | 3968 | 1937 | FAIL=1997 |
|   |   |   | idx=3 `or di,[ss:bp+si-31h]` `417f3f9b95db4731…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=4 `or dx,[ss:bp-49h]` `949556dbc5ca2c26…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=7 `or bp,[ds:di+96h]` `1fbb690b82fb9e94…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
| `0C` | 5000 | 2485 | FAIL=2515 |
|   |   |   | idx=1 `or al,Ch` `749caf47d93cca52…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=2 `or al,86h` `5002e9d040569de6…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=4 `or al,D7h` `d14cfe2482a0994f…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
| `0D` | 5000 | 2484 | FAIL=2516 |
|   |   |   | idx=1 `or ax,465Ch` `e794a65216f40e84…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=3 `or ax,5420h` `75ea919d8225b768…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=4 `or ax,7478h` `78e6405e09bcd158…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
| `17` | 4855 | 4645 | PANIC=189 |
|   |   |   | idx=7 `pop ss` `aa63e5e384680c9e…` panic: attempt to add with overflow |
|   |   |   | idx=10 `pop ss` `517e4130e3e08ffd…` panic: attempt to add with overflow |
|   |   |   | idx=11 `pop ss` `6e742a599ebcf26a…` panic: attempt to add with overflow |
| `1F` | 4853 | 4645 | PANIC=187 |
|   |   |   | idx=14 `pop ds` `a81cad37695d4580…` panic: attempt to add with overflow |
|   |   |   | idx=40 `pop ds` `114d126881d91966…` panic: attempt to add with overflow |
|   |   |   | idx=71 `pop ds` `d13f4591354f07de…` panic: attempt to add with overflow |
| `20` | 3972 | 1971 | FAIL=2001 |
|   |   |   | idx=3 `and [ds:bx+si+3Eh],bh` `149925a9f70e7298…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=4 `and [ss:bp+si+46F0h],dh` `34f6c53a6f2679be…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=7 `and [ss:bp+4622h],ch` `2c48797220a86743…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `21` | 3975 | 1948 | FAIL=1992 |
|   |   |   | idx=0 `and cx,bp` `21777f5f35e7846e…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=1 `and di,sp` `03945d3c10fb8aec…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=2 `and bp,bx` `80c2e8e2f9d9808a…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `22` | 3975 | 1973 | FAIL=2002 |
|   |   |   | idx=3 `and bl,dh` `8da1c60aaacca2f2…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=4 `and ah,[ss:bp+di]` `ea8308369c4c6260…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
|   |   |   | idx=13 `and bh,ah` `de385bdfa2c97f5e…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
| `23` | 3975 | 1955 | FAIL=1985 |
|   |   |   | idx=5 `and di,sp` `44f6f450aaf69904…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=7 `and sp,[ds:bx+di+70h]` `8362f0f261e6d6fa…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=11 `and bx,si` `5db1d37d03e597fb…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
| `24` | 5000 | 2495 | FAIL=2505 |
|   |   |   | idx=0 `and al,FCh` `eccc20145fa0b46c…` flags exp=0x0446 act=0x0456 umask=0x0FD7; FLAGS exp=0x0446 act=0x0456 |
|   |   |   | idx=1 `and al,FFh` `dde1fc3485ba1101…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=3 `and al,39h` `be1163aa8670f373…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
| `25` | 5000 | 2500 | FAIL=2500 |
|   |   |   | idx=0 `and ax,268Dh` `8bba7374116cdb53…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=2 `and ax,8ADDh` `5841d014a720ff82…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=7 `and ax,A5E0h` `a950515632df4d21…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
| `30` | 3973 | 1965 | FAIL=2008 |
|   |   |   | idx=1 `xor al,bl` `3318050d48c102a0…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
|   |   |   | idx=7 `xor ah,dh` `d7d4ef76ad084c3f…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=8 `xor [ds:A324h],dh` `00cc1fdf790a70ac…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
| `31` | 3971 | 1941 | FAIL=1996 |
|   |   |   | idx=0 `xor [ds:A324h],si` `da5a114e37a83d09…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
|   |   |   | idx=1 `xor [ds:bx+si+42FAh],di` `35104a3c37e110ac…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=3 `xor [ds:bx+si],di` `884bab254e1238db…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
| `32` | 3974 | 1965 | FAIL=2009 |
|   |   |   | idx=0 `xor dh,[ds:bx+si+4]` `40592f626619f906…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=2 `xor ah,[ds:si+5A7Eh]` `dd616d1bbbcd5f56…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=7 `xor ah,[ds:si-Fh]` `217099dcd73cf685…` flags exp=0x0446 act=0x0456 umask=0x0FD7; FLAGS exp=0x0446 act=0x0456 |
| `33` | 3972 | 1934 | FAIL=2004 |
|   |   |   | idx=0 `xor di,si` `bc45583a44ad7cbe…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=4 `xor sp,[ss:bp+di+5Fh]` `afedfe23d56ba228…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=6 `xor dx,[ss:bp+di+7Dh]` `0a6762c819e81743…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `34` | 5000 | 2491 | FAIL=2509 |
|   |   |   | idx=1 `xor al,BAh` `50dfb63fb01d0025…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=2 `xor al,C3h` `f7dc5ea1ff7b544d…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=4 `xor al,57h` `c59b6f998ab5f0ac…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
| `35` | 5000 | 2494 | FAIL=2506 |
|   |   |   | idx=0 `xor ax,0` `1eed4e8f1f08884c…` flags exp=0x0046 act=0x0056 umask=0x0FD7; FLAGS exp=0x0046 act=0x0056 |
|   |   |   | idx=3 `xor ax,F137h` `7e48d9c7f919d0ef…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=4 `xor ax,A4DEh` `9ae6039e7665841d…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
| `58` | 4859 | 4669 | PANIC=190 |
|   |   |   | idx=18 `pop ax` `e8245b2359904b65…` panic: attempt to add with overflow |
|   |   |   | idx=50 `pop ax` `0daefc9fef49e59c…` panic: attempt to add with overflow |
|   |   |   | idx=117 `pop ax` `52bac3e33546b1c7…` panic: attempt to add with overflow |
| `59` | 4859 | 4670 | PANIC=189 |
|   |   |   | idx=26 `pop cx` `52d62f3723bf5a20…` panic: attempt to add with overflow |
|   |   |   | idx=58 `pop cx` `34f9bbc6ad12e18b…` panic: attempt to add with overflow |
|   |   |   | idx=125 `pop cx` `3d7224ecac1e78d7…` panic: attempt to add with overflow |
| `5A` | 4861 | 4671 | PANIC=190 |
|   |   |   | idx=2 `pop dx` `f17881ceec88d0c1…` panic: attempt to add with overflow |
|   |   |   | idx=34 `pop dx` `cb940934b945532b…` panic: attempt to add with overflow |
|   |   |   | idx=101 `pop dx` `dc7e157c2954013c…` panic: attempt to add with overflow |
| `5B` | 4858 | 4669 | PANIC=189 |
|   |   |   | idx=10 `pop bx` `cf9e1210d9420d75…` panic: attempt to add with overflow |
|   |   |   | idx=42 `pop bx` `77db64b9097efa46…` panic: attempt to add with overflow |
|   |   |   | idx=109 `pop bx` `d0e68ec6b7d7a8a5…` panic: attempt to add with overflow |
| `5C` | 4860 | 4666 | PANIC=194 |
|   |   |   | idx=18 `pop sp` `afc9e22ddc853119…` panic: attempt to add with overflow |
|   |   |   | idx=50 `pop sp` `4de0ffce489586a5…` panic: attempt to add with overflow |
|   |   |   | idx=85 `pop sp` `413fff18f764184f…` panic: attempt to add with overflow |
| `5D` | 4860 | 4670 | PANIC=190 |
|   |   |   | idx=26 `pop bp` `fd658c75c7fed498…` panic: attempt to add with overflow |
|   |   |   | idx=58 `pop bp` `e28690acb8766c07…` panic: attempt to add with overflow |
|   |   |   | idx=93 `pop bp` `b5744d315a157746…` panic: attempt to add with overflow |
| `5E` | 4858 | 4668 | PANIC=190 |
|   |   |   | idx=2 `pop si` `a4b9b52423ee216c…` panic: attempt to add with overflow |
|   |   |   | idx=34 `pop si` `4a12fca9925eac52…` panic: attempt to add with overflow |
|   |   |   | idx=69 `pop si` `bcd7a5d24464d22b…` panic: attempt to add with overflow |
| `5F` | 4859 | 4670 | PANIC=189 |
|   |   |   | idx=10 `pop di` `a85d97ee0f04a54c…` panic: attempt to add with overflow |
|   |   |   | idx=42 `pop di` `393335c02897ccdf…` panic: attempt to add with overflow |
|   |   |   | idx=77 `pop di` `608aee387acb28ad…` panic: attempt to add with overflow |
| `69` | 3986 | 224 | FAIL=3726 |
| `6B` | 3982 | 235 | FAIL=3711 |
|   |   |   | idx=2 `imul si,[ds:bx+si-51h],FFF4h` `ab64f9df2ab273e8…` flags exp=0x0C17 act=0x0C53 umask=0x0FD7; FLAGS exp=0x0C17 act=0x0C53 |
| `70` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `71` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `72` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `73` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `74` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `75` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `76` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `78` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `79` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `7A` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `7B` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `7C` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `7D` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `7E` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `7F` | 5000 | 5000 | FAIL=0 (SST-D-012 RESOLVED) |
| `80.1` | 3972 | 1988 | FAIL=1984 |
|   |   |   | idx=1 `or byte [ds:bx+124Eh],B3h` `baf6d20652456c60…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=3 `or dh,51h` `d81bd93ffafb22c3…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=4 `or byte [ss:bp+di-21h],62h` `f1d9bef996b15772…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
| `80.4` | 3972 | 1988 | FAIL=1984 |
|   |   |   | idx=1 `and byte [ss:bp+di-21h],62h` `30b43c6f775626e8…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=2 `and byte [ds:bx],Ah` `e2a6cb4c8d1a464c…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=4 `and byte [ds:bx+124Eh],B3h` `cffcff970146b693…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
| `80.6` | 3972 | 1988 | FAIL=1984 |
|   |   |   | idx=0 `xor byte [ds:bx],Ah` `b34ead8042f24d3b…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=3 `xor byte [ss:bp+di-21h],62h` `a9c6c66e57da6050…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=4 `xor dh,51h` `0e2c49463e5230fa…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
| `81.1` | 3971 | 1967 | FAIL=1975 |
|   |   |   | idx=3 `or di,984Ch` `8b44d1bac98231ce…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=9 `or word [ds:bx+124Eh],C6B3h` `bb78cd0243363b90…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=11 `or si,C751h` `5b7c381bd7d3ec86…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `81.4` | 3970 | 1967 | FAIL=1974 |
|   |   |   | idx=6 `and di,984Ch` `ad97930f7cbad2a8…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=9 `and word [ss:bp+di-21h],F762h` `de11338e02387c56…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=10 `and word [ds:bx],F00Ah` `ca721118495894c2…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
| `81.6` | 3970 | 1966 | FAIL=1975 |
|   |   |   | idx=4 `xor di,984Ch` `ad17f21d707d8e6d…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=8 `xor word [ds:bx],F00Ah` `5750c73f549a76ce…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=11 `xor word [ss:bp+di-21h],F762h` `74db986b62b0d9c6…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `82.1` | 3970 | 1987 | FAIL=1983 |
|   |   |   | idx=0 `or dh,45h` `d1d37423ef80248f…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=1 `or byte [ds:bx+si],BBh` `a3fc42e2d4421ac3…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=2 `or byte [ds:bx+si-49h],47h` `bc626ef8dace808c…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
| `82.4` | 3970 | 1987 | FAIL=1983 |
|   |   |   | idx=1 `and byte [ds:bx+si],90h` `ef83841f4104828d…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=4 `and byte [ds:bx+si],BBh` `a0736c17bab2349e…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=5 `and dh,45h` `fba36298fd9e40ee…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
| `82.6` | 3970 | 1987 | FAIL=1983 |
|   |   |   | idx=3 `xor byte [ds:bx+si],90h` `6f8b05f75437970f…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=5 `xor byte [ds:bx+si-49h],47h` `5c362c220960ee06…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=6 `xor byte [ds:bx+si],BBh` `8e5cf0c4e7bf3150…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `83.1` | 3971 | 1967 | FAIL=1975 |
|   |   |   | idx=0 `or word [ds:bx+si-4Bh],FFE7h` `18c40e3d0c110636…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=1 `or word [ds:bx+di],2Dh` `ffd631b691a33a1e…` flags exp=0x0482 act=0x0492 umask=0x0FD7; FLAGS exp=0x0482 act=0x0492 |
|   |   |   | idx=3 `or bp,FFBCh` `c82facdcae950ec3…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
| `83.4` | 3969 | 1968 | FAIL=1972 |
|   |   |   | idx=1 `and word [ds:bx+si],FFC3h` `929697a4e3c608b4…` flags exp=0x0046 act=0x0056 umask=0x0FD7; FLAGS exp=0x0046 act=0x0056 |
|   |   |   | idx=4 `and word [ds:bx+di],2Dh` `d708549353af9391…` flags exp=0x0446 act=0x0456 umask=0x0FD7; FLAGS exp=0x0446 act=0x0456 |
|   |   |   | idx=5 `and word [ds:bx+si-4Bh],FFE7h` `e5dde8e89cea5892…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
| `83.6` | 3970 | 1967 | FAIL=1974 |
|   |   |   | idx=3 `xor word [ds:bx+si],FFC3h` `c8d1d7cdf0ea38e6…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=4 `xor bp,FFBCh` `6b453ced39d32e62…` flags exp=0x0082 act=0x0092 umask=0x0FD7; FLAGS exp=0x0082 act=0x0092 |
|   |   |   | idx=6 `xor word [ds:bx+di],2Dh` `f63a568ecf62e491…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `84` | 3971 | 1989 | FAIL=1982 |
|   |   |   | idx=2 `test dl,al` `47406be2ac194a7b…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=6 `test ah,dl` `6c7fc8636458d1b6…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
|   |   |   | idx=10 `test bl,ah` `9244d0e7588e9ed1…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
| `85` | 3970 | 1965 | FAIL=1976 |
|   |   |   | idx=2 `test bx,sp` `d2822c9d62032ae8…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=3 `test [ds:bx+di+36h],sp` `649f6b9d7b534240…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=4 `test [ss:bp+si+3Dh],cx` `8186d30e7e08de87…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
| `86` | 3970 | 3970 | FAIL=0 |
|   |   |   | (fixed 2026-08-16, SST-D-006: XCHG mem EA computed pre-swap) |
| `87` | 3981 | 3952 | FAIL=0 |
|   |   |   | (fixed 2026-08-16, SST-D-006: XCHG mem EA computed pre-swap) |
| `8F` | 4864 | 4453 | PANIC=189 |
|   |   |   | idx=1 `pop word [ds:bx+si+44h]` `5d573d0859cf0842…` panic: attempt to add with overflow |
|   |   |   | idx=8 `pop word [ds:di]` `d7f97c53b0303759…` panic: attempt to add with overflow |
|   |   |   | idx=10 `pop word [ds:si]` `d53abdf278a522e1…` panic: attempt to add with overflow |
| `9D` | 4850 | 4657 | PANIC=193 |
|   |   |   | idx=13 `popf` `ea42376309085237…` panic: attempt to add with overflow |
|   |   |   | idx=88 `popf` `4ebe3f7e8608fc8b…` panic: attempt to add with overflow |
|   |   |   | idx=127 `popf` `fdf95fd3a395bb7f…` panic: attempt to add with overflow |
| `A8` | 5000 | 2505 | FAIL=2495 |
|   |   |   | idx=0 `test al,16h` `8200412502a76f06…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
|   |   |   | idx=3 `test al,F5h` `781c6bb13180b01a…` flags exp=0x0086 act=0x0096 umask=0x0FD7; FLAGS exp=0x0086 act=0x0096 |
|   |   |   | idx=6 `test al,69h` `ca6a24649227c082…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
| `A9` | 5000 | 2504 | FAIL=2496 |
|   |   |   | idx=1 `test ax,2A2Fh` `8028ba3f90c767f8…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=2 `test ax,805Bh` `d347863b43f92db4…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=5 `test ax,C962h` `812af671a0d91610…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
| `C0.0` | 3971 | 2359 | FAIL=1612 |
|   |   |   | idx=1 `rol byte [ds:bx+si-29h],25h` `42d704860f75f210…` flags exp=0x0C97 act=0x0497 umask=0x0FD7; FLAGS exp=0x0C97 act=0x0497 (only OF(11) undefined, count>1) |
| `C0.4` | 3971 | 1247 | FAIL=2724 |
|   |   |   | idx=0 `shl byte [ss:bp+si],4Ah` `e6501a1380a44db1…` flags exp=0x0046 act=0x0846 umask=0x0FD7; FLAGS exp=0x0046 act=0x0846 |
|   |   |   | idx=1 `shl ch,9Fh` `120f83faa1cb956e…` flags exp=0x0046 act=0x0056 umask=0x0FD7; FLAGS exp=0x0046 act=0x0056 |
|   |   |   | idx=3 `shl byte [ds:EDBFh],DDh` `b19b6a19ab0ed940…` flags exp=0x0046 act=0x0856 umask=0x0FD7; FLAGS exp=0x0046 act=0x0856 |
| `C0.5` | 3971 | 1278 | FAIL=2693 |
|   |   |   | idx=1 `shr byte [ss:bp+si],4Ah` `186c052a7b048ddd…` flags exp=0x0056 act=0x0846 umask=0x0FD7; FLAGS exp=0x0056 act=0x0846 |
|   |   |   | idx=2 `shr byte [ds:EDBFh],DDh` `20b4a5a2cafd79da…` flags exp=0x0056 act=0x0856 umask=0x0FD7; FLAGS exp=0x0056 act=0x0856 |
|   |   |   | idx=5 `shr ah,FFh` `ba8393efd97e2b63…` flags exp=0x0456 act=0x0C46 umask=0x0FD7; FLAGS exp=0x0456 act=0x0C46 |
| `C0.6` | 3971 | 1245 | FAIL=2726 |
|   |   |   | idx=1 `sal byte [ds:EDBFh],DDh` `e53ebcb8468bba57…` flags exp=0x0046 act=0x0856 umask=0x0FD7; FLAGS exp=0x0046 act=0x0856 |
|   |   |   | idx=2 `sal byte [ss:bp+si],4Ah` `d2d091b3f373a6fe…` flags exp=0x0046 act=0x0846 umask=0x0FD7; FLAGS exp=0x0046 act=0x0846 |
|   |   |   | idx=3 `sal ch,9Fh` `bdcae2ac0c4b57c9…` flags exp=0x0046 act=0x0056 umask=0x0FD7; FLAGS exp=0x0046 act=0x0056 |
| `C0.7` | 3971 | 1278 | FAIL=2693 |
|   |   |   | idx=0 `sar byte [ds:EDBFh],DDh` `3f6c78fb359fbbb0…` flags exp=0x0056 act=0x0856 umask=0x0FD7; FLAGS exp=0x0056 act=0x0856 |
|   |   |   | idx=3 `sar byte [ss:bp+si],4Ah` `f366e9ea5e9b93e0…` flags exp=0x0097 act=0x0887 umask=0x0FD7; FLAGS exp=0x0097 act=0x0887 |
|   |   |   | idx=4 `sar byte [ds:si-38h],C1h` `039c6fafde14fc3f…` flags exp=0x0093 act=0x0083 umask=0x0FD7; FLAGS exp=0x0093 act=0x0083 |
| `C1.0` | 3971 | 2189 | FAIL=1754 |
|   |   |   | idx=1 `rol word [ss:bp+di],77h` `2f312bb94476919e…` flags exp=0x08D3 act=0x00D3 umask=0x0FD7; FLAGS exp=0x08D3 act=0x00D3 (only OF(11) undefined, count>1) |
| `C1.4` | 3973 | 1230 | FAIL=2716 |
|   |   |   | idx=1 `shl word [ss:bp+di-1Dh],6` `9b139f2e7435cfcc…` flags exp=0x0882 act=0x0892 umask=0x0FD7; FLAGS exp=0x0882 act=0x0892 |
|   |   |   | idx=2 `shl word [ss:bp+di+6BDh],CBh` `aa19e33e8c0c9692…` flags exp=0x0C07 act=0x0C17 umask=0x0FD7; FLAGS exp=0x0C07 act=0x0C17 |
|   |   |   | idx=3 `shl word [ds:bx+si+7110h],Fh` `46a278ff7b99ca1e…` flags exp=0x0847 act=0x0857 umask=0x0FD7; FLAGS exp=0x0847 act=0x0857 |
| `C1.5` | 3974 | 1274 | FAIL=2672 |
|   |   |   | idx=0 `shr word [ss:bp+di-1Dh],6` `a9598688620f50ab…` flags exp=0x0013 act=0x0813 umask=0x0FD7; FLAGS exp=0x0013 act=0x0813 |
|   |   |   | idx=1 `shr word [ss:bp+si-B3Bh],2Bh` `4901b41303ccd4e0…` flags exp=0x0013 act=0x0003 umask=0x0FD7; FLAGS exp=0x0013 act=0x0003 |
|   |   |   | idx=2 `shr word [ds:bx+si+7110h],Fh` `16a05454da0356f6…` flags exp=0x0012 act=0x0812 umask=0x0FD7; FLAGS exp=0x0012 act=0x0812 |
| `C1.6` | 3974 | 1223 | FAIL=2723 |
|   |   |   | idx=0 `sal word [ss:bp+di+6BDh],CBh` `4f612338ff4c540c…` flags exp=0x0C07 act=0x0C17 umask=0x0FD7; FLAGS exp=0x0C07 act=0x0C17 |
|   |   |   | idx=1 `sal word [ds:bx+si+7110h],Fh` `f0ec00c4b39a05a2…` flags exp=0x0046 act=0x0856 umask=0x0FD7; FLAGS exp=0x0046 act=0x0856 |
|   |   |   | idx=2 `sal word [ss:bp+si-B3Bh],2Bh` `cb3109c9a173cc24…` flags exp=0x0807 act=0x0007 umask=0x0FD7; FLAGS exp=0x0807 act=0x0007 |
| `C1.7` | 3974 | 1274 | FAIL=2672 |
|   |   |   | idx=0 `sar word [ds:bx+si+7110h],Fh` `32175bcd3c54ddc9…` flags exp=0x0056 act=0x0856 umask=0x0FD7; FLAGS exp=0x0056 act=0x0856 |
|   |   |   | idx=1 `sar word [ss:bp+di+6BDh],CBh` `db2b42518a857da7…` flags exp=0x0497 act=0x0C97 umask=0x0FD7; FLAGS exp=0x0497 act=0x0C97 |
|   |   |   | idx=2 `sar word [ss:bp+di-1Dh],6` `103f503ce9a0621a…` flags exp=0x0093 act=0x0893 umask=0x0FD7; FLAGS exp=0x0093 act=0x0893 |
| `C2` | 4848 | 2481 | PANIC=2343 |
|   |   |   | idx=0 `ret D907h` `140ebe1c3bf05d00…` panic: attempt to add with overflow |
|   |   |   | idx=1 `ret F656h` `cd140d911f80d80a…` panic: attempt to add with overflow |
|   |   |   | idx=2 `ret 7A29h` `0bc7dddb114aeeec…` panic: attempt to add with overflow |
| `C3` | 4846 | 4631 | PANIC=191 |
|   |   |   | idx=11 `ret` `b6b028de50df8ec6…` panic: attempt to add with overflow |
|   |   |   | idx=27 `ret` `98ba955602fec196…` panic: attempt to add with overflow |
|   |   |   | idx=38 `ret` `38b9d073683724b6…` panic: attempt to add with overflow |
| `CA` | 4847 | 2486 | PANIC=2337 |
|   |   |   | idx=0 `retf 6E14h` `002313b72717d06c…` panic: attempt to add with overflow |
|   |   |   | idx=3 `retf 9BAEh` `da45ff73034ccfe9…` panic: attempt to add with overflow |
|   |   |   | idx=5 `retf EB01h` `3e1d1ab53de56e5f…` panic: attempt to add with overflow |
| `CB` | 4848 | 4623 | PANIC=201 |
|   |   |   | idx=3 `retf` `85c8007c8b25ddd1…` panic: attempt to add with overflow |
|   |   |   | idx=7 `retf` `f87d194b46f7aa3a…` panic: attempt to add with overflow |
|   |   |   | idx=30 `retf` `1aba79c78abd2534…` panic: attempt to add with overflow |
| `D0.0` | 3955 | 3955 | FAIL=0 |
|   |   |   | (fixed 2026-08-16, SST-D-003: count==1 ROL CF/OF now correct) |
| `D0.4` | 3955 | 1970 | FAIL=1985 |
|   |   |   | idx=4 `shl dl,1` `11de9b3816641117…` flags exp=0x0097 act=0x0087 umask=0x0FD7; FLAGS exp=0x0097 act=0x0087 |
|   |   |   | idx=7 `shl dl,1` `2028b89b99dc472b…` flags exp=0x0886 act=0x0896 umask=0x0FD7; FLAGS exp=0x0886 act=0x0896 |
|   |   |   | idx=10 `shl byte [ds:si],1` `365e0a9c211562bb…` flags exp=0x0803 act=0x0813 umask=0x0FD7; FLAGS exp=0x0803 act=0x0813 |
| `D0.5` | 3955 | 1995 | FAIL=1960 |
|   |   |   | idx=0 `shr byte [ss:bp-40h],1` `08b3cf87512224d2…` flags exp=0x0C17 act=0x0C07 umask=0x0FD7; FLAGS exp=0x0C17 act=0x0C07 |
|   |   |   | idx=1 `shr byte [ds:bx+si+7Fh],1` `0c36c50bb6bc578b…` flags exp=0x0012 act=0x0002 umask=0x0FD7; FLAGS exp=0x0012 act=0x0002 |
|   |   |   | idx=4 `shr byte [ss:bp+di-72DDh],1` `cc9226cc380994fb…` flags exp=0x0C13 act=0x0C03 umask=0x0FD7; FLAGS exp=0x0C13 act=0x0C03 |
| `D0.6` | 3955 | 2032 | FAIL=1923 |
|   |   |   | idx=5 `sal dl,1` `a6ea5c33666bf9c0…` flags exp=0x0886 act=0x0896 umask=0x0FD7; FLAGS exp=0x0886 act=0x0896 |
|   |   |   | idx=6 `sal dl,1` `b3c0b9f165148e05…` flags exp=0x0097 act=0x0087 umask=0x0FD7; FLAGS exp=0x0097 act=0x0087 |
|   |   |   | idx=8 `sal byte [ds:si],1` `5ada664ce6a27e84…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
| `D0.7` | 3955 | 1995 | FAIL=1960 |
|   |   |   | idx=2 `sar byte [ss:bp-40h],1` `f0ef3e81bd4c192c…` flags exp=0x0413 act=0x0403 umask=0x0FD7; FLAGS exp=0x0413 act=0x0403 |
|   |   |   | idx=3 `sar byte [ds:bx+si+7Fh],1` `94e131392b05fe83…` flags exp=0x0012 act=0x0002 umask=0x0FD7; FLAGS exp=0x0012 act=0x0002 |
|   |   |   | idx=6 `sar byte [ss:bp+di-72DDh],1` `aef14d000ec9a818…` flags exp=0x0413 act=0x0403 umask=0x0FD7; FLAGS exp=0x0413 act=0x0403 |
| `D1.0` | 3955 | 3927 | FAIL=0 |
|   |   |   | (fixed 2026-08-16, SST-D-003: count==1 ROL CF/OF now correct) |
| `D1.4` | 3955 | 1987 | FAIL=1940 |
|   |   |   | idx=1 `shl word [ds:bx],1` `1c97de7ad3801101…` flags exp=0x0817 act=0x0807 umask=0x0FD7; FLAGS exp=0x0817 act=0x0807 |
|   |   |   | idx=4 `shl word [ss:bp+di-40F5h],1` `0b5a2ad394a94bd4…` flags exp=0x0097 act=0x0087 umask=0x0FD7; FLAGS exp=0x0097 act=0x0087 |
|   |   |   | idx=5 `shl word [ds:bx],1` `650771c8b5cf318d…` flags exp=0x0892 act=0x0882 umask=0x0FD7; FLAGS exp=0x0892 act=0x0882 |
| `D1.5` | 3955 | 1987 | FAIL=1940 |
|   |   |   | idx=0 `shr word [ds:bx],1` `4fcb8ac45556cca5…` flags exp=0x0817 act=0x0807 umask=0x0FD7; FLAGS exp=0x0817 act=0x0807 |
|   |   |   | idx=1 `shr word [ss:bp+si+63h],1` `fbeba17baa8e6664…` flags exp=0x0012 act=0x0002 umask=0x0FD7; FLAGS exp=0x0012 act=0x0002 |
|   |   |   | idx=4 `shr word [ds:bx],1` `8de99e3e09ffd323…` flags exp=0x0013 act=0x0003 umask=0x0FD7; FLAGS exp=0x0013 act=0x0003 |
| `D1.6` | 3955 | 1947 | FAIL=1980 |
|   |   |   | idx=2 `sal word [ss:bp+si+63h],1` `74ea3f9726487a54…` flags exp=0x0896 act=0x0886 umask=0x0FD7; FLAGS exp=0x0896 act=0x0886 |
|   |   |   | idx=6 `sal word [ss:bp+di-40F5h],1` `c6fda9f15a99817e…` flags exp=0x0012 act=0x0002 umask=0x0FD7; FLAGS exp=0x0012 act=0x0002 |
|   |   |   | idx=11 `sal word [ss:bp-40h],1` `b0302a997fe18bac…` flags exp=0x0493 act=0x0483 umask=0x0FD7; FLAGS exp=0x0493 act=0x0483 |
| `D1.7` | 3955 | 1987 | FAIL=1940 |
|   |   |   | idx=2 `sar word [ds:bx],1` `c610ea1b61796aed…` flags exp=0x0012 act=0x0002 umask=0x0FD7; FLAGS exp=0x0012 act=0x0002 |
|   |   |   | idx=3 `sar word [ss:bp+si+63h],1` `ba6592efda928ccf…` flags exp=0x0013 act=0x0003 umask=0x0FD7; FLAGS exp=0x0013 act=0x0003 |
|   |   |   | idx=6 `sar word [ds:bx],1` `8e149aba4dc8c770…` flags exp=0x0093 act=0x0083 umask=0x0FD7; FLAGS exp=0x0093 act=0x0083 |
| `D2.0` | 3957 | 2368 | FAIL=1589 |
|   |   |   | idx=0 `rol byte [ds:bx+si+5B6Fh],cl` `849ac602cadf7aa2…` flags exp=0x0856 act=0x0056 umask=0x0FD7; FLAGS exp=0x0856 act=0x0056 (only OF(11) undefined, count>1) |
| `D2.4` | 3957 | 1259 | FAIL=2698 |
|   |   |   | idx=1 `shl dl,cl` `0549b9a7b9e02832…` flags exp=0x0C47 act=0x0C57 umask=0x0FD7; FLAGS exp=0x0C47 act=0x0C57 |
|   |   |   | idx=2 `shl byte [ds:E841h],cl` `a330abb69ab6128b…` flags exp=0x0C86 act=0x0496 umask=0x0FD7; FLAGS exp=0x0C86 act=0x0496 |
|   |   |   | idx=3 `shl byte [ss:bp-1Bh],cl` `42812fa93bd61306…` flags exp=0x0046 act=0x0856 umask=0x0FD7; FLAGS exp=0x0046 act=0x0856 |
| `D2.5` | 3957 | 1289 | FAIL=2668 |
|   |   |   | idx=0 `shr dl,cl` `e720448c5baf072c…` flags exp=0x0457 act=0x0C57 umask=0x0FD7; FLAGS exp=0x0457 act=0x0C57 |
|   |   |   | idx=2 `shr byte [ss:bp-1Bh],cl` `abd250993c9451e1…` flags exp=0x0056 act=0x0856 umask=0x0FD7; FLAGS exp=0x0056 act=0x0856 |
|   |   |   | idx=6 `shr byte [ds:si],cl` `0af9010778e17e04…` flags exp=0x0456 act=0x0C46 umask=0x0FD7; FLAGS exp=0x0456 act=0x0C46 |
| `D2.6` | 3957 | 1276 | FAIL=2681 |
|   |   |   | idx=0 `sal byte [ds:E841h],cl` `b5811d27b1d08f23…` flags exp=0x0487 act=0x0497 umask=0x0FD7; FLAGS exp=0x0487 act=0x0497 |
|   |   |   | idx=1 `sal byte [ss:bp-1Bh],cl` `dfa9f0549f4268a4…` flags exp=0x0046 act=0x0856 umask=0x0FD7; FLAGS exp=0x0046 act=0x0856 |
|   |   |   | idx=2 `sal byte [ss:bp+si+1D6Bh],cl` `5e0210a343feca2b…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
| `D2.7` | 3957 | 1289 | FAIL=2668 |
|   |   |   | idx=0 `sar byte [ss:bp-1Bh],cl` `0b9b4481f28181f2…` flags exp=0x0097 act=0x0897 umask=0x0FD7; FLAGS exp=0x0097 act=0x0897 |
|   |   |   | idx=2 `sar dl,cl` `3113268fe2be5b60…` flags exp=0x0497 act=0x0C97 umask=0x0FD7; FLAGS exp=0x0497 act=0x0C97 |
|   |   |   | idx=4 `sar byte [ds:si],cl` `6894759aa5b27b59…` flags exp=0x0456 act=0x0C46 umask=0x0FD7; FLAGS exp=0x0456 act=0x0C46 |
| `D3.0` | 3955 | 2256 | FAIL=1671 |
|   |   |   | idx=1 `rol word [ds:bx],cl` `0da54c82fc9a08eb…` flags exp=0x04D7 act=0x04D6 umask=0x0FD7; FLAGS exp=0x04D7 act=0x04D6 (only OF(11) undefined, count>1) |
| `D3.4` | 3960 | 1267 | FAIL=2665 |
|   |   |   | idx=0 `shl word [ds:si-1C4Dh],cl` `69df0df536c4177e…` flags exp=0x0C86 act=0x0496 umask=0x0FD7; FLAGS exp=0x0C86 act=0x0496 |
|   |   |   | idx=3 `shl di,cl` `79b6ed707c4334d5…` flags exp=0x0C47 act=0x0447 umask=0x0FD7; FLAGS exp=0x0C47 act=0x0447 |
|   |   |   | idx=5 `shl word [ds:bx],cl` `a27638b00fa82ba0…` flags exp=0x0446 act=0x0456 umask=0x0FD7; FLAGS exp=0x0446 act=0x0456 |
| `D3.5` | 3957 | 1278 | FAIL=2651 |
|   |   |   | idx=0 `shr ax,cl` `d44a3e758d0b649b…` flags exp=0x0456 act=0x0446 umask=0x0FD7; FLAGS exp=0x0456 act=0x0446 |
|   |   |   | idx=2 `shr di,cl` `df3522294b6fc70a…` flags exp=0x0456 act=0x0446 umask=0x0FD7; FLAGS exp=0x0456 act=0x0446 |
|   |   |   | idx=3 `shr word [ds:bx+73h],cl` `3886a4e49c7be207…` flags exp=0x0056 act=0x0046 umask=0x0FD7; FLAGS exp=0x0056 act=0x0046 |
| `D3.6` | 3960 | 1278 | FAIL=2654 |
|   |   |   | idx=1 `sal di,cl` `e30d324502e6d602…` flags exp=0x0C47 act=0x0447 umask=0x0FD7; FLAGS exp=0x0C47 act=0x0447 |
|   |   |   | idx=2 `sal word [ds:si-1C4Dh],cl` `dbd088295143635d…` flags exp=0x0C86 act=0x0496 umask=0x0FD7; FLAGS exp=0x0C86 act=0x0496 |
|   |   |   | idx=5 `sal word [ds:bx+si-6164h],cl` `e6e7e6c8a9536797…` flags exp=0x0087 act=0x0897 umask=0x0FD7; FLAGS exp=0x0087 act=0x0897 |
| `D3.7` | 3957 | 1279 | FAIL=2650 |
|   |   |   | idx=0 `sar di,cl` `4f060f3ad6ebdbdc…` flags exp=0x0456 act=0x0446 umask=0x0FD7; FLAGS exp=0x0456 act=0x0446 |
|   |   |   | idx=1 `sar word [ds:bx+73h],cl` `7b4b0ca7a580a515…` flags exp=0x0097 act=0x0087 umask=0x0FD7; FLAGS exp=0x0097 act=0x0087 |
|   |   |   | idx=2 `sar ax,cl` `d42802343e520624…` flags exp=0x0497 act=0x0487 umask=0x0FD7; FLAGS exp=0x0497 act=0x0487 |
| `D7` | 3957 | 3734 | PANIC=223 |
|   |   |   | idx=38 `xlatb` `fcaa2b4557093807…` panic: attempt to add with overflow |
|   |   |   | idx=39 `xlatb` `d59bb504647cb61e…` panic: attempt to add with overflow |
|   |   |   | idx=52 `xlatb` `c6a11aab7147d87b…` panic: attempt to add with overflow |
| `E0` | 4849 | 4849 | FAIL=0 (SST-D-012 RESOLVED) |
| `E1` | 4846 | 4846 | FAIL=0 (SST-D-012 RESOLVED) |
| `E2` | 4848 | 4848 | FAIL=0 (SST-D-012 RESOLVED) |
| `E3` | 4847 | 4847 | FAIL=0 (SST-D-012 RESOLVED) |
| `EB` | 3964 | 3964 | FAIL=0 (SST-D-012 RESOLVED) |
| `F6.0` | 3968 | 1967 | FAIL=2001 |
|   |   |   | idx=2 `test byte [ss:bp+30h],75h` `b7529a4b5f3d43bf…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=4 `test byte [ds:bx+di],6Ah` `91ec7868d388ba88…` flags exp=0x0046 act=0x0056 umask=0x0FD7; FLAGS exp=0x0046 act=0x0056 |
|   |   |   | idx=6 `test byte [ds:si+3D3Bh],D0h` `7050d9eca172bed6…` flags exp=0x0486 act=0x0496 umask=0x0FD7; FLAGS exp=0x0486 act=0x0496 |
| `F6.1` | 3968 | 1967 | FAIL=2001 |
|   |   |   | idx=3 `test byte [ss:bp+30h],75h` `3cefa308079630b8…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
|   |   |   | idx=5 `test byte [ds:bx+di],6Ah` `c0429a3b1f65f758…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
|   |   |   | idx=7 `test byte [ds:si+3D3Bh],D0h` `87af58e0cf4c2960…` flags exp=0x0446 act=0x0456 umask=0x0FD7; FLAGS exp=0x0446 act=0x0456 |
| `F7.0` | 3967 | 1945 | FAIL=1994 |
|   |   |   | idx=2 `test word [ds:si-5Bh],DB8h` `776fb02b8cd28995…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=4 `test word [ds:bx+53h],C624h` `0f54cf98d8322aa8…` flags exp=0x0402 act=0x0412 umask=0x0FD7; FLAGS exp=0x0402 act=0x0412 |
|   |   |   | idx=10 `test word [ss:bp+30h],9E75h` `26ad0782119a7d9f…` flags exp=0x0002 act=0x0012 umask=0x0FD7; FLAGS exp=0x0002 act=0x0012 |
| `F7.1` | 3968 | 1945 | FAIL=1995 |
|   |   |   | idx=3 `test word [ds:si-5Bh],DB8h` `cb38e6da3db8a4f6…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=5 `test word [ds:bx+53h],C624h` `f3206f8efeb4ba59…` flags exp=0x0406 act=0x0416 umask=0x0FD7; FLAGS exp=0x0406 act=0x0416 |
|   |   |   | idx=11 `test word [ss:bp+30h],9E75h` `07e13d20c586e5c4…` flags exp=0x0006 act=0x0016 umask=0x0FD7; FLAGS exp=0x0006 act=0x0016 |
| `F7.3` | 3967 | 3938 | PANIC=1 |
|   |   |   | idx=1207 `neg word [ds:bx+si]` `d4d5e5cb5b0d37c9…` panic: attempt to negate with overflow |
| `F7.5` | 3958 | 3930 | FAIL=0 |
| `F7.7` | 3965 | 1389 | FAIL=0 PANIC=0 |
|   |   |   | (RESOLVED 2026-08-16 F8: signed IDIV; idx=7 sample now PASS) |
| `FF.3` | 3968 | 2948 | FAIL=0 |
| `FF.5` | 3968 | 2945 | FAIL=0 |

## Divergence clusters — classification

Taxonomy: **emu86-bug** | **emu86-intentional-8086-semantics** |
**harness-caveat** | **test-suite-edge**. No cluster is `test-suite-edge` (the
Harris captures are self-consistent and hash-pinned; every divergence reproduces
with a rationale below). All FAIL clusters below were verified at the bit level
(sample extraction) and, where noted, by re-running with an explicit `--umask`.

### FAIL clusters

**SST-D-001 — logical ops AF(4) bit: emu86 leaves AF unchanged, Harris clears it to 0.**
Forms: 08-0D, 20-25, 30-35, 80.1/80.4/80.6, 81.1/81.4/81.6, 82.1/82.4/82.6,
83.1/83.4/83.6, 84, 85, A8, A9, F6.0, F6.1, F7.0, F7.1 — 79,739 FAIL.
`alu::update_flags_bitwise` sets CF/ZF/SF/OF/PF but never AF; emu86 therefore
preserves the init AF (the SST initial state carries AF=1 in these tests), while
the 80C286 leaves AF=0 after AND/OR/XOR/TEST. Intel 80286 documents AF as
*undefined* for logical ops, so the divergence lives entirely in an
architecturally-undefined bit. Verified: `--umask 0x0FC7` (default 0x0FD7 minus
AF) turns all 08/20/30-class FAILs into PASS. Classification: **harness-caveat**
(policy flags_umask gap — logical-op forms should mask AF; not an emu86 semantic
error because the bit is architecturally undefined).

**SST-D-002 — shift/rotate-adjacent AF/OF undefined-bit noise.**
Forms: C0.0, C0.4-C0.7, C1.0, C1.4-C1.7, D0.4-D0.7, D1.4-D1.7, D2.0,
D2.4-D2.7, D3.0, D3.4-D3.7 — 65,208 FAIL (58,582 previously: 47,799 for
8-bit/16-bit-by-1/CL forms + 10,783 C1.x FAIL; +5,552 after the SST-D-009 fix,
when formerly-PANIC C1.4-C1.7 imm-count tests completed and failed on these same
undefined bits; +6,626 after the SST-D-004-adjacent ROL fix (SST-D-003), when
the count>1 ROL forms (C0.0, C1.0, D2.0, D3.0) reduced to their undefined-OF
residual — those forms now differ **only** on OF(11) for count>1, and pass
under `--umask 0x07F7`).
Bit analysis: AF(4) appears everywhere; OF(11) appears for count>1 forms
(C0.x imm-count, D2.x/D3.x by-CL, and now the count>1 ROL residuals). Intel
80286: SHL/SHR/SAR/ROL define OF only for count==1 and leave AF undefined;
emu86's `update_flags_shl/shr/sar` correctly set OF only for count==1 and never
touch AF, and the ROL arm (SST-D-003, fixed) matches for count==1 — so emu86's
*defined* bits match; the FAILs are entirely in undefined AF and (count>1)
undefined OF. Classification: **harness-caveat** (policy per-form umask cannot
express "OF defined only when count==1"; residual undefined-bit comparison).

**SST-D-003 — ROL: emu86 does not update CF/OF at all.**
Forms: C0.0, C1.0 (FAIL part), D0.0, D1.0, D2.0, D3.0 — 16,885 FAIL (15,472 +
1,413 C1.0 from the formerly-PANIC C1.x imm-count tests, which now complete and
fail on the missing ROL CF/OF update).
`alu::shift(ShiftOp::Rol)` contained `// TODO SET FLAGS ?` and left every flag
unchanged; the 80C286 sets CF (rotated-out bit) and, for count==1, OF. Samples
all show `CF(0)`/`OF(11)` diffs, e.g. `rol dl,1` exp=0x0087 act=0x0886. Intel
80286 defines CF for ROL (and OF for count==1), so this is a genuine emu86
deficiency. Classification: **emu86-bug** (ROL flag update missing).

**RESOLVED 2026-08-16 (F4):** the ROL arm of `alu::shift` now computes the
effective count (`n & 0x1f`), sets `CF = bit((width - n_mod) % width)` of the
original value (for `n != 0`; `n == 0` is a complete no-op leaving all flags
unchanged), and sets `OF = CF XOR new MSB` only when `n_mod == 1`. ZF/SF/PF/AF
are preserved (rotate does not touch them), matching the 80C286. Verified on
the pinned D1.0 sample (`rol word [ss:bp+di-40F5h],1`: 0x6B2D → 0xD65A, flags
0x0843 → 0x0842) and on all six ROL forms under `--umask 0x07F7` (OF masked):
**23,680/23,680 PASS, 0 FAIL, 0 PANIC**. Without the mask the count==1 forms
(D0.0, D1.0) pass 100% (PASS=3955/3927, FAIL=0); the remaining count>1 FAILs
(6,626 total: C0.0 1612, C1.0 1754, D2.0 1589, D3.0 1671) differ **only** on
the undefined-OF bit (0x0800) — the SST-D-002 harness-caveat (see that
cluster). Fix demonstrated by the ROL mirror tests in `shift_semantics_test.rs`
(count==1 CF/OF, count>1 CF+preserved-OF, full-circle CF=old-LSB, count==0
no-op); `cargo test --locked --all-targets` 325 passed; hermetic micro lane
green after moving D1.0 from FAILREPRO to the PASS list (spec.txt:55,
micro.rs PASS_STEMS, FAILREPRO.txt).

**SST-D-004 — IMUL: CF/OF overflow flag diverges from hardware.**
Forms: 69, 6B, F7.5 — residual FAIL after masking the Intel-undefined IMUL bits:
under `--umask 0x0801` (CF/OF only, per Intel 80286 "IMUL affects only CF/OF")
69 → 90 FAIL, 6B → 250 FAIL, F7.5 → 167 FAIL (F7.5 policy already 0x0801).
Samples: `imul word [ds:bx-44FAh]` exp=0x0096 act=0x0883 umask=0x0801 — emu86
sets CF/OF=1 where the 80C286 has 0, i.e. emu86's signed-overflow detection
(`alu::multiply` ovf = `(result & value_mask) != result`) disagrees with the
hardware's IMUL overflow flag for some operand pairs. Classification:
**emu86-bug** (IMUL CF/OF computation). Note the *rest* of the 69/6B failures
(69: 3,734→90; 6B: 3,723→250) is undefined SF/ZF/AF/PF noise → **harness-caveat**
for that portion (7,117 FAIL) because the policy note already says "CF/OF defined,
rest undefined (Intel 80286)" but the form's flags_umask was left None.

**RESOLVED 2026-08-16 (F6):** the signed IMUL overflow test in `alu::multiply`
is now the sign-extension check: CF/OF is set only when the product does not fit
the destination half (AX for size 1, DX:AX for size 2), i.e. when the result is
*not* a sign-extension of its low half. The old `(result & value_mask) != result`
wrongly flagged a valid sign-extended result: pinned F7.5 sample
`imul word [ds:bx-44FAh]` with AX=0x01DB × operand=0xFFFF (−1) → 0xFFFFFE25 fits
in a signed word (0xFE25 = −475), hardware CF=OF=0. All three forms now pass
100% under `--umask 0x0801` (69: PASS=3950 FAIL=0; 6B: PASS=3946 FAIL=0; F7.5:
PASS=3930 FAIL=0); F7.5 also passes under the default umask (its policy is
already 0x0801). The residual 69/6B FAILs under the default umask are unchanged
undefined SF/ZF/AF/PF noise (the D-004-undefined harness-caveat, Track 2). Fix
demonstrated by the `imul16_*` / `imul8_*` mirror tests in `alu_test.rs`;
`cargo test --locked --all-targets` 332 passed; hermetic micro lane green after
moving F7.5 from FAILREPRO to the PASS list (spec.txt, micro.rs PASS_STEMS,
FAILREPRO.txt).

**SST-D-005 — IDIV (F7.7): signed division.**
F7.7: 334 FAIL + 723 PANIC + 2,576 SKIP_EXCEPTION.
`alu::divmod` performs unsigned u32/u16 division (`a / b`), but IDIV is signed.
Samples: `idiv word [ss:bp+di]` AX exp=0xE5AA act=0x133E / DX exp=0x5DB7
act=0x2FB3 — emu86's unsigned quotient/remainder diverge from the hardware's
signed ones; and 723 PANIC "Divide Error" (`divmod` asserts `quotient <= 0xffff`
in the unsigned domain, alu.rs:136-137) fire where the 80C286 produced a valid
signed result. The 2,576 SKIP_EXCEPTION are the suite's exception-expected
divide-overflow tests, correctly skipped. Classification: **emu86-bug** (IDIV
computed as unsigned DIV).

**RESOLVED 2026-08-16 (F8):** `Opcode::OP_IDIV` added (decode F6/7 + F7/7 →
`OP_IDIV`; F6/6 + F7/6 stay `OP_DIV`); `alu::divmod` now takes a `DivideOp`
(Unsigned/Signed) mirroring `MultiplyOp`, and the Signed arm does 32/16 signed
division (`a as i32 as i64 / b as i16 as i64`, truncated toward zero, remainder
keeps the dividend's sign) with the divide-error surface kept as a `panic!`
(emu86 has no exception machinery; the corpus's #DE tests are marked
SkipException and never execute the panic). Fix demonstrated by the mirror test
`idiv_word_signed_division_matches_pinned_sample` (pinned F7.7 sample: dividend
DX:AX=0x0B1E:0x9A19, divisor [ss:bp+di]=0x93ED → quotient 0xE5AA / remainder
0x5DB7) plus the alu-level `divmod_signed_*` tests in `shift_semantics_test.rs`;
`cargo test --locked --all-targets` 338 passed; hermetic micro lane green after
moving F7.7 from FAILREPRO to the PASS list (spec.txt, micro.rs PASS_STEMS,
FAILREPRO.txt). Release-mode re-run: F7.7 PASS=1389 FAIL=0 PANIC=0; F7.6 (DIV)
unchanged PASS=1847 FAIL=0 PANIC=0.

**SST-D-006 — XCHG with a memory operand whose EA uses the exchanged register.**
Forms: 86 (283 FAIL), 87 (595 FAIL) — 878 FAIL total.
`OP_XCHG` reads both operands, then writes operand0 then operand1; the memory
operand's EA is re-derived at write time from the *already-updated* register.
For `xchg bh,[ds:bx+di-49h]`: init bx=0xFF99 → expected mem[0x1070AA]=0xFF; emu86
writes 0xFF to 0x104DAA (computed with the post-swap bx=0xDC99). Verified via
`--check-writes` (unexpected write `[0x104DAA] 0->255`). Classification:
**emu86-bug** (XCHG store to re-derived EA).

**RESOLVED 2026-08-16 (F5):** `OP_XCHG` now pre-computes the memory operand's EA
(`operand_mem_addr`) from the *pre-swap* register values before any write, and
stores the old register value to that fixed address via the new
`operand_mem_write_at(addr, val)` helper (`step.rs`). Re-deriving the EA after
the register write was the bug. Verified on the pinned 87 sample `xchg di,[ds:di]`
(bytes 87 3D F4: di=0x8AA6, ds:di=0xB4356, mem=0xA91E → di=0xA91E,
mem[0xB4356]=0x8AA6; the old code wrote 0x8AA6 to the post-swap EA 0xB34CE).
Both affected forms now pass 100% (86: PASS=3970 FAIL=0; 87: PASS=3952 FAIL=0,
0 PANIC). Fix demonstrated by the step-level mirror test
`xchg_mem_ea_uses_pre_swap_register` in `shift_semantics_test.rs`; `cargo test
--locked --all-targets` 326 passed; hermetic micro lane green after moving 87
from FAILREPRO to the PASS list (spec.txt, micro.rs PASS_STEMS, FAILREPRO.txt).

**SST-D-007 — far CALL/JMP through memory: no 64KB offset wrap on the far pointer read.**
Forms: FF.3 (2 FAIL), FF.5 (2 FAIL).
The far pointer is a 4-byte `m16:16` stored in memory. In the 2 failing cases the
pointer *crosses the 64KB offset boundary*: e.g. `call far [ds:bx+di]` with
bx=di=0xFFFF gives EA offset 0xFFFE, so the offset word sits at `ds:0xFFFE`
(linear 0x1FB9E, bytes 7A 83 → IP 0x837A) and the segment word sits at the
*wrapped* `ds:0x0000` (linear 0x0FBA0, bytes 3A 65 → CS 0x653A). Hardware wraps
the effective-address offset at 0x10000 for the 4-byte read; emu86's `read_u32`
reads 4 linear bytes at abs_normal (0x1FBA0 is 0x00 in its RAM) → CS=0x0000.
Samples: `call far [ds:bx+di]` CS exp=0x653A act=0x0000. Only 2 per form (0.05%)
— rare because the pointer must straddle the boundary. Classification:
**emu86-bug** (memory reads do not wrap the EA offset at the 64KB segment
boundary for multi-byte operands).

**RESOLVED 2026-08-16 (F7):** `read_u16`/`read_u32`/`write_u16`/`write_u32`
now wrap each byte's offset at the 64KB segment boundary (`base + ((off + i) &
0xffff)`) instead of reading the linear continuation. Verified on the pinned
FF.5 sample `jmp far [ds:bx+di]` (bytes FF 29 F4, ds=0x0FBA, bx=di=0xFFFF →
EA ds:0xFFFE; IP=0x74CD from ds:0xFFFE/0xFFFF, CS=0x848F from the wrapped
ds:0x0000/0x0001; the old code read the linear continuation at 0x1FBA:0 → CS=
0x0000). Both affected forms now pass 100% (FF.3: PASS=2948 FAIL=0; FF.5:
PASS=2945 FAIL=0, 0 PANIC). Fix demonstrated by the step-level mirror test
`jmp_far_mem_wraps_far_pointer_at_64kb_boundary` in `shift_semantics_test.rs`;
`cargo test --locked --all-targets` 333 passed; hermetic micro lane green after
moving FF.5 from FAILREPRO to the PASS list (spec.txt, micro.rs PASS_STEMS,
FAILREPRO.txt).

**SST-D-012 — short-branch IP off-by-one (runner terminating-HALT convention).**
Forms: Jcc 70-7F (201), LOOP/JCXZ E0-E3 (39), JMP rel8 EB (16) — 256 FAIL.
Pre-existing at baseline `3c0b9ea` (identical counts), unaffected by F1-F7.
Root cause: `runner.rs:230` `compare_state` — the SST suite's terminating-HALT
convention applies `expected = listed_IP − 1` for a *listed* final IP (line 228)
but uses `expected = init` for an *absent* final IP (line 230), skipping the −1.
The failing tests are short branches whose target = `init IP − 1` (backward
`rel8 = -3` onto a `0xF4` HALT byte), so the reference's post-HALT IP wraps back
to `init` and is recorded "unchanged/absent"; emu86 (one step, landing at
`target = init − 1`) is then compared to `expected = init` → off by one. EB
(unconditional) is affected → not a flag caveat. Classification:
**harness/runner bug (false-FAIL)** — emu86 is correct; the runner's absent-IP
comparison is wrong.

**RESOLVED 2026-08-16 (D-012):** the absent-IP arm of `compare_state` now applies
the same `−1` convention (`init[IP_IDX].wrapping_sub(1)` at `runner.rs:230`):
emu86 stops one step before the terminating HALT, so whether the reference's
final IP is listed or absent, the expected emu86 IP is the recorded value minus
one. Zero emu86 changes. Full-corpus stride-1 recount confirms exactly 256
false FAILs flipped to PASS with zero regressions: PASS 849,877 → 850,133,
FAIL 152,640 → 152,384, PANIC 0; all 20 short-branch forms now 100% PASS
(70-7F, E0-E3, EB). The fix is demonstrated by the runner unit test
`absent_final_ip_short_branch_onto_halt_passes_d012` (JMP rel8 = -3 onto a HALT
byte at init−1, absent final IP → PASS); `cargo test --locked --all-targets`
339 passed.

---

## Track 3 R1 — String-family arbitration (A4-AF)

R1 extended the pinned fetch to the string family (A4-AF MOVS/CMPS/STOS/LODS/SCAS,
fetched from the pinned commit; sha256 pinned in `data/sst/manifest.txt`). The
forms are `cap: Implemented` but `scope: Deferred(RepString)` — not in the V1
sweep. Empirical inspection (`emu86_sst run --all`, conservative filter off):

- **Bare string ops (no prefix)**: 100% PASS — emu86's single-iteration string
  ops (MOVS/CMPS/STOS/LODS/SCAS) match hardware exactly.
- **REP-prefixed MOVS/CMPS/STOS/SCAS**: 100% PASS — the `opcode_*` while-loop
  REP implementation (cpu_movs/scas/stos/cmps.rs) completes a whole-REP test in
  one `step()` and matches the hardware final state.
- **Segment-override + string ops** (26/2E/36/3E + A4-AF): FAIL — SST-D-013.
- **REP-prefixed LODS** (F2/F3 + AC/AD): PANIC — SST-D-014.
- **LOCK-prefixed string ops** (F0 + A4-AF): DECODE_ERR — SST-D-015 (open).

**SST-D-013 — segment-override prefix applied to string-op destination.**
Forms: A4-AF with a 26/2E/36/3E prefix. `operand_dst` (decode.rs:57) used
`sreg.unwrap_or(Reg::ES)`, so a segment-override prefix overrode the destination
segment too — but a string-op dest is **always ES:DI**; the override applies
only to the source (DS:SI). For `3E A4` (DS:MOVSB) emu86 wrote to DS:DI instead
of ES:DI. Classification: **emu86-bug** (decoder).
**RESOLVED 2026-08-16 (R1):** `operand_dst` now always uses `Reg::ES`, ignoring
the prefix. Demonstrated by `ds_override_movsb_writes_dest_to_es_not_ds` in
`cpu_movs.rs`; all 10 string forms 0 FAIL with `--all`.

**SST-D-014 — REP LODS panicked ("REP prefix is not yet implemented").**
Forms: AC/AD with F2/F3. The `OP_LODS` arm in step.rs did not `return` early, so
REP LODS fell through to the line-268 `panic!`. Classification: **emu86-bug**.
**RESOLVED 2026-08-16 (R1):** LODS extracted to `cpu_lods.rs::opcode_lods`
mirroring the MOVS/SCAS pattern (CX-counted while-loop, no ZF break since LODS
sets no flags), and added to the rep-aware dispatch list. Demonstrated by the
`rep_lodsb_*` / `rep_lodsw_*` tests in `cpu_lods.rs`; all REP LODS tests 0 PANIC.

**SST-D-015 — LOCK prefix (0xF0) not parsed by the decoder.**
Forms: any with an F0 prefix (e.g. `cs lock movsb` = 2E F0 A4). The decoder's
prefix loop (decode.rs:170-181) handled 26/2E/36/3E/F2/F3 but not F0, so a LOCK
prefix was treated as the opcode → DECODE_ERR. The 80C286 executes `lock movsb`
(LOCK ignored on string ops; exception: none). ~115 DECODE_ERR per byte-string
form, ~100-108 per word-string form. Classification: **emu86-bug** (decoder).
**RESOLVED 2026-08-16 (R1):** 0xF0 added to the prefix loop as parse-and-discard
(LOCK is a no-op in emu86's single-step model — no concurrency, so no bus
locking). Demonstrated by `lock_prefix_movsb_decodes_and_executes` in
`cpu_movs.rs`; all 10 string forms now 0 DECODE_ERR with `--all`.

**R1 status:** string forms fetched and pinned; bare + REP + segment-override +
LOCK-prefixed string ops now 100% PASS/PANIC/DECODE_ERR-free with `--all`. Scope
lift (Deferred → V1) + scoped un-filtering of the string family remains
(see R1 step 3-5 in `.opencode/plan.md`); `cargo test --locked --all-targets`
348 passed (incl. 9 new string-op unit tests); V1 sweep totals unchanged
(string forms remain Deferred scope).

### PANIC clusters (all `catch_unwind`-captured; reported, never aborts)

**SST-D-008 — stack-pointer wrap panics ("attempt to add with overflow").**
Forms: 07, 17, 1F, 58-5F, 8F, 9D, C2, C3, CA, CB, D7 — 7,762 PANIC.
`Machine::stack_pop_u16` (`addr.off.0 + 2`, machine.rs:75) overflows u16 when SP
is ≥ 0xFFFE; the 80C286 wraps SP mod 0x10000. Debug build panics on overflow;
this is the debug-mode arithmetic trap (release would wrap silently). Samples all
`pop`/`ret`/`popf`/`xlat` with near-top-of-stack SP. Classification:
**emu86-bug** (non-wrapping stack arithmetic; manifests as PANIC in debug builds).

**RESOLVED 2026-08-16 (F2):** stack arithmetic now wraps: `stack_push_u16`
`addr.off.0.wrapping_sub(2)` (machine.rs:64), `stack_pop_u16`
`addr.off.0.wrapping_add(2)` (machine.rs:75), XLAT offset
`addr_off.wrapping_add(idx)` (step.rs:279), RET/RETF immediate adjust
`SP.wrapping_add(adj)` (step.rs:329,340).

Note on verification: this defect only traps in **debug** builds (u16 overflow
panics); release already wraps silently, so a release-mode run exercises the
same values but cannot distinguish old vs new code. The fix is demonstrated by
the 4 new unit tests in `machine.rs` (run under `cargo test`, debug profile):
`stack_push_wraps_sp_at_zero` and `stack_pop_wraps_sp_at_max` trap with
"attempt to subtract/add with overflow" on the old code and pass now, with
round-trip memory-value assertions. These are synthetic primitive tests, not
data-file runs, so they are consistent with the speed policy (which forbids
running the *debug binary against the corpus*). Release-mode runs on all 18
affected forms (07 17 1F 58-5F 8F 9D C2 C3 CA CB D7) — 86,115 PASS aggregate,
0 FAIL, 0 PANIC (381 pre-existing exception-expected tests); `cargo test
--locked --all-targets` 319 passed. No FAILREPRO pin existed for this cluster.

**SST-D-009 — C1.x 16-bit shift count assertion.**
Forms: C1.0-C1.7 — 9,773 PANIC ("assertion failed: val as u8 as u16 == val",
step.rs:146). C1.x operands are `OPER_IMM8_EXT` (sign-extended imm8); for counts
≥ 0x80 the sign-extended u16 value fails `val as u8 as u16 == val` and asserts.
The 80C286 masks shift counts to 5 bits (count & 0x1F). Samples:
`shl word [ss:bp+di+6BDh],CBh` panics. Classification: **emu86-bug** (shift-count
assert on sign-extended imm8; counts ≥ 0x80 not masked to 5 bits).

**RESOLVED 2026-08-16 (F3):** `step.rs:145` now truncates the count to its low
byte (`Value::U16(val) => val as u8`) before passing it to `alu::shift`, which
already masks to 5 bits (`n & 0x1f`; `rotate_left` reduces mod width) matching
the 80C286. No C1.x PANIC remains (all 5 C1.x forms: 0 PANIC). The 9,773
former-PANIC tests now complete: 2,808 → PASS and 6,965 → FAIL. The residual
FAILs are *not* the D-009 bug — C1.4-C1.7 fail only on undefined-AF (and
count>1 OF) bits, which is the SST-D-002 harness-caveat (+5,552; D-002 total
now 58,582), and C1.0 fails on the missing ROL CF/OF update, which is the
SST-D-003 emu86-bug (+1,413; D-003 total now 16,885). Fix demonstrated by the
step-level mirror tests in `shift_semantics_test.rs` (pinned C1.4 sample
`shl word [ss:bp+di+6BDh],CBh`, count byte 0xCB sign-extends to 0xFFCB →
masked to 0x0B, result 0x8000 << 11 = 0x0000, machine advances without
trapping); `cargo test --locked --all-targets` 320 passed; hermetic micro lane
green after re-pinning C1.4 to bucket=FAIL (SST-D-002).

**SST-D-010 — IDIV "Divide Error" panic.** 723 PANIC, same root as SST-D-005
(unsigned divmod asserting quotient > 0xffff). Classification: **emu86-bug**.
RESOLVED 2026-08-16 (F8) with SST-D-005: the signed divmod only panics on a
genuine #DE (zero divisor / signed quotient overflow), which the corpus marks
SkipException; the 723 former panics were valid signed results and now PASS
(F7.7 PANIC=0).

**SST-D-011 — F7.3 NEG "attempt to negate with overflow".** 1 PANIC
(`alu::unary` NEG `-(a as i16)` overflows at i16::MIN). Debug-mode overflow trap.
Classification: **emu86-bug**.

**RESOLVED 2026-08-16 (F1):** `alu.rs:262` NEG now uses `(a as i16).wrapping_neg()`
(i16::MIN wraps to itself, matching the 80C286 result 0x8000). This defect only
traps in **debug** builds (release already wraps silently); the fix is
demonstrated by the `neg16_min_i16` / `neg8_min_i8` unit tests in `alu_test.rs`
(debug profile, synthetic — consistent with the speed policy), which trap on the
old code and pass now. Release-mode runs on the affected NEG forms also pass
(`F6.3` 3968/3968, `F7.3` 3939/3939, 0 FAIL, 0 PANIC; 28 pre-existing
exception-expected tests, all #GP #13, unchanged from baseline); hermetic micro
lane `F7.3.MOO` PASS; `cargo test --locked --all-targets` 315 passed. No
FAILREPRO pin existed for this cluster.

### Cluster size accounting

- FAIL **0** = no remaining FAIL clusters. Track 2 applied per-form
  `flags_umask` to every harness-caveat form, masking the architecturally-
  undefined bits (D-001 logical AF, D-002 shift AF/OF, D-004-undefined IMUL
  bits) per Intel 80286 docs. All emu86-bug clusters were already resolved by
  Track 1 (F1-F8 + D-012); the harness-caveat residuals are now masked.
  (History: the F7 sweep had FAIL 152,974 = harness-caveat 152,384 +
  emu86-bug 334 (D-005) + runner-bug 256 (D-012). F8 resolved D-005 → 152,640.
  D-012 resolved the runner false-FAILs → 152,384. Track 2 masked the
  remaining 152,384 harness-caveat FAILs → 0.)
- PANIC 0 = no remaining PANIC clusters
  (SST-D-010 resolved 2026-08-16 — signed IDIV, no longer counted; SST-D-008
  resolved 2026-08-16 — non-wrapping stack arithmetic, no longer counted;
  SST-D-011 resolved 2026-08-16 — debug-only trap; SST-D-009 resolved
  2026-08-16 — C1.x shift-count assert, no longer counted; SST-D-003 resolved
  2026-08-16 — ROL CF/OF, no longer counted; SST-D-006 resolved 2026-08-16 —
  XCHG memory EA, no longer counted; SST-D-007 resolved 2026-08-16 — multi-byte
  reads wrap the EA offset at 0x10000, no longer counted).
- Sum check: 0 = 0 ✓ ; PANIC 0 ✓.

## Coverage statement (honest)

**Covered by hardware evidence now (V1 conservative subset, 258 forms):** every
step-implemented, non-prefixed, non-exception family in the conservative scope —
ADD/OR/ADC/SBB/AND/SUB/XOR/CMP (all reg/mem/imm forms incl. grp1 80-83), INC/DEC,
PUSH/POP (all forms), PUSHF/POPF, XCHG, MOV (all forms), TEST, LEA, NOP, CBW/CWD,
far/near CALL/JMP/RET, Jcc/LOOP/JCXZ, flag ops (CLC/STC/CLD/STD/CLI/STI),
XLAT, shifts/rotates (SHL/SHR/SAR/ROL), grp3 TEST, MUL/IMUL/DIV/IDIV. For all of
these, the *defined* flag bits and register/memory results match hardware except
for the clusters above; **98.85% of executed tests pass outright and the entire
remaining surface (SKIP_EXCEPTION 11,640) is exception-expected tests the suite
itself skips** — all FAIL and PANIC clusters are resolved. Per-form `flags_umask`
now masks the architecturally-undefined bits (logical-op AF, shift AF/OF,
IMUL undefined bits) per Intel 80286 docs, so the conservative V1 subset
compares only defined bits. **The conservative V1 sweep is at 0 FAIL / 0 PANIC.**

**Planned-but-not-validated (deferred, per policy):** REP/string family
(MOVS/CMPS/STOS/LODS/SCAS/INS/OUTS), segment/operand/address/LOCK prefixes, IN/OUT
port I/O, INT/INTO/IRET/HLT, exception-expected tests (no exception machinery in
emu86), and decode-only/not-implemented forms (DAA/DAS/AAA/AAS, PUSHA/POPA,
BOUND, WAIT, LAHF/SAHF, ROR/RCL/RCR, 8-bit MUL/DIV/IMUL, CMC, AAM/AAD, ESC,
LES/LDS, ENTER/LEAVE). These have **no** hardware evidence in this ledger.

## Open items (out of scope here — potential fixes live here, not in this doc)

- No remaining FAIL/PANIC items in the conservative V1 sweep. The full
  divergence ledger is closed: every emu86-bug cluster (D-003..D-011) is
  resolved by Track 1 (F1-F8), the runner false-FAIL D-012 is resolved, and the
  harness-caveat residuals (D-001/D-002/D-004-undefined) are masked by per-form
  `flags_umask` (Track 2). The FAILREPRO registry is empty.
  (SST-D-008 stack wrap fixed 2026-08-16 — wrapping stack/XLAT/RET
  arithmetic; SST-D-011 NEG i16::MIN fixed 2026-08-16 — `wrapping_neg`;
  SST-D-009 C1.x shift-count assert fixed 2026-08-16 — count masked to low
  byte, 5-bit in `alu::shift`; SST-D-003 ROL CF/OF fixed 2026-08-16 — CF =
  rotated-out bit, OF for count==1; SST-D-006 XCHG memory-EA fixed 2026-08-16
  — EA computed pre-swap; SST-D-004 IMUL CF/OF fixed 2026-08-16 — sign-
  extension overflow check; SST-D-007 far-branch 64KB wrap fixed 2026-08-16 —
  multi-byte reads wrap the EA offset at 0x10000; SST-D-005/SST-D-010 IDIV
  signed division fixed 2026-08-16 — `DivideOp` signed arm, `OP_IDIV`;
  SST-D-012 short-branch IP off-by-one fixed 2026-08-16 — runner absent-IP
  `−1` convention, 256 false FAILs → PASS; SST-D-001 logical AF + SST-D-002
  shift AF/OF + SST-D-004-undefined IMUL bits masked 2026-08-16 — per-form
  `flags_umask`, Track 2, 152,384 harness-caveat FAILs → PASS.)
- Forward work: Track 3 (REP-vs-dosemu2 reconciliation) and Track 4 (Hydra
  Option C/D) remain, per `.opencode/plan.md`.

## Hermetic micro-corpus (checked-in)

**What this is:** a small, checked-in subset of the real Harris 80C286 captures
above, re-serialized with the vendored MOO writer and executed through the real
runner by the `just check` cargo-test lane (module `dis86/src/emu86/sst/micro.rs`,
3 tests). The lane is hermetic: paths resolve from `CARGO_MANIFEST_DIR`, it reads
only files committed in this repo, and it makes no network access and no use of
the gitignored 612MB `data/sst/full/` fetch.

**Location & provenance:** `dis86/data/sst/micro/` — one MOO file per family,
named after its source form. `spec.txt` records each entry's source form + index
in the pinned full data; upstream provenance/licensing is in `ATTRIBUTION.md`;
the per-file SHA-256 pins live in `data/sst/manifest.txt`. Regeneration is
out-of-band via `emu86_sst micro-extract data/sst/micro/spec.txt` (a build
helper; the checked-in files are what the lane runs).

**Counts (37 entries, ~22KB total):**

- **37 PASS-representative files** spanning the conservative breadth — ADD (04),
  OR (0C), ADC (14), SBB (1C), AND (24), SUB (2D), XOR (35), CMP (3D),
  INC (40), DEC (4F), PUSH r16 (50), POP r16 (58), MOV (89/8B/B8), XCHG (91),
  CBW/CWD (98/99), Jcc (74), JMP rel8 (EB), RET (C3), LOOP (E2), JCXZ (E3),
  MOV moffs8 write (A2), SHL r/m16,1 (D1.4) and its variable-count sibling
  (C1.4), grp3 TEST/NOT/NEG (F6.0/F6.2/F7.3), IDIV (F7.7), flag ops (F8/FC),
  PUSHF (9C). Every entry was verified with the release runner before inclusion;
  each must execute 100% PASS with **zero** FILTERED / SKIP_EXCEPTION / FAIL /
  DECODE_ERR / PANIC (entries are chosen clean: no leading prefix byte, no
  exception key). The logical-ops entries (0C/24/35/F6.0) and the shift entries
  (C1.4/D1.4) pass under their per-form `flags_umask` (AF masked for logicals
  and shifts; OF masked for variable-count shifts), so the lane now exercises
  the masked-undefined-bit path that Track 2 established.
- **0 FAILREPRO files** — the registry is empty. The last pin, C1.4 (SST-D-002,
  the shift undefined-AF residual), flipped to PASS on 2026-08-16 when Track 2
  masked AF/OF for the shift forms and was advanced to the PASS list. Earlier
  pins were advanced by their respective fixes: D1.0 (SST-D-003, ROL CF/OF),
  87 (SST-D-006, XCHG memory-EA), F7.5 (SST-D-004, IMUL CF/OF), FF.5 (SST-D-007,
  64KB offset wrap), F7.7 (SST-D-005/SST-D-010, signed IDIV).

**Expected-FAILREPRO contract:** with zero pins remaining, the lane now asserts
only that every micro file PASSES. If a future change introduces a divergence,
the `pass_micro_files_all_pass_cleanly` test will catch it. (The historical
contract is retained in `FAILREPRO.txt` for reference: a future pin would again
require (a) dropping the entry from `spec.txt`, (b) regenerating the corpus, and
(c) moving the entry from `FAILREPRO.txt` + the `micro.rs` expectations table
into the PASS list — never weakening the assertion.)

**Honest framing:** this corpus is *not* new coverage — it is the same
hardware-anchored evidence as the full P3 run above, pinned hermetically so
`just check` continuously guards the harness (and later, emu86 fixes). The
full-run numbers in the Totals table remain the authoritative coverage
statement.
