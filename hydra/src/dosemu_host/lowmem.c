/*
 * lowmem.c - raw shared-memory bridge to dosemu2's guest low memory.
 *
 * mapmshm creates several memfds with the same dosemu_<pid> name. Do not
 * trust readdir() order: production selection cross-checks candidate mappings
 * against guest bytes read independently through dosdebug and fails closed if
 * more than one distinct backing inode matches.
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

struct lowmem {
    pid_t pid;
    int fd;
    uint8_t *base;
    size_t size;
    bool connected;
};

typedef struct candidate_id {
    dev_t dev;
    ino_t ino;
} candidate_id_t;

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

static bool target_is_dosemu_memfd(pid_t pid, const char *target)
{
    char expected[96];
    snprintf(expected, sizeof(expected), "/memfd:dosemu_%ld", (long)pid);
    size_t n = strlen(expected);
    if (strncmp(target, expected, n) != 0)
        return false;
    /* procfs commonly appends " (deleted)". Reject unrelated prefix matches. */
    return target[n] == '\0' || target[n] == ' ';
}

static bool same_id(candidate_id_t a, candidate_id_t b)
{
    return a.dev == b.dev && a.ino == b.ino;
}

static bool probes_match(const uint8_t *base, size_t size,
                         const lowmem_probe_t *probes, size_t probe_count)
{
    for (size_t i = 0; i < probe_count; i++) {
        const lowmem_probe_t *p = &probes[i];
        if (!p->bytes || p->len == 0)
            return false;
        if ((uint64_t)p->addr + p->len > size)
            return false;
        if (memcmp(base + p->addr, p->bytes, p->len) != 0)
            return false;
    }
    return true;
}

static lowmem_t *lowmem_connect_impl(pid_t pid,
                                     const lowmem_probe_t *probes,
                                     size_t probe_count)
{
    char dirpath[64];
    snprintf(dirpath, sizeof(dirpath), "/proc/%ld/fd", (long)pid);
    DIR *d = opendir(dirpath);
    if (!d)
        return NULL;

    lowmem_t *selected = NULL;
    candidate_id_t selected_id = {0, 0};
    candidate_id_t seen[64];
    size_t seen_count = 0;
    size_t eligible_distinct = 0;
    size_t matching_distinct = 0;

    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (e->d_name[0] == '.')
            continue;

        char *end = NULL;
        long fdnum = strtol(e->d_name, &end, 10);
        if (!end || *end != '\0' || fdnum < 0)
            continue;

        char target[256];
        if (readlink_proc(pid, e->d_name, target, sizeof(target)) != 0 ||
            !target_is_dosemu_memfd(pid, target))
            continue;

        char fdpath[96];
        snprintf(fdpath, sizeof(fdpath), "/proc/%ld/fd/%ld", (long)pid, fdnum);
        int fd = open(fdpath, O_RDWR);
        if (fd < 0)
            continue;

        struct stat st;
        if (fstat(fd, &st) != 0 || st.st_size < (off_t)TOTAL_SIZE) {
            close(fd);
            continue;
        }

        candidate_id_t id = {st.st_dev, st.st_ino};
        bool duplicate = false;
        for (size_t i = 0; i < seen_count; i++) {
            if (same_id(seen[i], id)) {
                duplicate = true;
                break;
            }
        }
        if (duplicate) {
            close(fd);
            continue;
        }
        if (seen_count < sizeof(seen) / sizeof(seen[0]))
            seen[seen_count++] = id;
        else {
            fprintf(stderr, "lowmem: too many distinct dosemu memfd candidates\n");
            close(fd);
            goto fail;
        }
        eligible_distinct++;

        size_t size = (size_t)st.st_size;
        uint8_t *base = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (base == MAP_FAILED) {
            close(fd);
            continue;
        }

        bool match = probe_count == 0 || probes_match(base, size, probes, probe_count);
        if (!match) {
            munmap(base, size);
            close(fd);
            continue;
        }

        matching_distinct++;
        if (selected) {
            fprintf(stderr,
                    "lowmem: ambiguous backing: multiple distinct memfds match guest probes\n");
            munmap(base, size);
            close(fd);
            goto fail;
        }

        selected = calloc(1, sizeof(*selected));
        if (!selected) {
            munmap(base, size);
            close(fd);
            goto fail;
        }
        selected->pid = pid;
        selected->fd = fd;
        selected->base = base;
        selected->size = size;
        selected->connected = true;
        selected_id = id;
    }
    closedir(d);

    if (probe_count == 0) {
        /* Compatibility/test helper only: without an independent probe the
         * name is not enough. Accept exactly one distinct eligible memfd. */
        if (eligible_distinct != 1 || !selected) {
            fprintf(stderr,
                    "lowmem: unverified selection requires exactly one distinct candidate (found %zu)\n",
                    eligible_distinct);
            lowmem_disconnect(selected);
            return NULL;
        }
    } else if (matching_distinct != 1 || !selected) {
        fprintf(stderr,
                "lowmem: no unique memfd matched %zu independent guest-memory probe(s)\n",
                probe_count);
        lowmem_disconnect(selected);
        return NULL;
    }

    (void)selected_id;
    return selected;

fail:
    closedir(d);
    lowmem_disconnect(selected);
    return NULL;
}

lowmem_t *lowmem_connect(pid_t pid)
{
    return lowmem_connect_impl(pid, NULL, 0);
}

lowmem_t *lowmem_connect_verified(pid_t pid,
                                  const lowmem_probe_t *probes,
                                  size_t probe_count)
{
    if (!probes || probe_count == 0)
        return NULL;
    return lowmem_connect_impl(pid, probes, probe_count);
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
    if (!p || (uint64_t)addr + sizeof(uint16_t) > lm->size)
        return 0;
    uint16_t v;
    memcpy(&v, p, sizeof(v));
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
    if (p && (uint64_t)addr + sizeof(uint16_t) <= lm->size)
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
