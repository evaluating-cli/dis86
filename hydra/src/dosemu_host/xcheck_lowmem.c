#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lowmem.h"
#include "dosdebug.h"

#define TARGET_ADDR 0x10000u /* seg 0x1000:off 0x0000 */

static int failures = 0;
#define CHECK(cond, label) do { if (cond) printf("PASS: %s\n", label); \
  else { printf("FAIL: %s\n", label); failures++; } } while (0)

int main(int argc, char **argv)
{
    if (argc < 2) { fprintf(stderr, "usage: %s <pid>\n", argv[0]); return 2; }
    pid_t pid = (pid_t)strtol(argv[1], NULL, 0);

    lowmem_t *lm = lowmem_connect(pid);
    CHECK(lm != NULL, "lowmem_connect");
    if (!lm) return 1;

    dosdebug_t *db = dosdebug_connect(pid);
    CHECK(db != NULL, "dosdebug_connect");
    if (!db) { lowmem_disconnect(lm); return 1; }

    uint8_t back[4];

    /* bridge -> CPU view: write via lowmem, read via dosdebug */
    lowmem_write8(lm, TARGET_ADDR, 0x42);
    int n = dosdebug_read_mem(db, 0x1000, 0x0000, back, 1);
    CHECK(n == 1 && back[0] == 0x42, "lowmem write visible via dosdebug (0x42)");
    printf("  dosdebug read back = 0x%02x\n", back[0]);

    /* CPU view -> bridge: write via dosdebug, read via lowmem */
    uint8_t src[1] = { 0x43 };
    CHECK(dosdebug_write_mem(db, 0x1000, 0x0000, src, 1) == 0,
          "dosdebug write 0x43");
    CHECK(lowmem_read8(lm, TARGET_ADDR) == 0x43,
          "dosdebug write visible via lowmem (0x43)");
    printf("  lowmem read back = 0x%02x\n", lowmem_read8(lm, TARGET_ADDR));

    /* restore original byte */
    uint8_t orig = lowmem_read8(lm, TARGET_ADDR);
    (void)orig;

    dosdebug_disconnect(db);
    lowmem_disconnect(lm);

    if (failures == 0) { printf("CROSS-CHECK PASSED\n"); return 0; }
    printf("%d CROSS-CHECK FAILURE(S)\n", failures);
    return 1;
}