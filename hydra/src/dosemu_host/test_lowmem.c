/*
 * test_lowmem.c - exercises the raw lowmem memfd bridge against a running
 * dosemu2 instance.
 *
 * Usage: test_lowmem <dosemu2_pid>
 *
 * Tested functions: connect, base, size, hostaddr, read8, read16, write8,
 * write16, is_valid, disconnect.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "lowmem.h"

static int g_failures = 0;

#define CHECK(cond, label)                                              \
    do {                                                                \
        if (cond) {                                                     \
            printf("PASS: %s\n", label);                                \
        } else {                                                        \
            printf("FAIL: %s\n", label);                                \
            g_failures++;                                               \
        }                                                               \
    } while (0)

static void print_bytes(const char *tag, const uint8_t *b, int n)
{
    printf("%s (%d bytes):", tag, n);
    for (int i = 0; i < n; i++)
        printf(" %02x", b[i]);
    printf("\n");
}

int main(int argc, char **argv)
{
    if (argc < 2) {
        fprintf(stderr, "usage: %s <dosemu2_pid>\n", argv[0]);
        return 2;
    }
    pid_t pid = (pid_t)strtol(argv[1], NULL, 0);

    /* --- connect --- */
    lowmem_t *lm = lowmem_connect(pid);
    CHECK(lm != NULL, "lowmem_connect");
    if (!lm) {
        fprintf(stderr, "unable to connect to dosemu2 (pid=%ld)\n", (long)pid);
        return 1;
    }

    /* --- base + size --- */
    uint8_t *base = lowmem_base(lm);
    size_t size = lowmem_size(lm);
    printf("base = %p  size = 0x%zx (%zu)\n", (void *)base, size, size);
    CHECK(base != NULL, "lowmem_base != NULL");
    CHECK(size >= 0x110000u, "lowmem_size >= 0x110000");

    /* --- hostaddr ranges ---
     * The memfd backs more than lowmem+HMA: dosemu grows it to cover
     * extended memory too, so any addr < size is mapped. Only addresses
     * at/above size (or above UINT32_MAX) are out of range.
     */
    CHECK(lowmem_hostaddr(lm, 0x0000) == base, "hostaddr(0x0000) == base");
    CHECK(lowmem_hostaddr(lm, 0x10ffff) == base + 0x10ffff,
          "hostaddr(0x10ffff) == base + 0x10ffff");
    CHECK(lowmem_hostaddr(lm, (uint32_t)(size - 1)) == base + size - 1,
          "hostaddr(size-1) == base + size - 1");
    CHECK(lowmem_hostaddr(lm, (uint32_t)size) == NULL,
          "hostaddr(size) == NULL");
    CHECK(lowmem_hostaddr(lm, 0xffffffffu) == NULL,
          "hostaddr(0xffffffff) == NULL");

    /* --- IVT (offset 0): first 16 bytes --- */
    uint8_t ivt[16];
    for (int i = 0; i < 16; i++)
        ivt[i] = lowmem_read8(lm, (uint32_t)i);
    print_bytes("IVT @ 0x0000", ivt, 16);

    /* --- BDA (offset 0x400): 16 bytes --- */
    uint8_t bda[16];
    for (int i = 0; i < 16; i++)
        bda[i] = lowmem_read8(lm, 0x400 + (uint32_t)i);
    print_bytes("BDA @ 0x0400", bda, 16);

    /* --- 16-bit read at a known safe location --- */
    uint16_t w = lowmem_read16(lm, 0x0000);
    CHECK(w == (uint16_t)(ivt[0] | (ivt[1] << 8)),
          "read16(0x0000) matches IVT bytes 0-1");
    printf("read16(0x0000) = 0x%04x\n", w);

    /* --- write/read verification at 0x10000 --- */
    const uint32_t addr = 0x10000;
    uint8_t orig = lowmem_read8(lm, addr);
    lowmem_write8(lm, addr, 0x90);
    CHECK(lowmem_read8(lm, addr) == 0x90, "write8(0x10000, 0x90) read back");
    lowmem_write8(lm, addr, orig);
    CHECK(lowmem_read8(lm, addr) == orig, "write8 restored original byte");

    /* --- write16 at an odd address (unaligned) --- */
    uint8_t o1 = lowmem_read8(lm, 0x10001);
    uint8_t o2 = lowmem_read8(lm, 0x10002);
    lowmem_write16(lm, 0x10001, 0xbeef);
    CHECK(lowmem_read8(lm, 0x10001) == 0xef && lowmem_read8(lm, 0x10002) == 0xbe,
          "write16 unaligned @ 0x10001 verified (be ef)");
    lowmem_write8(lm, 0x10001, o1);
    lowmem_write8(lm, 0x10002, o2);

    /* --- is_valid --- */
    CHECK(lowmem_is_valid(lm), "lowmem_is_valid (dosemu2 alive)");

    /* --- disconnect --- */
    lowmem_disconnect(lm);
    printf("disconnected\n");

    if (g_failures == 0) {
        printf("ALL TESTS PASSED\n");
        return 0;
    }
    printf("%d TEST(S) FAILED\n", g_failures);
    return 1;
}