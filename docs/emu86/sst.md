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
| **PASS** | **827,177** (81.56% of executed) |
| **FAIL** | **157,081** (15.49%) |
| **DECODE_ERR** | **0** |
| **PANIC** | **18,259** (1.80%) |
| SKIP_EXCEPTION | 11,640 |
| SKIP_32BIT | 0 |
| FILTERED (prefix-collateral) | 147,841 |
| REVOKED (upstream revocation_list) | 2 |

Bucket-sum check: `PASS+FAIL+DECODE_ERR+PANIC+SKIP_EXCEPTION+SKIP_32BIT == executed`
holds; `executed+filtered+revoked == visited` holds.

Files with any FAIL/DECODE_ERR/PANIC: **95** (out of 258).
DECODE_ERR count: **0** across the whole run.

## Per-form breakdown (only forms with non-pass, non-filtered, non-skip-exception outcomes)

95 forms have FAIL and/or PANIC. Table columns: form | executed | PASS | outcomes.
Up to 3 samples per form (idx / name / first-16-of-sha1 / detail) from run output.

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
| `69` | 3986 | 216 | FAIL=3734 |
|   |   |   | idx=0 `imul ax,[ds:bx+di-7945h],F76h` `0a5d3b8908512d35…` flags exp=0x0893 act=0x08D7 umask=0x0FD7; FLAGS exp=0x0893 act=0x08D7 |
|   |   |   | idx=1 `imul si,[ss:bp+36h],F75h` `c40434206a432d0d…` flags exp=0x0C97 act=0x0C13 umask=0x0FD7; FLAGS exp=0x0C97 act=0x0C13 |
|   |   |   | idx=3 `imul si,[ss:bp+si-620Ah],58E5h` `18c2588b8db21f53…` flags exp=0x0813 act=0x0803 umask=0x0FD7; FLAGS exp=0x0813 act=0x0803 |
| `6B` | 3982 | 223 | FAIL=3723 |
|   |   |   | idx=0 `imul di,[ss:bp-39D0h],40h` `d10a7dd5501d53b5…` flags exp=0x0096 act=0x08D3 umask=0x0FD7; FLAGS exp=0x0096 act=0x08D3 |
|   |   |   | idx=1 `imul dx,si,FFD6h` `bfe2373b9e3c4347…` flags exp=0x0893 act=0x0887 umask=0x0FD7; FLAGS exp=0x0893 act=0x0887 |
|   |   |   | idx=2 `imul si,[ds:bx+si-51h],FFF4h` `ab64f9df2ab273e8…` flags exp=0x0C17 act=0x0C53 umask=0x0FD7; FLAGS exp=0x0C17 act=0x0C53 |
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
| `86` | 3970 | 3687 | FAIL=283 |
|   |   |   | idx=7 `xchg bh,[ds:bx+di-49h]` `92392837af0e0a21…` [0x1070AA] exp=0xFF act=0xDC |
|   |   |   | idx=36 `xchg bl,[ds:bx+di+13h]` `9b8728c8c26f214f…` [0xB7726] exp=0xBB act=0x00 |
|   |   |   | idx=37 `xchg bl,[ds:bx+si]` `15a0ff4a91183032…` [0x72314] exp=0xA5 act=0xBE |
| `87` | 3981 | 3357 | FAIL=595 |
|   |   |   | idx=0 `xchg di,[ds:di]` `eb65e3ae7e2c04e1…` [0xB4356] exp=0xA6 act=0x1E; [0xB4357] exp=0x8A act=0xA9 |
|   |   |   | idx=3 `xchg bx,[ds:bx+si-1884h]` `a8bc69f333e84105…` [0x40A8B] exp=0xD0 act=0x77; [0x40A8C] exp=0x65 act=0x2C |
|   |   |   | idx=12 `xchg bp,[ss:bp+AB7h]` `beb8dbc1e8ece614…` [0x10B67B] exp=0xD4 act=0x0F; [0x10B67C] exp=0xAB act=0xBC |
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
| `C0.0` | 3971 | 1240 | FAIL=2731 |
|   |   |   | idx=1 `rol byte [ds:bx+si-29h],25h` `42d704860f75f210…` flags exp=0x0C97 act=0x0496 umask=0x0FD7; FLAGS exp=0x0C97 act=0x0496 |
|   |   |   | idx=2 `rol bh,73h` `f654019178633dfb…` flags exp=0x0896 act=0x0097 umask=0x0FD7; FLAGS exp=0x0896 act=0x0097 |
|   |   |   | idx=3 `rol byte [ds:si-38h],C1h` `2bfeada58af4ee41…` flags exp=0x00C2 act=0x08C2 umask=0x0FD7; FLAGS exp=0x00C2 act=0x08C2 |
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
| `C1.0` | 3971 | 654 | FAIL=1336 PANIC=1953 |
|   |   |   | idx=1 `rol word [ss:bp+di],77h` `2f312bb94476919e…` flags exp=0x08D3 act=0x00D3 umask=0x0FD7; FLAGS exp=0x08D3 act=0x00D3 |
|   |   |   | idx=3 `rol word [ds:bx-6Eh],3Fh` `60243e6a3b9565fb…` flags exp=0x04D2 act=0x0CD3 umask=0x0FD7; FLAGS exp=0x04D2 act=0x0CD3 |
|   |   |   | idx=5 `rol word [ss:bp+di-1Dh],6` `88fdc30d48edb8a2…` flags exp=0x0056 act=0x0857 umask=0x0FD7; FLAGS exp=0x0056 act=0x0857 |
| `C1.4` | 3973 | 677 | FAIL=1314 PANIC=1955 |
|   |   |   | idx=1 `shl word [ss:bp+di-1Dh],6` `9b139f2e7435cfcc…` flags exp=0x0882 act=0x0892 umask=0x0FD7; FLAGS exp=0x0882 act=0x0892 |
|   |   |   | idx=2 `shl word [ss:bp+di+6BDh],CBh` `aa19e33e8c0c9692…` panic: assertion failed: val as u8 as u16 == val |
|   |   |   | idx=3 `shl word [ds:bx+si+7110h],Fh` `46a278ff7b99ca1e…` flags exp=0x0847 act=0x0857 umask=0x0FD7; FLAGS exp=0x0847 act=0x0857 |
| `C1.5` | 3974 | 697 | FAIL=1294 PANIC=1955 |
|   |   |   | idx=0 `shr word [ss:bp+di-1Dh],6` `a9598688620f50ab…` flags exp=0x0013 act=0x0813 umask=0x0FD7; FLAGS exp=0x0013 act=0x0813 |
|   |   |   | idx=1 `shr word [ss:bp+si-B3Bh],2Bh` `4901b41303ccd4e0…` flags exp=0x0013 act=0x0003 umask=0x0FD7; FLAGS exp=0x0013 act=0x0003 |
|   |   |   | idx=2 `shr word [ds:bx+si+7110h],Fh` `16a05454da0356f6…` flags exp=0x0012 act=0x0812 umask=0x0FD7; FLAGS exp=0x0012 act=0x0812 |
| `C1.6` | 3974 | 663 | FAIL=1328 PANIC=1955 |
|   |   |   | idx=0 `sal word [ss:bp+di+6BDh],CBh` `4f612338ff4c540c…` panic: assertion failed: val as u8 as u16 == val |
|   |   |   | idx=1 `sal word [ds:bx+si+7110h],Fh` `f0ec00c4b39a05a2…` flags exp=0x0046 act=0x0856 umask=0x0FD7; FLAGS exp=0x0046 act=0x0856 |
|   |   |   | idx=2 `sal word [ss:bp+si-B3Bh],2Bh` `cb3109c9a173cc24…` flags exp=0x0807 act=0x0007 umask=0x0FD7; FLAGS exp=0x0807 act=0x0007 |
| `C1.7` | 3974 | 696 | FAIL=1295 PANIC=1955 |
|   |   |   | idx=0 `sar word [ds:bx+si+7110h],Fh` `32175bcd3c54ddc9…` flags exp=0x0056 act=0x0856 umask=0x0FD7; FLAGS exp=0x0056 act=0x0856 |
|   |   |   | idx=1 `sar word [ss:bp+di+6BDh],CBh` `db2b42518a857da7…` panic: assertion failed: val as u8 as u16 == val |
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
| `D0.0` | 3955 | 987 | FAIL=2968 |
|   |   |   | idx=0 `rol dl,1` `cbfeb5b2b5150e49…` flags exp=0x0087 act=0x0886 umask=0x0FD7; FLAGS exp=0x0087 act=0x0886 |
|   |   |   | idx=3 `rol dl,1` `85c0beeb034baa32…` flags exp=0x0852 act=0x0853 umask=0x0FD7; FLAGS exp=0x0852 act=0x0853 |
|   |   |   | idx=4 `rol byte [ds:bx+si+7Fh],1` `b2c1dcdfbf76d5e6…` flags exp=0x0842 act=0x0043 umask=0x0FD7; FLAGS exp=0x0842 act=0x0043 |
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
| `D1.0` | 3955 | 934 | FAIL=2993 |
|   |   |   | idx=0 `rol word [ss:bp+di-40F5h],1` `f6b69bc8bdfed7bd…` flags exp=0x0842 act=0x0843 umask=0x0FD7; FLAGS exp=0x0842 act=0x0843 |
|   |   |   | idx=1 `rol word [ds:bx],1` `ca308e1507f984ea…` flags exp=0x0006 act=0x0807 umask=0x0FD7; FLAGS exp=0x0006 act=0x0807 |
|   |   |   | idx=4 `rol word [ss:bp+si+63h],1` `fbf3a9a89b936da0…` flags exp=0x00C2 act=0x08C2 umask=0x0FD7; FLAGS exp=0x00C2 act=0x08C2 |
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
| `D2.0` | 3957 | 1232 | FAIL=2725 |
|   |   |   | idx=0 `rol byte [ds:bx+si+5B6Fh],cl` `849ac602cadf7aa2…` flags exp=0x0856 act=0x0056 umask=0x0FD7; FLAGS exp=0x0856 act=0x0056 |
|   |   |   | idx=2 `rol byte [ds:49Ch],cl` `2a46974e98dae4e6…` flags exp=0x0803 act=0x0002 umask=0x0FD7; FLAGS exp=0x0803 act=0x0002 |
|   |   |   | idx=3 `rol byte [ds:si],cl` `921311549ac22620…` flags exp=0x04C3 act=0x0CC3 umask=0x0FD7; FLAGS exp=0x04C3 act=0x0CC3 |
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
| `D3.0` | 3955 | 1208 | FAIL=2719 |
|   |   |   | idx=1 `rol word [ds:bx],cl` `0da54c82fc9a08eb…` flags exp=0x04D7 act=0x04D6 umask=0x0FD7; FLAGS exp=0x04D7 act=0x04D6 |
|   |   |   | idx=4 `rol word [ds:si-1C4Dh],cl` `5c6e44a9c9e3d9ae…` flags exp=0x0453 act=0x0452 umask=0x0FD7; FLAGS exp=0x0453 act=0x0452 |
|   |   |   | idx=5 `rol ax,cl` `442f0af6490d4790…` flags exp=0x0CC3 act=0x04C3 umask=0x0FD7; FLAGS exp=0x0CC3 act=0x04C3 |
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
| `F7.5` | 3958 | 3763 | FAIL=167 |
|   |   |   | idx=13 `imul word [ds:bx-44FAh]` `6b0be02aa172b5cd…` flags exp=0x0096 act=0x0883 umask=0x0801; FLAGS exp=0x0096 act=0x0883 |
|   |   |   | idx=20 `imul bx` `e4707bccc333bf9e…` flags exp=0x0496 act=0x0C93 umask=0x0801; FLAGS exp=0x0496 act=0x0C93 |
|   |   |   | idx=37 `imul word [ds:di]` `349066fd49b53932…` flags exp=0x0496 act=0x0C93 umask=0x0801; FLAGS exp=0x0496 act=0x0C93 |
| `F7.7` | 3965 | 332 | FAIL=334 PANIC=723 |
|   |   |   | idx=7 `idiv word [ss:bp+di]` `b08f0b9dd03d3b7d…` AX exp=0xE5AA act=0x133E; DX exp=0x5DB7 act=0x2FB3 |
|   |   |   | idx=9 `idiv word [ds:si+3D3Bh]` `2bcb8be3ab9e9cb6…` panic: Divide Error |
|   |   |   | idx=10 `idiv word [ds:bx+di+59h]` `5e8c66d6508cec66…` panic: Divide Error |
| `FF.3` | 3968 | 2946 | FAIL=2 |
|   |   |   | idx=2916 `call far [ds:bx+di]` `0fa97fc1b8e6aceb…` CS exp=0x653A act=0x0000 |
|   |   |   | idx=3650 `call far [ss:bp+di]` `c52ea1ee7585a084…` CS exp=0x2EF7 act=0x0000 |
| `FF.5` | 3968 | 2943 | FAIL=2 |
|   |   |   | idx=2914 `jmp far [ds:bx+di]` `0921a4aa52083974…` CS exp=0x848F act=0x0000 |
|   |   |   | idx=3652 `jmp far [ss:bp+di]` `5d6cea81c9b44d85…` CS exp=0x5E79 act=0x0000 |

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
Forms: C0.4-C0.7, C1.4-C1.7 (FAIL part), D0.4-D0.7, D1.4-D1.7, D2.4-D2.7,
D3.4-D3.7 — 53,030 FAIL (47,799 for 8-bit/16-bit-by-1/CL forms + 5,231 C1.x FAIL).
Bit analysis: AF(4) appears everywhere; OF(11) appears for count>1 forms
(C0.x imm-count, D2.x/D3.x by-CL). Intel 80286: SHL/SHR/SAR define OF only for
count==1 and leave AF undefined; emu86's `update_flags_shl/shr/sar` correctly set
OF only for count==1 and never touch AF, so emu86's *defined* bits match — the
FAILs are entirely in undefined AF and (count>1) undefined OF. Classification:
**harness-caveat** (policy per-form umask cannot express "OF defined only when
count==1"; residual undefined-bit comparison).

**SST-D-003 — ROL: emu86 does not update CF/OF at all.**
Forms: C0.0, C1.0 (FAIL part), D0.0, D1.0, D2.0, D3.0 — 15,472 FAIL.
`alu::shift(ShiftOp::Rol)` contains `// TODO SET FLAGS ?` and leaves every flag
unchanged; the 80C286 sets CF (rotated-out bit) and, for count==1, OF. Samples
all show `CF(0)`/`OF(11)` diffs, e.g. `rol dl,1` exp=0x0087 act=0x0886. Intel
80286 defines CF for ROL (and OF for count==1), so this is a genuine emu86
deficiency. Classification: **emu86-bug** (ROL flag update missing).

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

**SST-D-006 — XCHG with a memory operand whose EA uses the exchanged register.**
Forms: 86 (283 FAIL), 87 (595 FAIL) — 878 FAIL total.
`OP_XCHG` reads both operands, then writes operand0 then operand1; the memory
operand's EA is re-derived at write time from the *already-updated* register.
For `xchg bh,[ds:bx+di-49h]`: init bx=0xFF99 → expected mem[0x1070AA]=0xFF; emu86
writes 0xFF to 0x104DAA (computed with the post-swap bx=0xDC99). Verified via
`--check-writes` (unexpected write `[0x104DAA] 0->255`). Classification:
**emu86-bug** (XCHG store to re-derived EA).

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

### PANIC clusters (all `catch_unwind`-captured; reported, never aborts)

**SST-D-008 — stack-pointer wrap panics ("attempt to add with overflow").**
Forms: 07, 17, 1F, 58-5F, 8F, 9D, C2, C3, CA, CB, D7 — 7,762 PANIC.
`Machine::stack_pop_u16` (`addr.off.0 + 2`, machine.rs:75) overflows u16 when SP
is ≥ 0xFFFE; the 80C286 wraps SP mod 0x10000. Debug build panics on overflow;
this is the debug-mode arithmetic trap (release would wrap silently). Samples all
`pop`/`ret`/`popf`/`xlat` with near-top-of-stack SP. Classification:
**emu86-bug** (non-wrapping stack arithmetic; manifests as PANIC in debug builds).

**SST-D-009 — C1.x 16-bit shift count assertion.**
Forms: C1.0-C1.7 — 9,773 PANIC ("assertion failed: val as u8 as u16 == val",
step.rs:146). C1.x operands are `OPER_IMM8_EXT` (sign-extended imm8); for counts
≥ 0x80 the sign-extended u16 value fails `val as u8 as u16 == val` and asserts.
The 80C286 masks shift counts to 5 bits (count & 0x1F). Samples:
`shl word [ss:bp+di+6BDh],CBh` panics. Classification: **emu86-bug** (shift-count
assert on sign-extended imm8; counts ≥ 0x80 not masked to 5 bits).

**SST-D-010 — IDIV "Divide Error" panic.** 723 PANIC, same root as SST-D-005
(unsigned divmod asserting quotient > 0xffff). Classification: **emu86-bug**.

**SST-D-011 — F7.3 NEG "attempt to negate with overflow".** 1 PANIC
(`alu::unary` NEG `-(a as i16)` overflows at i16::MIN). Debug-mode overflow trap.
Classification: **emu86-bug**.

### Cluster size accounting

- FAIL 157,081 = harness-caveat (139,886: SST-D-001 79,739 + SST-D-002 53,030 +
  SST-D-004-undefined part 7,117) + emu86-bug (17,195: SST-D-003 15,472 +
  SST-D-004-CF/OF 507 + SST-D-005 334 + SST-D-006 878 + SST-D-007 4).
- PANIC 18,259 = SST-D-008 7,762 + SST-D-009 9,773 + SST-D-010 723 + SST-D-011 1.
- Sum check: 139,886 + 17,195 = 157,081 ✓ ; 18,259 ✓.

## Coverage statement (honest)

**Covered by hardware evidence now (V1 conservative subset, 258 forms):** every
step-implemented, non-prefixed, non-exception family in the conservative scope —
ADD/OR/ADC/SBB/AND/SUB/XOR/CMP (all reg/mem/imm forms incl. grp1 80-83), INC/DEC,
PUSH/POP (all forms), PUSHF/POPF, XCHG, MOV (all forms), TEST, LEA, NOP, CBW/CWD,
far/near CALL/JMP/RET, Jcc/LOOP/JCXZ, flag ops (CLC/STC/CLD/STD/CLI/STI),
XLAT, shifts/rotates (SHL/SHR/SAR/ROL), grp3 TEST, MUL/IMUL/DIV/IDIV. For all of
these, the *defined* flag bits and register/memory results match hardware except
for the clusters above; 81.56% of executed tests pass outright and the FAIL
surface decomposes into (a) architecturally-undefined bits (harness-caveat),
(b) the listed emu86-bug clusters.

**Planned-but-not-validated (deferred, per policy):** REP/string family
(MOVS/CMPS/STOS/LODS/SCAS/INS/OUTS), segment/operand/address/LOCK prefixes, IN/OUT
port I/O, INT/INTO/IRET/HLT, exception-expected tests (no exception machinery in
emu86), and decode-only/not-implemented forms (DAA/DAS/AAA/AAS, PUSHA/POPA,
BOUND, WAIT, LAHF/SAHF, ROR/RCL/RCR, 8-bit MUL/DIV/IMUL, CMC, AAM/AAD, ESC,
LES/LDS, ENTER/LEAVE). These have **no** hardware evidence in this ledger.

## Open items (out of scope here — potential fixes live here, not in this doc)

- Policy `flags_umask` gap: logical-op forms (SST-D-001) and shift forms
  (SST-D-002) compare Intel-undefined AF (and OF for count>1); per-form umasks or
  a "mask AF for logicals/shifts" rule would turn ~133K FAILs into PASS without
  any emu86 change.
- ROL flag update (SST-D-003): emulate CF/OF for ROL (count==1 OF).
- IMUL CF/OF (SST-D-004): signed-overflow flag vs Harris.
- IDIV signed division (SST-D-005/SST-D-010): replace unsigned divmod path with a
  signed one; hardware-anchored expected flags.
- XCHG memory EA (SST-D-006): compute EA once before writing the register operand.
- Far indirect CALL/JMP CS load (SST-D-007): wrap the EA offset at 0x10000 for
  multi-byte reads (fixes the far-pointer boundary case).
- Stack-pointer wrap arithmetic (SST-D-008) and shift-count masking for C1.x
  (SST-D-009) so debug builds do not trap; NEG at i16::MIN (SST-D-011).

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

- **31 PASS-representative files** spanning the conservative breadth — ADD (04),
  OR (0C), ADC (14), SBB (1C), AND (24), SUB (2D), XOR (35), CMP (3D),
  INC (40), DEC (4F), PUSH r16 (50), POP r16 (58), MOV (89/8B/B8), XCHG (91),
  CBW/CWD (98/99), Jcc (74), JMP rel8 (EB), RET (C3), LOOP (E2), JCXZ (E3),
  MOV moffs8 write (A2), SHL r/m16,1 (D1.4), grp3 TEST/NOT/NEG (F6.0/F6.2/F7.3),
  flag ops (F8/FC), PUSHF (9C). Every entry was verified with the release
  runner before inclusion; each must execute 100% PASS with **zero** FILTERED /
  SKIP_EXCEPTION / FAIL / DECODE_ERR / PANIC (entries are chosen clean: no
  leading prefix byte, no exception key). Note the logical-ops entries (0C/24/35/
  F6.0) are the init-AF=0 subset that does not trip the SST-D-001 undefined-AF
  caveat; the shift entry (D1.4) is an init-AF=0 case outside the SST-D-002 noise.
- **6 FAILREPRO files**, one per pinned emu86-bug cluster, each expected to
  *diverge*: ROL flags (D1.0, SST-D-003), IMUL CF/OF (F7.5, SST-D-004),
  IDIV-as-unsigned (F7.7, SST-D-005), XCHG memory-EA recompute (87, SST-D-006),
  far-branch 64KB offset wrap (FF.5, SST-D-007), and the C1.x shift-count assert
  panic (C1.4, SST-D-009). Their SHA1s, cluster IDs, and exact recorded
  divergences are listed in `micro/FAILREPRO.txt` and asserted byte-for-byte in
  the `micro.rs` expectations table.

**Expected-FAILREPRO contract:** these six entries are regression pins for
*known* emu86 behavior, not tests to make green. When a future emu86 fix flips
one of them to PASS, the lane **fails**; the fix's author then (a) drops the
entry from `spec.txt`, (b) regenerates the corpus, and (c) moves the entry from
`FAILREPRO.txt` + the `micro.rs` expectations table into the PASS list. Do not
weaken the lane assertion instead — that would hide a real semantic change.

**Honest framing:** this corpus is *not* new coverage — it is the same
hardware-anchored evidence as the full P3 run above, pinned hermetically so
`just check` continuously guards the harness (and later, emu86 fixes). The
full-run numbers in the Totals table remain the authoritative coverage
statement.
