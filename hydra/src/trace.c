#include "internal.h"
#include <pthread.h>

/* The Hydra trace sink (issue #97). See trace.h for the format and the
 * contract. The sink is off by default: an ordinary run neither changes
 * behaviour nor pays for the record.
 *
 * Two things are kept here:
 *
 *   - the sink itself (a file or an in-memory buffer) and the writer;
 *   - the event counters. The counters advance whether or not a sink is
 *     open, so a host can report what the machine did without turning the
 *     trace on. They are not per-run: a host that needs a per-run figure
 *     takes the difference, exactly like host_run_stats_t does with the
 *     hook dispatch count.
 *
 * Hooks run on their own pthreads (see run_begin in exec.c), so the sink is
 * written under a mutex. The lock is only taken when a sink is open.
 */

#define TRACE_MAX_ARGS 8
#define TRACE_MEM_INITIAL 4096

typedef enum {
  TRACE_SINK_NONE = 0,
  TRACE_SINK_FILE,
  TRACE_SINK_MEM,
} trace_sink_kind_t;

typedef struct trace_state {
  /* The kind is read without the lock on the hot path (an open sink is
   * the rare case), so it is read and written atomically. The writes
   * happen under the lock; the atomic store is what makes the lock-free
   * read well defined. */
  int kind;
  FILE *fp;
  char *buf;
  size_t len;
  size_t cap;
  u16 last_code_load;
  int have_code_load;

  u64 calls;
  u64 returns;
  u64 native_calls;
  u64 native_returns;
  u64 steps;
  u64 interrupts;
} trace_state_t;

static trace_state_t T[1];

/* The sink is written from the hook threads as well as from the host's
 * thread, so it is written under a lock. The lock is a separate object
 * because a statically initialised mutex cannot be part of a designated
 * initialiser. */
static pthread_mutex_t T_MUTEX = PTHREAD_MUTEX_INITIALIZER;

static int trace_kind(void)
{
  return __atomic_load_n(&T->kind, __ATOMIC_ACQUIRE);
}

static void trace_set_kind(int kind)
{
  __atomic_store_n(&T->kind, kind, __ATOMIC_RELEASE);
}

/* Close the sink. The caller holds the lock. */
static void trace_close_locked(void)
{
  if (trace_kind() == TRACE_SINK_FILE && T->fp) {
    fclose(T->fp);
    T->fp = NULL;
  }
  trace_set_kind(TRACE_SINK_NONE);
  T->have_code_load = 0;
}

static void trace_write_locked(const char *line)
{
  int kind = trace_kind();
  if (kind == TRACE_SINK_FILE) {
    fputs(line, T->fp);
    return;
  }

  if (kind != TRACE_SINK_MEM) return;

  size_t n = strlen(line);
  if (T->len + n + 1 > T->cap) {
    size_t cap = T->cap ? T->cap : TRACE_MEM_INITIAL;
    while (T->len + n + 1 > cap) cap *= 2;
    char *buf = realloc(T->buf, cap);
    if (!buf) FAIL("Failed to grow the trace buffer to %zu bytes", cap);
    T->buf = buf;
    T->cap = cap;
  }

  memcpy(T->buf + T->len, line, n);
  T->len += n;
  T->buf[T->len] = '\0';
}

/* The load segment can change under a running trace (the MZ path moves the
 * code after the image is loaded), so it is re-emitted whenever it changes.
 * A reader applies the most recent preceding line, not the first one. */
static void trace_emit_code_load_locked(void)
{
  u16 load = CODE_START_SEG;
  if (load == (u16)-1) return; /* not configured yet */
  if (T->have_code_load && T->last_code_load == load) return;

  char line[64];
  snprintf(line, sizeof(line), "# code_load=%04x\n", load);
  trace_write_locked(line);
  T->last_code_load = load;
  T->have_code_load = 1;
}

static void trace_emit_locked(const char *line)
{
  if (trace_kind() == TRACE_SINK_NONE) return;
  trace_emit_code_load_locked();
  trace_write_locked(line);
}

