/* native_runner.c - the native side of the #96 parity lane.
 *
 * Runs the dis86 --codegen-hydra output for parity/fixture.exe on the
 * standalone native host (hydra/src/native_host): a 1 MiB linear guest
 * memory, no dosemu2 process, no emulator. It loads the fixture's module at
 * segment 1 (linear 0x10), releases the fixture's park sentinel, seeds the
 * near-return landing slot, and drives the hook machinery to completion.
 * The fixture writes its outbox with stores only, so the runner just reads
 * the memory back, checks it against the shared contract (parity/outbox.h)
 * and writes the same snapshot the golden lane captures.
 *
 * usage: native_runner <fixture.exe> <out.lanes>
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
    const char *exe_path, *out_path;
    uint8_t *image;
    size_t image_size;
    hydra_machine_t m = {0};
    hydra_machine_audio_t audio = {0};
    native_host_ctx_t *ctx;
    uint32_t base;
    uint16_t code_seg;
    const uint8_t *header;
    int steps = 0;

    if (argc != 3) {
        fprintf(stderr, "usage: %s <fixture.exe> <out.lanes>\n", argv[0]);
        return 2;
    }
    exe_path = argv[1];
    out_path = argv[2];

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

    /* Guest termination is process-wide, not a return value: if the fixture
     * ever reaches INT 21h AH=4Ch, the native host ends the process from
     * inside the INT hook and control never comes back here. That means none
     * of the reporting below runs — no outbox_scan, no verify, no snapshot —
     * and a guest exiting with AL=0 is indistinguishable from this runner
     * succeeding. Today's fixture parks instead of terminating
     * (fixture.asm drives AH=4Ch only in the lane's planned v2), so this is
     * latent. Before v2 lands, the reporting either has to move into an
     * atexit handler registered after hydra_machine_init, or termination has
     * to become something the exec loop can observe. See the "Guest
     * termination" note in hydra/README.md. */
    for (;;) {
        int running = hydra_machine_exec(&m, 0);

        /* Hydra's callstack bookkeeping is a two-phase event: the exec
         * records it and the host must drain it with notify() (the dosemu
         * host driver does this right after every exec). */
        hydra_machine_notify(&m);

        if (!running)
            break;
        if (++steps > 100000) {
            fprintf(stderr, "native_runner: livelocked at %04x:%04x after "
                    "100000 steps\n", m.registers->cs - code_seg,
                    m.registers->ip);
            return 1;
        }
    }

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
    return 0;
}
