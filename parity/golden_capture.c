/* golden_capture.c - golden lane of the #96 parity fixture.
 *
 * usage: golden_capture <dosemu-pid> <out.lanes> [timeout_s]
 *
 * Connects to the running dosemu2 guest's lowmem backing, polls for the
 * fixture outbox header (parity/outbox.h) written by parity/fixture.asm,
 * verifies it against the standing contract, and writes the same 80-byte
 * snapshot the native lane writes. The fixture keeps spinning on its park
 * sentinel; this tool never releases it. The caller owns session teardown.
 *
 * Exit codes: 0 capture+verify ok, 1 failure, 2 usage.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "lowmem.h"
#include "outbox.h"

#define POLL_MS 100

int main(int argc, char **argv)
{
    pid_t pid;
    const uint8_t *base;
    const uint8_t *header = NULL;
    lowmem_t *lm;
    int timeout_s = 60;
    int waited_ms = 0;

    if (argc < 3 || argc > 4) {
        fprintf(stderr, "usage: %s <dosemu-pid> <out.lanes> [timeout_s]\n",
                argv[0]);
        return 2;
    }
    pid = (pid_t)strtol(argv[1], NULL, 0);
    if (argc == 4) {
        timeout_s = atoi(argv[3]);
        if (timeout_s <= 0) {
            fprintf(stderr, "golden_capture: bad timeout\n");
            return 2;
        }
    }

    lm = lowmem_connect(pid);
    if (!lm) {
        fprintf(stderr, "golden_capture: lowmem_connect(%d) failed\n",
                (int)pid);
        return 1;
    }
    base = lowmem_base(lm);

    while (waited_ms <= timeout_s * 1000) {
        header = outbox_scan(base, lowmem_guest_size());
        if (header)
            break;
        if (!lowmem_is_valid(lm)) {
            fprintf(stderr, "golden_capture: dosemu2 exited before the "
                    "outbox appeared\n");
            lowmem_disconnect(lm);
            return 1;
        }
        usleep(POLL_MS * 1000);
        waited_ms += POLL_MS;
    }
    if (!header) {
        fprintf(stderr, "golden_capture: outbox did not appear within %d s\n",
                timeout_s);
        lowmem_disconnect(lm);
        return 1;
    }

    if (outbox_verify(header) != 0) {
        fprintf(stderr, "golden_capture: outbox at linear 0x%06x failed "
                "verification\n", (unsigned)(header - base));
        lowmem_disconnect(lm);
        return 1;
    }

    outbox_print_lanes(header, stdout);
    if (outbox_write_snapshot(header, argv[2]) != 0) {
        lowmem_disconnect(lm);
        return 1;
    }

    printf("golden: PASS (outbox at linear 0x%06x after %d.%03d s)\n",
           (unsigned)(header - base), waited_ms / 1000, waited_ms % 1000);
    lowmem_disconnect(lm);
    return 0;
}