int hydra_trace_open(const char *path)
{
  if (!path) FAIL("hydra_trace_open(NULL)");

  FILE *fp = fopen(path, "w");
  if (!fp) {
    fprintf(stderr, "hydra trace: cannot open '%s' for writing\n", path);
    return -1;
  }

  /* Line buffered: a trace that is lost because the run died is a trace that
   * cannot be used for the work it was recorded for. */
  setvbuf(fp, NULL, _IOLBF, 0);

  pthread_mutex_lock(&T_MUTEX);
  trace_close_locked();
  T->fp = fp;
  trace_set_kind(TRACE_SINK_FILE);
  T->have_code_load = 0;
  trace_write_locked("# hydra trace v1\n");
  pthread_mutex_unlock(&T_MUTEX);

  return 0;
}

int hydra_trace_open_mem(void)
{
  pthread_mutex_lock(&T_MUTEX);
  trace_close_locked();
  trace_set_kind(TRACE_SINK_MEM);
  T->len = 0;
  T->have_code_load = 0;
  if (!T->buf) {
    T->buf = malloc(TRACE_MEM_INITIAL);
    if (!T->buf) FAIL("Failed to allocate the trace buffer");
    T->cap = TRACE_MEM_INITIAL;
  }
  if (T->buf) T->buf[0] = '\0';
  trace_write_locked("# hydra trace v1\n");
  pthread_mutex_unlock(&T_MUTEX);

  return 0;
}

