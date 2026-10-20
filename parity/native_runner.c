/* native_runner.c - the native side of the #96 parity lane.
 *
 * Runs the dis86 --codegen-hydra output for parity/fixture.exe on the
 * standalone native host (hydra/src/native_host): a 1 MiB linear guest
 * memory, no dosemu2 process, no emulator. It loads the fixture's module at
 * segment 1 (linear 0x10), releases the fixture's park sentinel, seeds the
 * near-return landing, and drives the hook machinery to completion.
 * The fixture writes its outbox with stores only, so the runner just reads
 * the memory back, checks it against the shared contract (parity/outbox.h)
 * and writes the same snapshot the golden lane captures.
 *
 * With --trace <file> the runner also records what the machine did in the
 * Hydra trace format (issue #97). That trace is the #97 parity gate: the
 * observations it holds are checked against the fixture's known layout
 * (parity/annotations.py), so the tool's correctness rides on the anchored
 * fixture instead of on itself.
 *
 * usage: native_runner <fixture.exe> <out.lanes> [--trace <file>]
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The generated appdata header pulls in hydra/hydra.h (all Hydra macros and
 * types) and declares the fixture hook registration helper. Do NOT also
 * include src/internal.h here: several Hydra headers have no include guards,
 * so the hydra.h path and the internal.h path would redefine them. */
#include "hydra_user_appdata.h"
#include "native_host.h"
#include "outbox.h"

/* Defined by hydra/src/conf.c; declared here to avoid internal.h. */
extern hydra_conf_t HYDRA_CONF[1];

/* The fixture's own landing: the `jmp $` stall at module 0x0189 lives
 * outside both annotated functions, so a hook miss there makes
 * hydra_exec_run return RESUME (0) and ends the loop without ever running
 * raw bytes. We seed the entry-return word with it because the decompiled
 * epilogue pops the near-return IP from [SS:SP0] (SP0 = 0x400). */
#define END_STALL_M 0x0189
#define ENTRY_SP 0x400

static uint8_t *read_file(const char *path, size_t *out_size)
{
    FILE *f = fopen(path, "rb");
    long sz;
    uint8_t *buf;

    if (!f) {
        fprintf(stderr, "native_runner: cannot open %s\n", path);
        return NULL;
    }
    if (fseek(f, 0, SEEK_END) != 0 || (sz = ftell(f)) < 0 ||
        fseek(f, 0, SEEK_SET) != 0) {
        fprintf(stderr, "native_runner: cannot size %s\n", path);
        fclose(f);
        return NULL;
    }
    buf = malloc((size_t)sz);
    if (!buf || fread(buf, 1, (size_t)sz, f) != (size_t)sz) {
        fprintf(stderr, "native_runner: short read on %s\n", path);
        free(buf);
        fclose(f);
        return NULL;
    }
    fclose(f);
    *out_size = (size_t)sz;
    return buf;
}

