/*
 * lowmem.c - raw shared-memory bridge to dosemu2's guest low memory.
 *
 * dosemu2 launched with $_mapping = "mapmshm" backs its 1MB lowmem +
 * HMA (64KB) with a memfd named "dosemu_<pid>". The fd is visible in
 * /proc/<pid>/fd/ and can be opened from another process, giving a
 * bidirectional MAP_SHARED mapping of the guest's conventional memory.
 *
 * Guest address A < LOWMEM_SIZE + HMASIZE maps to base + A.
 */

#define _POSIX_C_SOURCE 200809L

#include "lowmem.h"

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#define LOWMEM_SIZE 0x100000u /* 1 MB conventional memory */
#define HMASIZE     0x10000u  /* 64 KB high memory area */
#define TOTAL_SIZE  (LOWMEM_SIZE + HMASIZE)

#define MEMFD_PREFIX "/memfd:dosemu_"

struct lowmem {
    pid_t pid;         /* dosemu2 process id */
    int fd;            /* open memfd */
    uint8_t *base;     /* mmap base */
    size_t size;       /* mapped size */
    bool connected;
};

/* ---------------- internals ---------------- */

/*
 * Read the symlink target of /proc/<pid>/fd/<n> into buf (size len).
 * Returns 0 on success, -1 on failure.
 */
static int readlink_proc(pid_t pid, const char *name, char *buf, size_t len)
{
    char path[320];
    snprintf(path, sizeof(path), "/proc/%ld/fd/%s", (long)pid, name);
    ssize_t r = readlink(path, buf, len - 1);
    if (r < 0)
        return -1;
    buf[r] = '\0';
    return 0;
}

/*
 * Scan /proc/<pid>/fd/ for the first entry whose readlink target matches
 * /memfd:dosemu_. Returns the fd number (>= 0), or -1 if none found.
 */
static int find_memfd_fd(pid_t pid)
{
    char path[64];
    snprintf(path, sizeof(path), "/proc/%ld/fd", (long)pid);

    DIR *d = opendir(path);
    if (!d)
        return -1;

    int found = -1;
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (e->d_name[0] == '.')
            continue;
        char target[256];
        if (readlink_proc(pid, e->d_name, target, sizeof(target)) != 0)
            continue;
        if (strncmp(target, MEMFD_PREFIX, strlen(MEMFD_PREFIX)) == 0) {
            char *end = NULL;
            long n = strtol(e->d_name, &end, 10);
            if (end && *end == '\0') {
                found = (int)n;
                break;
            }
        }
    }
    closedir(d);
    return found;
}

/* ---------------- public API ---------------- */

lowmem_t *lowmem_connect(pid_t pid)
{
    lowmem_t *lm = calloc(1, sizeof(*lm));
    if (!lm)
        return NULL;

    int fdnum = find_memfd_fd(pid);
    if (fdnum < 0) {
        fprintf(stderr, "lowmem: no memfd:dosemu_ in /proc/%ld/fd "
                "(is dosemu2 running with $_mapping=\"mapmshm\"?)\n",
                (long)pid);
        free(lm);
        return NULL;
    }

    char path[64];
    snprintf(path, sizeof(path), "/proc/%ld/fd/%d", (long)pid, fdnum);
    int fd = open(path, O_RDWR);
    if (fd < 0) {
        perror("lowmem: open memfd");
        free(lm);
        return NULL;
    }

    struct stat st;
    if (fstat(fd, &st) != 0) {
        perror("lowmem: fstat memfd");
        close(fd);
        free(lm);
        return NULL;
    }
    if (st.st_size < (off_t)TOTAL_SIZE) {
        fprintf(stderr, "lowmem: memfd too small (%ld bytes, need %u)\n",
                (long)st.st_size, (unsigned)TOTAL_SIZE);
        close(fd);
        free(lm);
        return NULL;
    }

    size_t size = (size_t)st.st_size;
    void *base = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (base == MAP_FAILED) {
        perror("lowmem: mmap");
        close(fd);
        free(lm);
        return NULL;
    }

    lm->pid = pid;
    lm->fd = fd;
    lm->base = base;
    lm->size = size;
    lm->connected = true;
    return lm;
}

void lowmem_disconnect(lowmem_t *lm)
{
    if (!lm)
        return;
    if (lm->base && lm->size)
        munmap(lm->base, lm->size);
    if (lm->fd >= 0)
        close(lm->fd);
    lm->base = NULL;
    lm->size = 0;
    lm->fd = -1;
    lm->connected = false;
    free(lm);
}

uint8_t *lowmem_hostaddr(lowmem_t *lm, uint32_t addr)
{
    if (!lm || !lm->connected || !lm->base)
        return NULL;
    if (addr >= lm->size)
        return NULL;
    return lm->base + addr;
}

uint8_t lowmem_read8(lowmem_t *lm, uint32_t addr)
{
    uint8_t *p = lowmem_hostaddr(lm, addr);
    return p ? *p : 0;
}

uint16_t lowmem_read16(lowmem_t *lm, uint32_t addr)
{
    uint8_t *p = lowmem_hostaddr(lm, addr);
    if (!p)
        return 0;
    uint16_t v;
    memcpy(&v, p, sizeof(v)); /* safe for unaligned access */
    return v;
}

void lowmem_write8(lowmem_t *lm, uint32_t addr, uint8_t val)
{
    uint8_t *p = lowmem_hostaddr(lm, addr);
    if (p)
        *p = val;
}

void lowmem_write16(lowmem_t *lm, uint32_t addr, uint16_t val)
{
    uint8_t *p = lowmem_hostaddr(lm, addr);
    if (p)
        memcpy(p, &val, sizeof(val));
}

uint8_t *lowmem_base(lowmem_t *lm)
{
    return lm ? lm->base : NULL;
}

size_t lowmem_size(lowmem_t *lm)
{
    return lm ? lm->size : 0;
}

bool lowmem_is_valid(lowmem_t *lm)
{
    if (!lm || !lm->connected)
        return false;
    return kill(lm->pid, 0) == 0;
}