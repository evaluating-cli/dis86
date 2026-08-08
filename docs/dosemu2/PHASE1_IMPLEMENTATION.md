# Phase 1 Implementation: `simx86` Core Hook & Low-Memory Export Architecture

**Status:** Implementation Specification & Integration Blueprint  
**Target:** `dosemu2` (`src/base/emu-i386/simx86/`, `src/base/lib/mapping/`), `dis86` (`dis86/src/emu86/validator/`)  

---

## 1. Objective

This document outlines the concrete code changes required in `dosemu2` and `dis86` to complete **Phase 1**:
1. Single-instruction stepping and state synchronization hook inside `dosemu2`'s `simx86` core (`interp.c`).
2. Exporting `dosemu2`'s low-memory buffer (`lowmem_base`) directly to `/dev/shm/dosemu_mem` for zero-copy memory validation.
3. Clean JIT block exit and descriptor cache resynchronization (`SetSegReal`) when control flow is altered externally.
4. End-to-end lockstep validation of the 16-bit real-mode test corpus.

---

## 2. `dosemu2` Core Code Modifications

### 2.1 Low-Memory Shared Memory Export (`src/base/lib/mapping/mapfile.c`)
To enable `dis86`'s `ShmMem::attach("/dev/shm/dosemu_mem")` to attach directly to `dosemu2`'s `lowmem_base` without redundant allocations:

```c
#ifdef HAVE_SHM_OPEN
static int do_open_pshm(void)
{
  char *name;
  int ret, fd;

  // Use /dosemu_mem or a persistent PID-tagged name linked to /dev/shm/dosemu_mem
  ret = asprintf(&name, "/dosemu_mem");
  assert(ret != -1);
  shm_unlink(name); // Remove any stale mapping from previous crash
  
  fd = shm_open(name, O_RDWR | O_CREAT | O_TRUNC, S_IRUSR | S_IWUSR);
  if (fd == -1) {
    free(name);
    return -1;
  }
  // Retain the named shm object while dosemu2 is running so the validator can attach
  free(name);
  return fd;
}
#endif
```

---

### 2.2 `simx86` Step Hook & Register Synchronization (`src/base/emu-i386/simx86/hydra_step.c`)

