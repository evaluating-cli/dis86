# Phase 1 Implementation: `simx86` Core Hook & Low-Memory Export Architecture

**Status:** Reviewed architecture blueprint — implementation pending
**Target:** dosemu2 `devel` verified at `604ce0cdd1a71f657e2a2df623d216d5ab289313` and current `dis86` validator  
**Scope:** 16-bit real-mode MZ executables; one concurrent validator instance

> **Documentation-only scope:** No dosemu2 core patch or complete Rust adapter implementation is included in this PR. The examples below are required implementation sketches, not code that has been compiled or integration-tested.

---

## 1. Objective

Phase 1 adds the minimum core instrumentation needed to use dosemu2 as the differential-validation backend for `emu86`:

- request/apply and publish/ack hooks around `FindExecCode()` execution;
- bidirectional 14-register synchronization through `/dev/shm/hydra_remote`;
- exact target-entry gating;
- a real zero-copy `/dev/shm/dosemu_mem` view of dosemu2 low memory;
- validator-side normalization of the few places where a `simx86` node does not equal one `emu86::Machine::step()`.

This document supersedes the original Phase 1 draft. In particular it does **not** rename the generic POSIX-SHM allocator, does **not** use `EXCP_EMULEAVE` for validator jumps, and does **not** claim `G->seqlen == 1`.

### 1.1 Draft exit criteria

Keep the architecture PR in draft until both of the following implementation branches exist and are linked from its description:

- **emu86/dis86:** implement CMPS, configurable PSP loading, the corrected `DosemuProcess` launch, initial-state normalization, and boundary-driven normalized stepping.
- **dosemu2:** implement the page-sized control mapping, shared-memory core hook, requested-executable/PSP capture, low-memory export, and node-boundary reporting.

These branches provide implementation feedback for the contract. Their existence is the draft exit criterion; completing the verification checklist remains the Phase 1 completion criterion.

---

## 2. Source facts the implementation must preserve

### 2.1 `FindExecCode()` owns the authoritative running `PC`

`Interp86()` passes a linear `PC` into `FindExecCode()`. After each compiled node:

```c
PC = DoExec(G);
```

`TheCPU.eip` is synchronized when `FindExecCode()` returns. Therefore:

- externally supplied `CS:IP` must update the local `PC` **before** node lookup;
- post-step `IP` must be published as `PC - LONG_CS`.

### 2.2 `MSSTP` is necessary but not a universal one-instruction boundary

`FindExecCode()` clears `MSSTP|MTRAP` at the top of its loop. Validator mode must reassert `MSSTP` after that reset.

`_Interp86()` stops after one outer `InterpOne()` when `MSSTP` is set, but current `InterpOne()` deliberately has two boundary exceptions:

- REP string operations become one-iteration loop nodes under `MSSTP`;
- `POP SS`, `MOV SS`, and non-trap `STI` may recursively compile the following instruction into the same node to preserve interrupt inhibition.

`TNode::seqlen` is byte length and must never be treated as instruction count.

### 2.3 `EXCP_EMULEAVE` is the wrong redirection primitive

Current dosemu2 handles `EXCP_EMULEAVE` by calling `instr_sim_leave()`. It leaves instruction-simulation mode; it is not a clean local dispatcher restart.

Normal validator redirection therefore updates register state and returns a recomputed `PC` without setting `TheCPU.err`.

### 2.4 Full-sim low memory is anonymous by default

Current mapping driver order tries `softmmu` first. `open_mapping_softmmu()` succeeds for `EMU_FULLSIM()`, and `alloc_mapping_softmmu()` calls `alloc_tail(-1, ...)`, producing anonymous memory with no reopenable fd/path.

True zero-copy export therefore requires validator mode to select the existing POSIX-SHM mapping driver (`mapshm`) before low memory is allocated.

---

## 3. Shared control block

Reuse the existing Phase 0 ABI from `hydra/src/remote/shmdata.h` and `dis86/src/emu86/validator/shmdata.rs`:

```c
struct shmdata {
    u32 init;
    u32 end;
    u32 pid;
    u32 reserved0;
    u64 req;
    u64 ack;

    u16 ax, bx, cx, dx;
    u16 si, di, bp, sp;
    u16 ip, cs, ds, es, ss, flags;
};
```

Required invariants remain:

```c
_Static_assert(offsetof(shmdata_t, req) == 16, "req ABI");
_Static_assert(offsetof(shmdata_t, ack) == 24, "ack ABI");
_Static_assert(offsetof(shmdata_t, ax)  == 32, "register ABI");
_Static_assert(sizeof(shmdata_t) == 64, "shmdata ABI size");
```

The control atomics order the ordinary register payload:

- validator writes payload, then `store_release(req)`;
- dosemu2 `load_acquire(req)`, then reads payload;
- dosemu2 writes payload, then `store_release(ack)`;
- validator `load_acquire(ack)`, then reads payload.

---

## 4. dosemu2 shared-control lifecycle

Create `/dev/shm/hydra_remote` during validator-mode initialization, but keep `init == 0` until the target executable reaches its exact entry point.

Illustrative initialization:

```c
static shmdata_t *g_shm;
static bool g_validator_enabled;
static bool g_lockstep_active;

static int dosemu_hydra_init(void)
{
    int fd;
    long page_size;
    size_t map_len;

    if (!getenv("DIIS_DOSEMU_VALIDATOR"))
        return 0;

    fd = open("/dev/shm/hydra_remote",
              O_RDWR | O_CREAT | O_EXCL, 0600);
    if (fd < 0)
        return -1;
    page_size = sysconf(_SC_PAGESIZE);
    if (page_size <= 0) {
        close(fd);
        return -1;
    }
    map_len = (sizeof(*g_shm) + (size_t)page_size - 1) &
              ~((size_t)page_size - 1);
    if (ftruncate(fd, map_len) < 0) {
        close(fd);
        return -1;
    }

    g_shm = mmap(NULL, map_len, PROT_READ | PROT_WRITE,
                 MAP_SHARED, fd, 0);
    close(fd);
    if (g_shm == MAP_FAILED) {
        g_shm = NULL;
        return -1;
    }

    memset(g_shm, 0, sizeof(*g_shm));
    g_shm->pid = getpid();
    g_validator_enabled = true;
    return 0;
}
```

The page-rounded `map_len` is part of the existing Rust `ShmData::attach()` contract (normally 4096 bytes); truncating the file to the 64-byte ABI structure makes attachment fail before `mmap()`. The parent `DosemuProcess` already removes stale paths before spawning. `O_EXCL` prevents a live second validator from silently truncating an existing control mapping.

---

## 5. Exact MZ entry gate

### 5.1 Metadata supplied by `DosemuProcess`

`DosemuProcess` already receives `exe_path`. Parse the same MZ header used by `Emulator::new()` and pass the relative entry coordinates plus a canonical executable identity to dosemu2:

```text
DIIS_DOSEMU_MZ_CS=<header.cs>
DIIS_DOSEMU_MZ_IP=<header.ip>
DIIS_DOSEMU_EXE=<canonical target path or nonce bound to this exec>
```

Do not pass a guessed absolute load segment.

### 5.2 Exec-bound runtime PSP gate

The current `emu86` MZ loader establishes:

```text
image load segment = PSP + 0x10
CS = PSP + 0x10 + header.cs
IP = header.ip
DS = ES = PSP
```

Instrument the DOS exec/load path to compare the program being successfully loaded with `DIIS_DOSEMU_EXE` (canonicalized under DOS path semantics), and record the PSP allocated for that exec in `g_target_psp`. A PSP signature plus relative entry coordinates alone is not executable identity. Use the recorded value in the hook:

```c
static bool at_target_entry(unsigned int pc)
{
    uint16_t psp;
    uint16_t expected_cs;
    uint16_t ip;

    if (PROTMODE())
        return false;
    if (!g_target_exec_seen)
        return false;

    psp = g_target_psp;
    if (TheCPU.ds != psp || TheCPU.es != psp)
        return false;

    /* Standard PSP begins with INT 20h: bytes cd 20. */
    if (READ_WORD((dosaddr_t)psp << 4) != 0x20cd)
        return false;

    expected_cs = (uint16_t)(psp + 0x10u + g_target_mz_cs);
    ip = (uint16_t)(pc - LONG_CS);

    return TheCPU.cs == expected_cs && ip == g_target_mz_ip;
}
```