void hydra_trace_close(void)
{
  pthread_mutex_lock(&T_MUTEX);
  trace_close_locked();
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_flush(void)
{
  pthread_mutex_lock(&T_MUTEX);
  if (trace_kind() == TRACE_SINK_FILE && T->fp) fflush(T->fp);
  pthread_mutex_unlock(&T_MUTEX);
}

int hydra_trace_enabled(void)
{
  return trace_kind() != TRACE_SINK_NONE;
}

const char *hydra_trace_mem(size_t *_len)
{
  if (trace_kind() != TRACE_SINK_MEM) return NULL;

  /* The pointer and the length are read together, so they agree at the
   * moment of the call. The buffer may still move afterwards: a hook
   * running concurrently can grow it, which is why the sink may only be
   * read while the machine is stopped. */
  pthread_mutex_lock(&T_MUTEX);
  if (_len) *_len = T->len;
  const char *buf = T->buf ? T->buf : "";
  pthread_mutex_unlock(&T_MUTEX);

  return buf;
}

/* Whether a sink is open. Read without the lock: the kind is a single
 * word read atomically, and the only thing a read that races with a
 * close could cost is a trace event that is written (or skipped) one
 * moment late. */
static int trace_sink_open(void)
{
  return trace_kind() != TRACE_SINK_NONE;
}

/* The name of a function at an address, or NULL. Overlay addresses have no
 * absolute segment of their own, so they carry no name here: the physical
 * segment the overlay is mapped at is what the guest branches to, and that
 * is what the trace records. */
static const char *trace_name_of(addr_t addr)
{
  if (addr_is_overlay(addr)) return NULL;
  return hydra_function_name(addr);
}

/* A name is recorded as a single whitespace-free token: the format is
 * whitespace separated, so a name with a space in it would break the
 * reader. A name that is not one is not recorded at all - the address is,
 * and the reader still works with it. The names come from the user's
 * metadata, so this is their spelling to lose, not the trace's. */
static int trace_name_is_token(const char *name)
{
  if (!name || !*name) return 0;
  for (const char *p = name; *p; p++) {
    if (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') return 0;
  }
  return 1;
}

/* Append " name=<name>" to the line, if the name is one the format can
 * carry. Returns the new length, or the old one when nothing was
 * written.
 *
 * A name that does not fit in the line is left out: a line that does not
 * fit is not written at all, and an event with its addresses is worth more
 * than an event with a name. */
static int trace_add_name(char *line, size_t size, int n, const char *name)
{
  if (!trace_name_is_token(name)) return n;
  if (n <= 0 || (size_t)n >= size) return n;
  size_t len = strlen(name);
  if (len + sizeof(" name=") > size - (size_t)n) return n;
  return n + snprintf(line + n, size - n, " name=%s", name);
}

/* The absolute guest address of a callee. An overlay callee resolves to the
 * physical segment the overlay is currently mapped at. */
static void trace_abs_addr(addr_t addr, u16 *_seg, u16 *_off)
{
  *_off = addr_off(addr);
  if (addr_is_overlay(addr)) {
    *_seg = hydra_overlay_segment_lookup(addr_overlay_num(addr));
    return;
  }
  *_seg = (u16)(CODE_START_SEG + addr_seg(addr));
}

void hydra_trace_dispatch(const char *name, u16 cs, u16 ip,
                          const char *phase, const char *result)
{
  if (!trace_sink_open()) return;

  pthread_mutex_lock(&T_MUTEX);
  char line[256];
  int n = snprintf(line, sizeof(line), "dispatch %s cs=%04x ip=%04x", phase, cs, ip);
  n = trace_add_name(line, sizeof(line), n, name);
  if (result && n > 0 && (size_t)n < sizeof(line)) {
    n += snprintf(line + n, sizeof(line) - n, " result=%s", result);
  }
  if (n > 0 && (size_t)n < sizeof(line)) {
    n += snprintf(line + n, sizeof(line) - n, "\n");
  }
  if (n > 0 && (size_t)n < sizeof(line)) trace_emit_locked(line);
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_call(u16 from_seg, u16 from_off, u16 to_seg, u16 to_off)
{
  __atomic_fetch_add(&T->calls, 1, __ATOMIC_RELAXED);
  if (!trace_sink_open()) return;

  /* The callee's name. It is resolved here, after the sink check, because
   * the lookup is a scan over the metadata: a closed sink must not pay for
   * it. */
  const char *name = hydra_function_name_at(to_seg, to_off);

  pthread_mutex_lock(&T_MUTEX);
  char line[256];
  int n = snprintf(line, sizeof(line), "call from=%04x:%04x to=%04x:%04x",
                   from_seg, from_off, to_seg, to_off);
  n = trace_add_name(line, sizeof(line), n, name);
  if (n > 0 && (size_t)n < sizeof(line)) {
    n += snprintf(line + n, sizeof(line) - n, "\n");
  }
  if (n > 0 && (size_t)n < sizeof(line)) trace_emit_locked(line);
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_ret(u16 from_seg, u16 from_off, u16 to_seg, u16 to_off)
{
  __atomic_fetch_add(&T->returns, 1, __ATOMIC_RELAXED);
  if (!trace_sink_open()) return;

  /* The name of the function that was left (the previous position). */
  const char *name = hydra_function_name_at(from_seg, from_off);

  pthread_mutex_lock(&T_MUTEX);
  char line[256];
  int n = snprintf(line, sizeof(line), "ret from=%04x:%04x to=%04x:%04x",
                   from_seg, from_off, to_seg, to_off);
  n = trace_add_name(line, sizeof(line), n, name);
  if (n > 0 && (size_t)n < sizeof(line)) {
    n += snprintf(line + n, sizeof(line) - n, "\n");
  }
  if (n > 0 && (size_t)n < sizeof(line)) trace_emit_locked(line);
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_native_call(hydra_machine_t *m, addr_t callee, int nargs)
{
  if (!m) FAIL("hydra_trace_native_call: no machine");

  __atomic_fetch_add(&T->native_calls, 1, __ATOMIC_RELAXED);
  if (!trace_sink_open()) return;

  if (nargs < 0) nargs = 0;
  if (nargs > TRACE_MAX_ARGS) nargs = TRACE_MAX_ARGS;

  u16 args[TRACE_MAX_ARGS];
  if (nargs > 0) {
    /* The arguments are on the guest stack right now: the call stub pushed
     * them and the call itself has not run yet, so this is exactly what the
     * callee is about to read. */
    u32 sp = (u32)m->registers->ss * 16 + m->registers->sp;
    for (int i = 0; i < nargs; i++) {
      args[i] = m->hardware->mem_read16(m->hardware->ctx, sp + 2 * (u32)i);
    }
  }

  u16 to_seg, to_off;
  trace_abs_addr(callee, &to_seg, &to_off);
  const char *name = trace_name_of(callee);

  pthread_mutex_lock(&T_MUTEX);
  char line[512];
  int n = snprintf(line, sizeof(line), "native_call from=%04x:%04x to=%04x:%04x",
                   m->registers->cs, m->registers->ip, to_seg, to_off);
  n = trace_add_name(line, sizeof(line), n, name);
  if (n > 0 && (size_t)n < sizeof(line)) {
    n += snprintf(line + n, sizeof(line) - n, " nargs=%d", nargs);
  }
  for (int i = 0; i < nargs && n > 0 && (size_t)n < sizeof(line); i++) {
    n += snprintf(line + n, sizeof(line) - n, " arg%d=%04x", i, args[i]);
  }
  if (n > 0 && (size_t)n < sizeof(line)) {
    n += snprintf(line + n, sizeof(line) - n, "\n");
  }
  if (n > 0 && (size_t)n < sizeof(line)) trace_emit_locked(line);
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_native_ret(hydra_machine_t *m, addr_t callee)
{
  if (!m) FAIL("hydra_trace_native_ret: no machine");

  __atomic_fetch_add(&T->native_returns, 1, __ATOMIC_RELAXED);
  if (!trace_sink_open()) return;

  u16 to_seg, to_off;
  trace_abs_addr(callee, &to_seg, &to_off);

  pthread_mutex_lock(&T_MUTEX);
  char line[256];
  int n = snprintf(line, sizeof(line), "native_ret from=%04x:%04x to=%04x:%04x dx=%04x ax=%04x",
                   m->registers->cs, m->registers->ip, to_seg, to_off,
                   m->registers->dx, m->registers->ax);
  if (n > 0 && (size_t)n < sizeof(line)) {
    n += snprintf(line + n, sizeof(line) - n, "\n");
  }
  if (n > 0 && (size_t)n < sizeof(line)) trace_emit_locked(line);
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_step(u16 cs, u16 ip)
{
  __atomic_fetch_add(&T->steps, 1, __ATOMIC_RELAXED);
  if (!trace_sink_open()) return;

  pthread_mutex_lock(&T_MUTEX);
  char line[64];
  int n = snprintf(line, sizeof(line), "step cs=%04x ip=%04x\n", cs, ip);
  if (n > 0 && (size_t)n < sizeof(line)) trace_emit_locked(line);
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_int(u8 num, u8 ah)
{
  __atomic_fetch_add(&T->interrupts, 1, __ATOMIC_RELAXED);
  if (!trace_sink_open()) return;

  pthread_mutex_lock(&T_MUTEX);
  char line[64];
  int n = snprintf(line, sizeof(line), "int num=%02x ah=%02x\n", num, ah);
  if (n > 0 && (size_t)n < sizeof(line)) trace_emit_locked(line);
  pthread_mutex_unlock(&T_MUTEX);
}

void hydra_trace_stats(hydra_run_stats_t *_out)
{
  if (!_out) return;

  pthread_mutex_lock(&T_MUTEX);
  _out->calls = __atomic_load_n(&T->calls, __ATOMIC_RELAXED);
  _out->returns = __atomic_load_n(&T->returns, __ATOMIC_RELAXED);
  _out->native_calls = __atomic_load_n(&T->native_calls, __ATOMIC_RELAXED);
  _out->native_returns = __atomic_load_n(&T->native_returns, __ATOMIC_RELAXED);
  _out->steps = __atomic_load_n(&T->steps, __ATOMIC_RELAXED);
  _out->interrupts = __atomic_load_n(&T->interrupts, __ATOMIC_RELAXED);
  pthread_mutex_unlock(&T_MUTEX);
}