```c
#include "syncpu.h"
#include "protmode.h"
#include "hydra_step.h"
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#include <assert.h>

static shmdata_t *g_shm = NULL;
static int g_state = 0; // 0 = INIT, 1 = WAIT, 2 = RUN
static bool g_hydra_active = false;

void dosemu_hydra_init(void) {
    int fd = open("/dev/shm/hydra_remote", O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) return;
    ftruncate(fd, sizeof(shmdata_t));
    g_shm = (shmdata_t *)mmap(NULL, sizeof(shmdata_t), PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    if (g_shm == MAP_FAILED) return;

    __atomic_store_n(&g_shm->init, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&g_shm->end, 0, __ATOMIC_RELAXED);
    g_shm->pid = getpid();
    g_shm->reserved0 = 0;
    __atomic_store_n(&g_shm->req, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&g_shm->ack, 0, __ATOMIC_RELAXED);
    g_hydra_active = true;
}

static void dump_cpu_to_shm(void) {
    g_shm->ax    = (uint16_t)TheCPU.eax;
    g_shm->bx    = (uint16_t)TheCPU.ebx;
    g_shm->cx    = (uint16_t)TheCPU.ecx;
    g_shm->dx    = (uint16_t)TheCPU.edx;
    g_shm->si    = (uint16_t)TheCPU.esi;
    g_shm->di    = (uint16_t)TheCPU.edi;
    g_shm->bp    = (uint16_t)TheCPU.ebp;
    g_shm->sp    = (uint16_t)TheCPU.esp;
    g_shm->ip    = (uint16_t)TheCPU.eip;
    g_shm->cs    = TheCPU.cs;
    g_shm->ds    = TheCPU.ds;
    g_shm->es    = TheCPU.es;
    g_shm->ss    = TheCPU.ss;
    g_shm->flags = (uint16_t)TheCPU.eflags;
}

static void load_cpu_from_shm(void) {
    TheCPU.eax = (TheCPU.eax & 0xffff0000) | g_shm->ax;
    TheCPU.ebx = (TheCPU.ebx & 0xffff0000) | g_shm->bx;
    TheCPU.ecx = (TheCPU.ecx & 0xffff0000) | g_shm->cx;
    TheCPU.edx = (TheCPU.edx & 0xffff0000) | g_shm->dx;
    TheCPU.esi = (TheCPU.esi & 0xffff0000) | g_shm->si;
    TheCPU.edi = (TheCPU.edi & 0xffff0000) | g_shm->di;
    TheCPU.ebp = (TheCPU.ebp & 0xffff0000) | g_shm->bp;
    TheCPU.esp = (TheCPU.esp & 0xffff0000) | g_shm->sp;
    TheCPU.eip = g_shm->ip;
    TheCPU.eflags = g_shm->flags;

    if (TheCPU.cs != g_shm->cs) SetSegReal(g_shm->cs, Ofs_CS);
    if (TheCPU.ds != g_shm->ds) SetSegReal(g_shm->ds, Ofs_DS);
    if (TheCPU.es != g_shm->es) SetSegReal(g_shm->es, Ofs_ES);
    if (TheCPU.ss != g_shm->ss) SetSegReal(g_shm->ss, Ofs_SS);
}

void dosemu_hydra_step_hook(void) {
    if (!g_hydra_active || !g_shm) return;

    // Force single-instruction mode
    TheCPU.mode |= MSSTP;

    while (1) {
        switch (g_state) {
            case 0: { // STATE_INIT
                dump_cpu_to_shm();
                __atomic_store_n(&g_shm->init, 1, __ATOMIC_RELEASE);
                g_state = 1;
            } break;
            case 1: { // STATE_WAIT
                while (1) {
                    if (__atomic_load_n(&g_shm->end, __ATOMIC_ACQUIRE)) return;
                    uint64_t req = __atomic_load_n(&g_shm->req, __ATOMIC_ACQUIRE);
                    uint64_t ack = __atomic_load_n(&g_shm->ack, __ATOMIC_RELAXED);
                    if (req == ack) {
                        usleep(10);
                        continue;
                    }
                    load_cpu_from_shm();
                    g_state = 2;
                    return; // Execute 1 instruction
                }
            } break;
            case 2: { // STATE_RUN
                dump_cpu_to_shm();
                uint64_t req = __atomic_load_n(&g_shm->req, __ATOMIC_RELAXED);
                __atomic_store_n(&g_shm->ack, req, __ATOMIC_RELEASE);
                g_state = 1;
            } break;
        }
    }
}
```

---

### 2.3 Insertion in `src/base/emu-i386/simx86/interp.c`
Inside `FindExecCode` in `interp.c`:

```c
    while (1) {
        TheCPU.mode &= ~(MSSTP|MTRAP);
        if (EFLAGS & TF)
            TheCPU.mode |= MSSTP|MTRAP;

#if USE_HYDRA
        dosemu_hydra_step_hook();
        if (TheCPU.err == EXCP_EMULEAVE) {
            return PC;
        }
#endif

        G = NULL;
        if (e_querynode(PC)) {
            G = FindTree(PC);
            // ...
```

---

## 3. Lockstep Verification Workflow

1. Spawn `dosemu -dumb -quiet -K <test_dir> -E <test_exe>`.
2. Attach `dis86` validator to `/dev/shm/hydra_remote` and `/dev/shm/dosemu_mem`.
3. Advance `emu86` and `dosemu2` in lockstep:
   - Extract 14 registers at each step.
   - Assert `emu86.regs == dosemu2.regs`.
   - Verify control flow jump targets and stack frames.
4. Clean teardown on binary exit.