Before the target exec is observed or this predicate matches, the hook must not wait for `req`; normal DOS startup continues. Clear the captured PSP when that DOS process exits. On the first match:

1. publish the initial CPU snapshot using the current `pc`;
2. `store_release(init, 1)`;
3. enter request-wait state.

This replaces both the inherited hard-coded `0x823:0000` assumption and the unsafe `CS >= load_seg` predicate.

### 5.3 Align emu86 before request 1

The validator treats the initial snapshot as a setup handshake, not as a completed step. Before sending request 1 it must:

1. instantiate/rebase emu86 at the captured runtime PSP rather than the current fixed `PSP_SEGMENT = 0x813`;
2. rebuild the PSP and load the image at `PSP + 0x10`, applying MZ relocations with that load segment, and establish matching CS:IP and SS:SP;
3. perform the existing Hydra `post_init_state()` normalization (AX through BP and FLAGS) against the published dosemu state, then write or confirm the normalized initial state on both sides; and
4. verify the complete compared register set and load-segment-dependent image/PSP memory before either emulator executes an instruction.

Publishing dosemu's DOS-provided registers alone is insufficient: emu86 starts general registers at zero, FLAGS at IF, and currently uses a fixed PSP. Any unresolved initial mismatch must fail initialization, not be reported as an instruction divergence.

---

## 6. Register serialization

### 6.1 Publish (`TheCPU` -> shared memory)

```c
static void publish_cpu(unsigned int pc)
{
    g_shm->ax = (uint16_t)TheCPU.eax;
    g_shm->bx = (uint16_t)TheCPU.ebx;
    g_shm->cx = (uint16_t)TheCPU.ecx;
    g_shm->dx = (uint16_t)TheCPU.edx;
    g_shm->si = (uint16_t)TheCPU.esi;
    g_shm->di = (uint16_t)TheCPU.edi;
    g_shm->bp = (uint16_t)TheCPU.ebp;
    g_shm->sp = (uint16_t)TheCPU.esp;
    g_shm->ip = (uint16_t)(pc - LONG_CS);
    g_shm->cs = TheCPU.cs;
    g_shm->ds = TheCPU.ds;
    g_shm->es = TheCPU.es;
    g_shm->ss = TheCPU.ss;
    g_shm->flags = (uint16_t)TheCPU.eflags;
}
```

Never use `TheCPU.eip` as the post-`DoExec()` source of truth inside `FindExecCode()`.

### 6.2 Apply (shared memory -> `TheCPU`)

```c
static unsigned int apply_cpu(void)
{
    assert(!PROTMODE());

    TheCPU.eax = (TheCPU.eax & 0xffff0000u) | g_shm->ax;
    TheCPU.ebx = (TheCPU.ebx & 0xffff0000u) | g_shm->bx;
    TheCPU.ecx = (TheCPU.ecx & 0xffff0000u) | g_shm->cx;
    TheCPU.edx = (TheCPU.edx & 0xffff0000u) | g_shm->dx;
    TheCPU.esi = (TheCPU.esi & 0xffff0000u) | g_shm->si;
    TheCPU.edi = (TheCPU.edi & 0xffff0000u) | g_shm->di;
    TheCPU.ebp = (TheCPU.ebp & 0xffff0000u) | g_shm->bp;
    TheCPU.esp = (TheCPU.esp & 0xffff0000u) | g_shm->sp;
    TheCPU.eip = g_shm->ip;
    TheCPU.eflags = (TheCPU.eflags & 0xffff0000u) | g_shm->flags;

    if (TheCPU.cs != g_shm->cs) SetSegReal(g_shm->cs, Ofs_CS);
    if (TheCPU.ds != g_shm->ds) SetSegReal(g_shm->ds, Ofs_DS);
    if (TheCPU.es != g_shm->es) SetSegReal(g_shm->es, Ofs_ES);
    if (TheCPU.ss != g_shm->ss) SetSegReal(g_shm->ss, Ofs_SS);

    return LONG_CS + TheCPU.eip;
}
```

