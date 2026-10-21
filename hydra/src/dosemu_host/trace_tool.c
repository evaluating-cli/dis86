/*
 * trace_tool.c - the instrumented-execution tool (issue #97).
 *
 * Runs a DOS guest on the stock dosemu2 host with Hydra's trace sink open,
 * and writes what the machine did to a trace file. The tool adds no host
 * machinery of its own: it drives the same host_run() loop the integration
 * tests drive, with the trace sink open and the per-instruction step seam
 * enabled on request.
 *
 * usage: hydra_trace --pid <dosemu2 pid> --program <file.com> --trace <out>
 *                    [--lib <user.so>] [--step] [--stop-after <n>]
 *                    [--poke-image <off>:<val>] [--max-steps <n>]
 *                    [--timeout-ms <n>] [--verbose]
 *
 * The user library (--lib) is the usual Hydra metadata library: it provides
 * the function metadata and, when it exports hydra_user_register_hooks(),
 * the native implementations of the hooked functions. The dosemu host calls
 * that entry point after the metadata is in place, so a library built from
 * the confgen appdata can register its hooks without the driver knowing
 * anything about them.
 *
 * What the trace holds is bounded by what the host can see: the guest CPU
 * runs free between the function-level breakpoints, so the trace has the
 * hook boundaries, the guest call/return boundaries the callstack model
 * sees at them, the native-to-guest call seam, and - with --step - the
 * individual guest instructions of the regions the host single-steps.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "hydra_machine.h"
#include "internal.h"
#include "host.h"
#include "host_driver.h"
#include "lowmem.h"
#include "trace.h"

static int g_failures;
#define CHECK(cond, ...) do {                                      \
  if (!(cond)) {                                                   \
    printf("  FAIL: "); printf(__VA_ARGS__); printf("\n");         \
    g_failures++;                                                 \
  } else {                                                         \
    printf("  ok:   "); printf(__VA_ARGS__); printf("\n");         \
  }                                                                \
} while (0)

static const char *reason_name(host_run_stop_reason_t r)
{
  switch (r) {
    case HOST_RUN_STOP_NONE:        return "NONE";
    case HOST_RUN_STOP_CALLBACK:    return "CALLBACK";
    case HOST_RUN_STOP_MAXSTEPS:    return "MAXSTEPS";
    case HOST_RUN_STOP_DOSEMU_EXIT: return "DOSEMU_EXIT";
    case HOST_RUN_STOP_TIMEOUT:     return "TIMEOUT";
    case HOST_RUN_STOP_ERROR:       return "ERROR";
  }
  return "?";
}

/* The guest is loaded by dosemu2 itself (-E program.com). Wait until its
 * bytes are in memory and the segment it runs in is known. */
static uint16_t wait_for_com(host_ctx_t *ctx, const uint8_t *file, size_t file_len)
{
  size_t check_len = file_len < 16 ? file_len : 16;
  for (int i = 0; i < 80; i++) {
    dosdebug_regs_t r;
    if (host_get_regs(ctx, &r) != 0) return 0;
    uint8_t m16[16];
    int got = dosdebug_read_mem(ctx->db, r.cs, 0x100, m16, (int)check_len);
    if (got == (int)check_len && memcmp(m16, file, check_len) == 0)
      return r.cs;
    if (dosdebug_go(ctx->db) != 0) return 0;
    usleep(250 * 1000u);
    if (dosdebug_stop(ctx->db) != 0) return 0;
  }
  return 0;
}

static int read_file(const char *path, uint8_t **out, size_t *out_len)
{
  FILE *f = fopen(path, "rb");
  if (!f) return -1;
  if (fseek(f, 0, SEEK_END) != 0) { fclose(f); return -1; }
  long sz = ftell(f);
  if (sz < 0 || sz > 32768) { fclose(f); return -1; }
  rewind(f);
  uint8_t *buf = malloc((size_t)sz);
  if (!buf || fread(buf, 1, (size_t)sz, f) != (size_t)sz) {
    free(buf); fclose(f); return -1;
  }
  fclose(f);
  *out = buf;
  *out_len = (size_t)sz;
  return 0;
}