int main(int argc, char **argv)
{
    const char *exe_path, *out_path, *trace_path = NULL;
    uint8_t *image;
    size_t image_size;
    hydra_machine_t m = {0};
    hydra_machine_audio_t audio = {0};
    native_host_ctx_t *ctx;
    uint32_t base;
    uint16_t code_seg;
    const uint8_t *header;
    int steps = 0;

    if (argc < 3) {
        fprintf(stderr, "usage: %s <fixture.exe> <out.lanes> [--trace <file>]\n", argv[0]);
        return 2;
    }
    exe_path = argv[1];
    out_path = argv[2];
    for (int i = 3; i < argc; i++) {
        if (!strcmp(argv[i], "--trace") && i + 1 < argc) {
            trace_path = argv[++i];
        } else {
            fprintf(stderr, "native_runner: unknown argument '%s'\n", argv[i]);
            return 2;
        }
    }

    image = read_file(exe_path, &image_size);
    if (!image)
        return 1;
    if (image_size != FIXTURE_HEADER_SIZE + FIXTURE_MODULE_SIZE ||
        image[0] != 'M' || image[1] != 'Z') {
        fprintf(stderr, "native_runner: %s is not the expected MZ fixture "
                "(%zu bytes)\n", exe_path, image_size);
        free(image);
        return 1;
    }

    hydra_machine_init(m.hardware, &audio, "native|code_load=1|data_seg=0");

    /* Metadata is loaded by hydra_machine_init; register both fixture hooks
     * only after it, or hydra_function_find cannot resolve them. */
    hydra_user_appdata__register_all_hooks();

    ctx = (native_host_ctx_t *)m.hardware->ctx;
    code_seg = HYDRA_CONF->code_load_offset;
    base = 16u * (uint32_t)code_seg;
    memcpy(ctx->mem + base, image + FIXTURE_HEADER_SIZE, FIXTURE_MODULE_SIZE);
    free(image);

    /* Release the park sentinel and seed the near-return landing word. */
    ctx->mem[base + OUTBOX_PARK_M] = 1;
    ctx->mem[base + ENTRY_SP + 0] = (uint8_t)(END_STALL_M & 0xff);
    ctx->mem[base + ENTRY_SP + 1] = (uint8_t)(END_STALL_M >> 8);

    *m.registers = (hydra_machine_registers_t){0};
    m.registers->cs = code_seg;
    m.registers->ds = code_seg;
    m.registers->es = code_seg;
    m.registers->ss = code_seg;
    m.registers->ip = 0;
    m.registers->sp = ENTRY_SP;

    /* Guest termination is a result the exec loop reports, not a process
     * exit: when the fixture asks to terminate (INT 21h AH=4Ch), the core
     * stops the hook at the interrupt — the guest's post-INT code never
     * runs — and hydra_machine_exec returns HYDRA_EXEC_TERMINATED. Control
     * comes back here, so the reporting below (outbox_scan, verify,
     * snapshot) runs before this process ends, and the guest's exit code is
     * known to the driver instead of being swallowed by an exit() nobody
     * could report from.
     *
     * Today's fixture parks instead of terminating (fixture.asm drives
     * AH=4Ch only in the lane's planned v2), so this branch is not taken yet
     * — it is here because the lane's v2 needs it, and because a driver that
     * cannot tell "the guest finished" from "the guest is stuck" is the
     * failure mode the livelock cap below only detects after 100000 steps. */
    int guest_exit_code = -1;
    if (trace_path && hydra_trace_open(trace_path) != 0) {
        fprintf(stderr, "native_runner: cannot write the trace to %s\n", trace_path);
        return 1;
    }
    for (;;) {
        int ret = hydra_machine_exec(&m, 0);

        /* Hydra's callstack bookkeeping is a two-phase event: the exec
         * records it and the host must drain it with notify() (the dosemu
         * host driver does this right after every exec). */
        hydra_machine_notify(&m);

        if (ret == HYDRA_EXEC_TERMINATED) {
            guest_exit_code = hydra_machine_exit_code();
            break;
        }
        if (ret == HYDRA_EXEC_RESUME)
            break;
        if (++steps > 100000) {
            fprintf(stderr, "native_runner: livelocked at %04x:%04x after "
                    "100000 steps\n", m.registers->cs - code_seg,
                    m.registers->ip);
            hydra_trace_close();
            return 1;
        }
    }
    if (trace_path)
        hydra_trace_close();

    header = outbox_scan(ctx->mem, NATIVE_HOST_MEM_SIZE);
    if (!header) {
        fprintf(stderr, "native_runner: outbox header never appeared\n");
        return 1;
    }
    if (header != ctx->mem + base + OUTBOX_M)
        fprintf(stderr, "native_runner: note: outbox at linear 0x%lx\n",
                (unsigned long)(header - ctx->mem));

    if (outbox_verify(header) != 0)
        return 1;
    outbox_print_lanes(header, stdout);
    if (outbox_write_snapshot(header, out_path) != 0)
        return 1;

    printf("native: PASS (steps=%d, outbox at module 0x%04x)\n",
           steps, (unsigned)(header - ctx->mem - base));
    if (guest_exit_code >= 0)
        printf("native: guest exit code %d\n", guest_exit_code);
    /* The exit status of this process is the lane's verdict, not the guest's
     * exit code: a guest that exits with a non-zero code is a normal
     * outcome, and reporting it as a lane failure would make the parity
     * comparison unreachable. */
    return 0;
}
