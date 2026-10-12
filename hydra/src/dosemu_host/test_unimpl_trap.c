/*
 * Compile + runtime coverage for the UNIMPL/UNIMPL_FLAGS markers
 * (hydra/src/header.h). The decompiler emits these calls for unmodeled
 * operations (UNIMPL) and imprecise flags values (UNIMPL_FLAGS): both must
 * accept their operands (type-checked, then discarded unevaluated) in
 * expression, statement, and condition position. UNIMPL traps loudly with
 * file:line; UNIMPL_FLAGS warns on stderr with file:line and returns 0
 * rather than aborting the function. Each case runs in a forked child so
 * the abort / exit is observable without killing the test process itself.
 */

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "internal.h"

static void trap_in_expression_u16(void) {
  u16 ax = 0x1234, bx = 0x5678;
  u16 x = UNIMPL(ax, bx); /* UNIMPL op */
  (void)x;
}

static void trap_in_expression_u8(void) {
  u16 flags = 0x0202;
  u8 y = (u8)UNIMPL_FLAGS(flags); /* UNIMPL flags */
  (void)y;
  _exit(20); /* reachable: the macro warns and returns 0 */
}

static void trap_in_expression_u32(void) {
  u16 hi = 0x1111, lo = 0x2222;
  u32 z = UNIMPL(hi, lo); /* UNIMPL op */
  (void)z;
}

static void trap_as_bare_statement(void) {
  UNIMPL(); /* UNIMPL op */
}

static void trap_in_condition(void) {
  u16 cond = 1;
  if (UNIMPL_FLAGS(cond)) { /* UNIMPL flags */
    _exit(42); /* unreachable: the macro returns 0 */
  }
  _exit(43); /* reachable */
}

static int read_child_stderr(int fd, char *buf, size_t cap) {
  ssize_t got = 0;
  ssize_t n;
  while ((n = read(fd, buf + got, cap - (size_t)got - 1)) > 0) {
    got += n;
    if (got >= (ssize_t)cap - 1) break;
  }
  if (got < 0) got = 0;
  buf[got] = '\0';
  return 0;
}

static int check_abort_case(const char *what, void (*fn)(void), const char *marker) {
  int fds[2];
  if (pipe(fds) != 0) { perror("pipe"); return 1; }
  pid_t pid = fork();
  if (pid < 0) { perror("fork"); close(fds[0]); close(fds[1]); return 1; }
  if (pid == 0) {
    close(fds[0]);
    if (dup2(fds[1], STDERR_FILENO) < 0) _exit(98);
    close(fds[1]);
    fn();
    /* If the trap returned, fail distinctly from an abort. */
    _exit(99);
  }
  close(fds[1]);
  char buf[512];
  read_child_stderr(fds[0], buf, sizeof(buf));
  close(fds[0]);
  int status = 0;
  if (waitpid(pid, &status, 0) < 0) { perror("waitpid"); return 1; }
  if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGABRT) {
    fprintf(stderr, "FAIL: %s: child did not abort (status=%d)\n", what, status);
    return 1;
  }
  if (strstr(buf, marker) == NULL ||
      strstr(buf, "test_unimpl_trap.c") == NULL) {
    fprintf(stderr, "FAIL: %s: missing file:line in trap output: %s\n", what, buf);
    return 1;
  }
  printf("PASS: %s\n", what);
  return 0;
}

static int check_continue_case(const char *what, void (*fn)(void), int expected_code, const char *marker) {
  int fds[2];
  if (pipe(fds) != 0) { perror("pipe"); return 1; }
  pid_t pid = fork();
  if (pid < 0) { perror("fork"); close(fds[0]); close(fds[1]); return 1; }
  if (pid == 0) {
    close(fds[0]);
    if (dup2(fds[1], STDERR_FILENO) < 0) _exit(98);
    close(fds[1]);
    fn();
    /* If the function returned instead of exiting, fail distinctly. */
    _exit(99);
  }
  close(fds[1]);
  char buf[512];
  read_child_stderr(fds[0], buf, sizeof(buf));
  close(fds[0]);
  int status = 0;
  if (waitpid(pid, &status, 0) < 0) { perror("waitpid"); return 1; }
  if (!WIFEXITED(status) || WEXITSTATUS(status) != expected_code) {
    fprintf(stderr, "FAIL: %s: child did not exit %d (status=%d)\n", what, expected_code, status);
    return 1;
  }
  if (strstr(buf, marker) == NULL ||
      strstr(buf, "test_unimpl_trap.c") == NULL) {
    fprintf(stderr, "FAIL: %s: missing file:line in warn output: %s\n", what, buf);
    return 1;
  }
  printf("PASS: %s\n", what);
  return 0;
}

int main(void) {
  int failures = 0;
  failures += check_abort_case("UNIMPL in u16 expression", trap_in_expression_u16, "UNIMPL executed at");
  failures += check_continue_case("UNIMPL_FLAGS in u8 expression", trap_in_expression_u8, 20, "UNIMPL_FLAGS executed at");
  failures += check_abort_case("UNIMPL in u32 expression", trap_in_expression_u32, "UNIMPL executed at");
  failures += check_abort_case("UNIMPL as bare statement", trap_as_bare_statement, "UNIMPL executed at");
  failures += check_continue_case("UNIMPL_FLAGS in condition", trap_in_condition, 43, "UNIMPL_FLAGS executed at");
  if (failures != 0) {
    fprintf(stderr, "test_unimpl_trap: %d case(s) failed\n", failures);
    return 1;
  }
  printf("test_unimpl_trap: all cases passed\n");
  return 0;
}