A protected-mode request is a Phase 1 contract violation. Do not silently update selectors while skipping descriptor-cache updates.

---

## 7. Split request/execute/ack hook

The integration is split around node execution rather than implemented as a single state-machine call at loop top.

Illustrative `FindExecCode()` structure:

```c
while (1) {
    uint64_t hydra_req = 0;

#if USE_HYDRA
    if (g_validator_enabled) {
        if (!g_lockstep_active) {
            if (at_target_entry(PC)) {
                publish_cpu(PC);
                __atomic_store_n(&g_shm->init, 1, __ATOMIC_RELEASE);
                g_lockstep_active = true;
            } else {
                goto normal_dispatch;
            }
        }

        if (!dosemu_hydra_wait_request(&hydra_req))
            return PC; /* end: execute nothing further */

        PC = apply_cpu();
    }
#endif

normal_dispatch:
    TheCPU.mode &= ~(MSSTP | MTRAP);
    if (EFLAGS & TF)
        TheCPU.mode |= MSSTP | MTRAP;
#if USE_HYDRA
    if (g_lockstep_active)
        TheCPU.mode |= MSSTP;
#endif

    /* Existing FindTree() / _Interp86() path. */
    ...

    PC = DoExec(G);

#if USE_HYDRA
    if (g_lockstep_active) {
        publish_cpu(PC);
        __atomic_store_n(&g_shm->ack, hydra_req, __ATOMIC_RELEASE);
    }
#endif

    if (TheCPU.err)
        return PC;
}
```

`dosemu_hydra_wait_request()` performs:

```c
for (;;) {
    if (__atomic_load_n(&g_shm->end, __ATOMIC_ACQUIRE))
        return false;

    uint64_t req = __atomic_load_n(&g_shm->req, __ATOMIC_ACQUIRE);
    uint64_t ack = __atomic_load_n(&g_shm->ack, __ATOMIC_RELAXED);
    if (req != ack) {
        *out_req = req;
        return true;
    }

    /* spin/yield policy may be tuned; semantics must not change */
}
```

No `EXCP_EMULEAVE` is used for normal `CS:IP` changes.

---

## 8. Zero-copy low-memory export

### 8.1 Why the original `do_open_pshm()` edit is invalid

`do_open_pshm()` is the generic object factory used by the POSIX-SHM mapping driver. Replacing its PID-specific name with `/dosemu_mem` causes unrelated mapping allocations to reopen/truncate the same object.

It must remain unchanged.

### 8.2 Force the POSIX-SHM backend in validator mode

Current full-sim dosemu2 otherwise selects `softmmu`, whose `alloc_mapping_softmmu()` uses anonymous memory.

Dosemu2's configuration script reads the `$_mapping` variable and passes its value to the `mappingdriver` command. The documented `-I string` interface supplies additional configuration statements on the command line. `DosemuProcess::spawn()` should therefore select the existing POSIX-SHM backend through that verified configuration interface rather than relying on an inferred environment-variable spelling:

```rust
Command::new(&dosemu_bin)
    .args(["-I", "$_mapping = \"mapshm\""])
    .env("DIIS_DOSEMU_VALIDATOR", "1")
    // entry metadata ...
```

Equivalent configuration-file injection of `$_mapping = "mapshm"` is also valid. `mapshm` is the current mapping-driver key for the POSIX-SHM backend. The implementation must verify from dosemu2's resulting runtime configuration that `mappingdriver mapshm` was selected before low-memory allocation.

### 8.3 Special-case only `MAPPING_LOWMEM`

Add a dedicated object creator in `mapfile.c`:

```c
#ifdef HAVE_SHM_OPEN
static int do_open_validator_lowmem(size_t mapsize)
{
    const char *name = "/dosemu_mem";
    int fd;

    fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, S_IRUSR | S_IWUSR);
    if (fd < 0)
        return -1;
    if (ftruncate(fd, mapsize) < 0) {
        close(fd);
        shm_unlink(name);
        return -1;
    }
    return fd;
}
#endif
```

