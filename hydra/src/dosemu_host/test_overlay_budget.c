/* Host-independent regressions for Option E metadata/breakpoint accounting. */
#include <stdio.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"

HYDRA_FUNC(h_budget_probe)
{
    (void)m;
    RETURN_NEAR();
}

int main(void)
{
    host_ctx_t ctx = {0};
    hydra_machine_t machine = {0};
    host_run_stats_t stats = {0};

    /* The core has exactly 64 overlay segment slots. Reject invalid metadata
     * before the host can copy an out-of-range overlay number into its own
     * reconciliation arrays (including while the stub is still unpaged). */
    hydra_function_def_t bad_defs[] = {
        { "F_bad_OVERLAY",
          ADDR_MAKE_EXT(1, HYDRA_OVERLAY_SEGMENT_COUNT, 0x0000) },
    };
    hydra_function_metadata_t bad_md = {
        sizeof(bad_defs) / sizeof(bad_defs[0]), bad_defs
    };
    if (hydra_function_metadata_set(&bad_md) == 0) {
        fprintf(stderr, "FAIL: out-of-range overlay metadata was accepted\n");
        return 1;
    }

    /* Satisfy only the precondition ahead of breakpoint preflight. A correct
     * failure must happen before any dosdebug connection or guest operation. */
    ctx.raw_code_reserved = 1;
    ctx.raw_code_size = 128;
    ctx.overlays_armed = 1;

    for (unsigned i = 0; i < HOST_RUN_MAX_BPS; i++) {
        hydra_hook_t hook = {
            NULL,
            h_budget_probe,
            ADDR_MAKE((u16)(0x0100u + i), 0x0000),
            0,
        };
        hydra_hook_register(hook);
    }

    /* The dynamic site is the 65th worst-case debugger entry. Its metadata is
     * intentionally absent: preflight must reject before overlay resolution. */
    hydra_hook_t overlay = {
        "F_budget_overlay",
        h_budget_probe,
        ADDR_MAKE(0x0200, 0x0100),
        HYDRA_HOOK_FLAGS_OVERLAY,
    };
    hydra_hook_register(overlay);

    host_run_stop_reason_t reason = host_run(&ctx, &machine, NULL, &stats);
    if (reason != HOST_RUN_STOP_ERROR) {
        fprintf(stderr, "FAIL: 64 static + 1 dynamic overlay site was not rejected\n");
        return 1;
    }
    if (stats.stops != 0 || stats.hook_dispatches != 0 ||
        stats.raw_code_runs != 0) {
        fprintf(stderr, "FAIL: breakpoint-budget rejection advanced execution\n");
        return 1;
    }

    printf("PASS: overlay metadata bounds and dynamic breakpoint budget are fail-closed\n");
    return 0;
}
