/*
 * Host-independent negative test for the external dosdebug backend's overlay
 * policy. Production overlay hooks are physical entry stubs carrying the
 * OVERLAY flag; without overlays=armed they must still fail before execution.
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

    /* Match generated production registration: physical entry-stub address +
     * HYDRA_HOOK_FLAGS_OVERLAY. Armed mode resolves its separate logical
     * F_name_OVERLAY metadata; default mode must reject before needing that. */
    hydra_hook_t hook = {
        "F_overlay_probe",
        h_overlay_probe,
        ADDR_MAKE(0x0010, 0x0100),
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

    printf("PASS: production-shaped overlay hook rejected before guest execution\n");
    return 0;
}