static size_t find_sig(const uint8_t *file, size_t file_len,
                       const uint8_t *sig, size_t sig_len)
{
  for (size_t i = 0; i + sig_len <= file_len; i++) {
    if (memcmp(file + i, sig, sig_len) == 0)
      return i;
  }
  return (size_t)-1;
}

/* The host refuses to run without a guest-owned raw-code region, and the
 * tool does not commandeer a low-memory address of its own: the region is
 * found by the marker the guest image carries (the same convention the
 * integration fixture uses), and the bytes after the marker are what the
 * guest reserved for Hydra's instruction snippets. */
static const uint8_t raw_marker[] = "HYDRA_RAW_SLOT!!"; /* 16 bytes + C NUL */
#define RAW_SCRATCH_SIZE 8192u

/* Stop the run once the guest has been through this many hook dispatches.
 * A guest that loops forever (the integration fixture does) would otherwise
 * never let go. */
static int stop_after_dispatches(host_ctx_t *ctx, const dosdebug_regs_t *regs,
                                 const host_run_stats_t *stats, void *user)
{
  (void)ctx; (void)regs;
  uint64_t target = *(const uint64_t *)user;
  return stats->hook_dispatches >= target;
}

static void usage(const char *argv0)
{
  fprintf(stderr,
    "usage: %s --pid <dosemu2 pid> --program <file.com> --trace <out>\n"
    "          [--lib <user.so>] [--step] [--stop-after <n>]\n"
    "          [--poke-image <off>:<val>] [--max-steps <n>]\n"
    "          [--timeout-ms <n>] [--verbose]\n",
    argv0);
}