Then change only the fd selection in `alloc_mapping_file()`:

```c
static void *alloc_mapping_file(int cap, size_t mapsize, void *target)
{
    int prot = PROT_READ | PROT_WRITE;
    int fd, rc;

#ifdef HAVE_SHM_OPEN
    if (getenv("DIIS_DOSEMU_VALIDATOR") && (cap & MAPPING_LOWMEM)) {
        fd = do_open_validator_lowmem(mapsize);
        if (fd < 0)
            return MAP_FAILED;
        /* already truncated by the dedicated creator */
        return alloc_tail(fd, mapsize, prot, target);
    }
#endif

    fd = mfops->open();
    if (fd < 0)
        return MAP_FAILED;
    rc = ftruncate(fd, mapsize);
    assert(rc != -1);
    return alloc_tail(fd, mapsize, prot, target);
}
```

Consequences:

- the `MAPPING_LOWMEM` object mapped at `lowmem_base` is the same fd-backed object exposed as `/dev/shm/dosemu_mem`;
- `ShmMem::attach()` maps those same pages with `MAP_SHARED`;
- EMS/XMS/DPMI/VGA and all other mappings continue through `mfops->open()` and retain the existing PID-specific/unlinked behavior;
- `O_EXCL` prevents accidental truncation of another live validator's low memory.

On mapping shutdown, unlink `/dosemu_mem` when validator mode is active. Existing mappings in the validator remain valid until unmapped.

The fixed Phase 0 pathname means Phase 1 supports one concurrent validator instance. A PID-scoped negotiated name is a later ABI revision.

---

## 9. `DosemuProcess` launch corrections

The Phase 0 adapter inherited these DOSBox-X-only arguments:

```text
-hydra <libhydraremote.so>
-hydra-conf normal
```

Current upstream dosemu2 has no such options. Remove them.

The patched dosemu2 core owns the new hook directly. The launch becomes conceptually:

```rust
let mut cmd = Command::new(&dosemu_bin);
cmd.args(["-dumb", "-quiet", "-K", test_dir, "-E", test_exe]);
cmd.args(["-I", "$_mapping = \"mapshm\""]);
cmd.env("DIIS_DOSEMU_VALIDATOR", "1");
cmd.env("DIIS_DOSEMU_MZ_CS", format!("{}", mz.hdr.cs));
cmd.env("DIIS_DOSEMU_MZ_IP", format!("{}", mz.hdr.ip));
cmd.env("DIIS_DOSEMU_EXE", canonical_target_path);
```

`$_mapping = "mapshm"` uses dosemu2's verified configuration interface. The `DIIS_DOSEMU_*` variables are implementation-local metadata for the new validator hook; their exact names may change, but their semantics are required.

---

## 10. Normalizing `simx86` node steps to `emu86` steps

### 10.1 Add a raw dosemu request primitive

Split the Rust adapter internally:

```text
raw_step(): exactly one req/ack exchange
step(): normalized validator step
```

### 10.2 REP string instructions

`simx86` with `MSSTP` deliberately exposes REP iterations individually; `emu86` currently loops through the entire REP inside one `Machine::step()`.

Before a normalized dosemu step, decode the instruction at the current shared-memory `CS:IP`. If it is a REP-prefixed string instruction:

```text
start = CS:IP
repeat raw_step()
until published CS:IP != start
comparison_span = 1 decoded instruction
```

This makes `DosemuProcess::step()` match one `emu86` REP step without changing either CPU implementation's internal behavior.

CMPS is a Phase 1 implementation prerequisite, not merely a corpus entry. Add `OP_CMPS` to `Machine::step()`'s REP-aware special-operation dispatch and implement byte/word comparison of `DS:(E)SI` against `ES:(E)DI`, DF-controlled index updates, CX decrement, subtraction flags, and REPE/REPNE termination. Add focused zero-count, forward/backward, equal/mismatch, byte/word, REPE, and REPNE tests before enabling CMPS lockstep cases.

### 10.3 `STI`, `MOV SS`, and `POP SS`

