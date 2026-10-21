/* outbox.h - the parity fixture's memory contract (issue #96).
 *
 * fixture.asm writes a byte-exact "outbox" into its own module image and
 * the two lanes of the parity test read it back through completely
 * different transports:
 *
 *   golden : live dosemu2 guest lowmem (parity/golden_capture.c)
 *   native : dis86 --codegen-hydra output on hydra + native_host
 *            (parity/native_runner.c)
 *
 * Both sides locate the outbox by scanning for the header rather than by
 * hardcoding a guest address, so the DOS load segment and the native host's
 * linear layout are free to differ. The snapshot the two lanes compare is
 * the header plus the full reserved lane area; the header is written last,
 * so its presence alone proves a complete world.
 *
 * Keep the layout constants below in lockstep with parity/fixture.asm
 * (parity/build.sh asserts the .asm `equ` values against these defines) and
 * the expected lane table in lockstep with the emitted code; the table is
 * the standing fixture's known-answer test and only changes when the
 * fixture's observable behavior intentionally changes.
 */

#ifndef PARITY_OUTBOX_H
#define PARITY_OUTBOX_H

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* ---- fixture.asm layout (module-relative) --------------------------------- */

#define OUTBOX_MAGIC "PHYPAR16"
#define OUTBOX_MAGIC_LEN 8
#define OUTBOX_VERSION 3
#define OUTBOX_LANE_RESERVE 64
#define OUTBOX_HEADER_SIZE 16
#define OUTBOX_LANE_M 0x0210
#define OUTBOX_LANE_N 43
#define OUTBOX_M 0x0200
#define OUTBOX_PARK_M 0x0260

/* Module the fixture loads: file = 0x20-byte MZ header + 0x400 module. */
#define FIXTURE_HEADER_SIZE 0x20
#define FIXTURE_MODULE_SIZE 0x400

/* Snapshot compared by the two lanes: header + every reserved lane byte. */
#define OUTBOX_SNAPSHOT_SIZE (OUTBOX_HEADER_SIZE + OUTBOX_LANE_RESERVE)

/* The fixture's complete, known observable surface: lanes 0..29 are the
 * arithmetic/string/aliasing lanes, lanes 30..40 exercise the byte-SAR /
 * byte-MUL / byte-IMUL / word-IMUL / word-DIV / word-IDIV remainder
 * lowerings, lane 41 is the "exit-AL" checksum (sum of lanes 0..40 mod
 * 256), lane 42 is unused (zero). */
static const uint8_t outbox_expected_lanes[OUTBOX_LANE_N] = {
    0xC0, 0xC1, 0x00, 0x01, 0x10, 0x00, 0x02, 0x81,
    0x81, 0x80, 0x0B, 0x2C, 0x78, 0x02, 0x8C, 0x02,
    0x34, 0x34, 0x12, 0x34, 0x20, 0x01, 0x78, 0x12,
    0x34, 0x34, 0x7A, 0x11, 0x80, 0x88, 0xF8, 0x08,
    0x04, 0xE0, 0xFF, 0x23, 0x56, 0xCD, 0xFF, 0xFE,
    0xFF, 0xCE, 0x00,
};

/* ---- shared helpers ------------------------------------------------------- */

/* Scan guest memory for a complete outbox header. Returns a pointer to the
 * header inside `mem` (lanes start at +OUTBOX_HEADER_SIZE) or NULL. The full
 * magic+version+reserve pattern is required so partial writes during boot
 * cannot produce a false positive. */
static inline const uint8_t *outbox_scan(const uint8_t *mem, size_t size)
{
    uint8_t pattern[OUTBOX_HEADER_SIZE];
    size_t i;

    memset(pattern, 0, sizeof(pattern));
    memcpy(pattern, OUTBOX_MAGIC, OUTBOX_MAGIC_LEN);
    pattern[8] = OUTBOX_VERSION;
    pattern[9] = OUTBOX_LANE_RESERVE;

    if (size < OUTBOX_HEADER_SIZE)
        return NULL;
    for (i = 0; i + OUTBOX_HEADER_SIZE <= size; i++) {
        if (memcmp(mem + i, pattern, OUTBOX_HEADER_SIZE) == 0)
            return mem + i;
    }
    return NULL;
}

static inline const uint8_t *outbox_lanes(const uint8_t *header)
{
    return header + OUTBOX_HEADER_SIZE;
}

static inline void outbox_print_lanes(const uint8_t *header, FILE *f)
{
    const uint8_t *lanes = outbox_lanes(header);
    int i;
    for (i = 0; i < OUTBOX_LANE_N; i++)
        fprintf(f, "%02x%c", lanes[i], i + 1 == OUTBOX_LANE_N ? '\n' : ' ');
}

/* Verify a located outbox against the fixture contract: version/reserve
 * header, the tabulated lane values, and the lane-30 checksum. Returns 0 on
 * success; prints a diagnostic and returns -1 otherwise. */
static inline int outbox_verify(const uint8_t *header)
{
    const uint8_t *lanes = outbox_lanes(header);
    unsigned int sum = 0;
    int i;

    if (memcmp(header, OUTBOX_MAGIC, OUTBOX_MAGIC_LEN) != 0)
        fprintf(stderr, "outbox: bad magic\n");
    else if (header[8] != OUTBOX_VERSION)
        fprintf(stderr, "outbox: version %u != %u\n", header[8], OUTBOX_VERSION);
    else if (header[9] != OUTBOX_LANE_RESERVE)
        fprintf(stderr, "outbox: lane reserve %u != %u\n", header[9], OUTBOX_LANE_RESERVE);
    else {
        for (i = 0; i < OUTBOX_LANE_N; i++) {
            if (lanes[i] != outbox_expected_lanes[i]) {
                fprintf(stderr, "outbox: lane %d = %02x, expected %02x\n",
                        i, lanes[i], outbox_expected_lanes[i]);
                return -1;
            }
        }
        for (i = 0; i < OUTBOX_LANE_N - 2; i++)
            sum += lanes[i];
        if ((sum & 0xff) != lanes[OUTBOX_LANE_N - 2]) {
            fprintf(stderr, "outbox: checksum %02x, expected %02x\n",
                    lanes[OUTBOX_LANE_N - 2], (unsigned)(sum & 0xff));
            return -1;
        }
        return 0;
    }
    return -1;
}

/* Write the compared snapshot (header + reserved lanes) to `path`. */
static inline int outbox_write_snapshot(const uint8_t *header, const char *path)
{
    FILE *f = fopen(path, "wb");
    if (!f) {
        fprintf(stderr, "outbox: cannot open %s\n", path);
        return -1;
    }
    if (fwrite(header, 1, OUTBOX_SNAPSHOT_SIZE, f) != OUTBOX_SNAPSHOT_SIZE) {
        fprintf(stderr, "outbox: short write to %s\n", path);
        fclose(f);
        return -1;
    }
    if (fclose(f) != 0) {
        fprintf(stderr, "outbox: close failed for %s\n", path);
        return -1;
    }
    return 0;
}

#endif /* PARITY_OUTBOX_H */