int main(int argc, char **argv)
{
  const char *pid_str = NULL;
  const char *program = NULL;
  const char *trace_path = NULL;
  const char *lib = "";
  int step = 0;
  int verbose = 0;
  uint64_t stop_after = 0;
  size_t max_steps = 0;
  int timeout_ms = 3000;
  long poke_off = -1;
  int poke_val = 0;

  for (int i = 1; i < argc; i++) {
    if (!strcmp(argv[i], "--pid") && i + 1 < argc) {
      pid_str = argv[++i];
    } else if (!strcmp(argv[i], "--program") && i + 1 < argc) {
      program = argv[++i];
    } else if (!strcmp(argv[i], "--trace") && i + 1 < argc) {
      trace_path = argv[++i];
    } else if (!strcmp(argv[i], "--lib") && i + 1 < argc) {
      lib = argv[++i];
    } else if (!strcmp(argv[i], "--step")) {
      step = 1;
    } else if (!strcmp(argv[i], "--verbose")) {
      verbose = 1;
    } else if (!strcmp(argv[i], "--stop-after") && i + 1 < argc) {
      stop_after = strtoull(argv[++i], NULL, 0);
    } else if (!strcmp(argv[i], "--max-steps") && i + 1 < argc) {
      max_steps = (size_t)strtoull(argv[++i], NULL, 0);
    } else if (!strcmp(argv[i], "--timeout-ms") && i + 1 < argc) {
      timeout_ms = (int)strtol(argv[++i], NULL, 0);
    } else if (!strcmp(argv[i], "--poke-image") && i + 1 < argc) {
      const char *spec = argv[++i];
      const char *colon = strchr(spec, ':');
      if (!colon) {
        fprintf(stderr, "hydra_trace: --poke-image wants <off>:<val>\n");
        return 2;
      }
      poke_off = strtol(spec, NULL, 0);
      poke_val = (int)strtol(colon + 1, NULL, 0);
    } else {
      fprintf(stderr, "hydra_trace: unknown argument '%s'\n", argv[i]);
      usage(argv[0]);
      return 2;
    }
  }

  if (!pid_str || !program || !trace_path) {
    usage(argv[0]);
    return 2;
  }

  printf("=== Hydra instrumented-execution trace ===\n");

  uint8_t *file = NULL;
  size_t file_len = 0;
  if (read_file(program, &file, &file_len) != 0) {
    fprintf(stderr, "hydra_trace: cannot read '%s'\n", program);
    return 2;
  }

  char conf[1024];
  snprintf(conf, sizeof(conf),
           "dosemu|pid=%s|code_load=0x0|data_seg=0x0|lib=%s",
           pid_str, lib);

  hydra_machine_t m = {0};
  hydra_machine_audio_t audio = {0};
  hydra_machine_init(m.hardware, &audio, conf);
  host_ctx_t *ctx = (host_ctx_t *)m.hardware->ctx;

  uint16_t com_seg = wait_for_com(ctx, file, file_len);
  CHECK(com_seg != 0, "the guest program is loaded (segment %04x)", com_seg);
  if (!com_seg) { host_disconnect(ctx); free(file); return 1; }

  /* Adopt the segment the guest actually runs in, so every address - the
   * image-relative ones the metadata uses included - is interpreted against
   * it. */
  host_set_code_load(ctx, com_seg);

  /* Reserve the raw-code region the guest image carries. The host refuses
   * to run without one, and no address is guessed here. */
  {
    size_t marker_pos = find_sig(file, file_len, raw_marker, 16);
    CHECK(marker_pos != (size_t)-1,
          "the guest image carries the raw-code reservation marker");
    if (marker_pos == (size_t)-1) {
      host_disconnect(ctx); free(file); return 1;
    }
    size_t scratch_pos = marker_pos + 16;
    uint32_t com_phys = (uint32_t)com_seg << 4;
    uint32_t scratch_phys = com_phys + 0x100u + (uint32_t)scratch_pos;
    CHECK(scratch_phys + RAW_SCRATCH_SIZE <= com_phys + 0x100u + (uint32_t)file_len,
          "the raw-code reservation lies inside the guest image");
    CHECK((scratch_phys & 0x0fu) == 0,
          "the raw-code reservation is paragraph aligned (%06x)", scratch_phys);
    CHECK(host_reserve_raw_code(ctx, scratch_phys, RAW_SCRATCH_SIZE) == 0,
          "the guest-owned raw-code region is registered");
    CHECK(host_raw_code_ready(ctx), "the raw-code region is ready");
  }

  if (poke_off >= 0) {
    /* The COM image starts 0x100 bytes into the segment (the PSP is in
     * front of it), so an image offset is segment*16 + 0x100 + off. */
    uint32_t phys = ((uint32_t)com_seg << 4) + 0x100u + (uint32_t)poke_off;
    lowmem_write8(ctx->lm, phys, (uint8_t)poke_val);
    printf("  poked %04x:%04x = %02x\n", com_seg,
           (unsigned)(0x100 + poke_off), (unsigned)poke_val);
  }

  if (hydra_trace_open(trace_path) != 0) {
    fprintf(stderr, "hydra_trace: cannot write the trace to '%s'\n", trace_path);
    host_disconnect(ctx); free(file); return 1;
  }

  host_run_stats_t stats;
  host_run_options_t opts = {0};
  opts.timeout_ms = timeout_ms;
  opts.verbose = verbose;
  opts.max_steps = max_steps;
  opts.trace_steps = step;
  if (stop_after > 0) {
    opts.stop_fn = stop_after_dispatches;
    opts.stop_user = &stop_after;
  }

  host_run_stop_reason_t reason = host_run(ctx, &m, &opts, &stats);

  /* The trace is closed before anything else is reported: the file is the
   * deliverable, and it must be complete even if the run went wrong. */
  hydra_trace_close();

  printf("host_run ended: %s\n", reason_name(reason));
  printf("stops=%lu hook_dispatches=%lu raw_code_runs=%lu redirects=%lu\n",
         (unsigned long)stats.stops, (unsigned long)stats.hook_dispatches,
         (unsigned long)stats.raw_code_runs, (unsigned long)stats.redirects);

  hydra_run_stats_t mstats;
  if (hydra_machine_stats(&mstats) == 0) {
    printf("trace events: calls=%llu returns=%llu native_calls=%llu "
           "native_returns=%llu steps=%llu interrupts=%llu\n",
           (unsigned long long)mstats.calls, (unsigned long long)mstats.returns,
           (unsigned long long)mstats.native_calls,
           (unsigned long long)mstats.native_returns,
           (unsigned long long)mstats.steps,
           (unsigned long long)mstats.interrupts);
  }

  printf("trace written to %s\n", trace_path);

  host_disconnect(ctx);
  free(file);

  return g_failures ? 1 : 0;
}