Current `simx86` may compile the following instruction into the same node for the interrupt shadow, but trap-mode `STI` can consume only STI. Determine the actual node boundary from simx86 metadata plus the authoritative returned PC (decoding from the starting address to that boundary as needed); never set `comparison_span = 2` solely because the first opcode is STI/MOV SS/POP SS.

If the measured boundary ends after the first iteration of a REP-prefixed second instruction, first record that the shadow instruction was consumed, then issue further raw steps while the published `CS:IP` remains at that REP address. Only after it advances may the normalized step return. This handles both variable shadow spans and the shadow-plus-REP composition.

Extend the backend abstraction with a small post-step span query, defaulting to one. The validator then performs:

```text
do_backend.step()
for _ in 0..do_backend.comparison_span():
    emu86.step()
compare states
```

This preserves simx86's interrupt-shadow implementation and makes the comparison boundary explicit instead of silently mismatching the two emulators.

The integration tests must include REP and each interrupt-shadow instruction so this adapter cannot regress unnoticed.

---

## 11. Shutdown behavior

The `end` flag is an execution stop barrier:

- dosemu2 observes it with acquire ordering while waiting for a request;
- `FindExecCode()` returns immediately without executing another node;
- `DosemuProcess::Drop` remains responsible for terminating/reaping the child.

Do not set `EXCP_EMULEAVE` and do not let the hook simply return to normal guest execution after observing `end`.

---

## 12. Verification checklist

Before Phase 1 can be called implemented, verify all of the following against a patched dosemu2 build:

- [ ] exact MZ entry gate fires once, at the target program and not DOS startup;
- [ ] the entry belongs to the requested executable's captured DOS exec/PSP, not a helper with the same relative entry;
- [ ] emu86 is rebased to the runtime PSP and both initial states are normalized before request 1;
- [ ] `init` is release-published only after the initial target state is complete;
- [ ] one normal request produces the expected next decoded-instruction boundary;
- [ ] post-step `IP` comes from returned `PC`;
- [ ] external `CS:IP` rewrite changes local `PC` before node lookup;
- [ ] no normal redirection path sets `EXCP_EMULEAVE`;
- [ ] high EFLAGS bits survive state import;
- [ ] real-mode segment writes call `SetSegReal()`;
- [ ] protected-mode segment mutation is rejected;
- [ ] REP string instructions normalize correctly;
- [ ] `STI`, `MOV SS`, and `POP SS` report the measured one- or two-instruction span, including trap-mode STI;
- [ ] a combined interrupt-shadow instruction followed by REP coalesces all remaining REP iterations;
- [ ] CMPS and REP/REPE/REPNE CMPS are implemented in emu86 and covered by focused tests;
- [ ] `end` executes zero additional guest instructions;
- [ ] validator mode selects `mapshm` through the verified `$_mapping` configuration path rather than anonymous `softmmu`;
- [ ] `/dev/shm/dosemu_mem` and `lowmem_base` observe identical writes in both directions;
- [ ] a second live validator cannot truncate the first mapping;
- [ ] generic mapping allocations remain distinct from `/dosemu_mem`;
- [ ] dosemu2 launch contains no DOSBox-X `-hydra` arguments.

---

## 13. Source locations verified for this blueprint

Dosemu2 `devel` at `604ce0cdd1a71f657e2a2df623d216d5ab289313`:

- `src/base/emu-i386/simx86/interp.c`
- `src/base/emu-i386/simx86/codegen.h`
- `src/base/emu-i386/simx86/cpu-emu.c`
- `src/base/emu-i386/simx86/protmode.c`
- `src/base/lib/mapping/mapping.c`
- `src/base/lib/mapping/mapfile.c`
- `src/base/lib/mapping/mapping.h`
- `etc/global.conf`

Current dis86:

- `dis86/src/emu86/loader.rs`
- `dis86/src/emu86/step.rs`
- `dis86/src/emu86/cpu_movs.rs`
- `dis86/src/emu86/validator/dosemu_process.rs`
- `dis86/src/emu86/validator/run.rs`
- `dis86/src/emu86/validator/shmmem.rs`
- `hydra/src/remote/shmdata.h`
