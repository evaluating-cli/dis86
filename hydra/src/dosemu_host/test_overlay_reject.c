/*
 * Host-independent negative test for the external dosdebug backend's overlay
 * policy. Overlay hooks have no stable physical breakpoint until paging has
 * resolved them, so this backend must reject the run before guest execution.
 */

#include <stdio.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"

HYDRA_FUNC(h_overlay_probe)
{
    (void)m;
    RETURN_NEAR();
}

int main(void)
{
    host_ctx_t ctx = {0};
    hydra_machine_t machine = {0};
    host_run_stats_t stats = {0};

    /* host_run checks raw-code ownership before hook installation. Satisfy
     * that precondition directly; the overlay rejection path must not touch
     * dosdebug or guest memory at all. */
    ctx.raw_code_reserved = 1;
    ctx.raw_code_size = 128;

    /* HYDRA_REGISTER_ADDR() deliberately constructs an ordinary seg:off
     * address, which is not a valid overlay hook. Register an overlay-typed
     * address explicitly so the negative test exercises the real contract. */
    hydra_hook_t hook = {
        NULL,
        h_overlay_probe,
        ADDR_MAKE_EXT(1, 0x0010, 0x0100),
        HYDRA_HOOK_FLAGS_OVERLAY,
    };
    hydra_hook_register(hook);

    if (host_hook_breakpoint_count(&ctx) != 0) {
        fprintf(stderr, "FAIL: overlay hook counted as a static breakpoint\n");
        return 1;
    }

    host_run_stop_reason_t reason = host_run(&ctx, &machine, NULL, &stats);
    if (reason != HOST_RUN_STOP_ERROR) {
        fprintf(stderr, "FAIL: overlay hook was not rejected before execution (reason=%d)\n",
                (int)reason);
        return 1;
    }
    if (stats.stops != 0 || stats.hook_dispatches != 0 ||
        stats.raw_code_runs != 0) {
        fprintf(stderr, "FAIL: overlay rejection advanced execution statistics\n");
        return 1;
    }

    printf("PASS: overlay hook rejected before guest execution\n");
    return 0;
}
