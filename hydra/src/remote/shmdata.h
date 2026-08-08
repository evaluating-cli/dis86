#pragma once
#include "header.h"

typedef struct shmdata shmdata_t;

// IMPORTANT: this layout must match dis86/src/emu86/validator/shmdata.rs.
// req/ack are accessed atomically by both processes and therefore must remain
// naturally aligned. Keep the explicit reserved word so their offsets are
// stable across the supported 64-bit host ABIs.
struct shmdata
{
  u32 init;
  u32 end;
  u32 pid;
  u32 reserved0;
  u64 req;  // request step by incrementing
  u64 ack;  // ack step by matching 'req' value

  // registers
  u16 ax;
  u16 bx;
  u16 cx;
  u16 dx;
  u16 si;
  u16 di;
  u16 bp;
  u16 sp;
  u16 ip;
  u16 cs;
  u16 ds;
  u16 es;
  u16 ss;
  u16 flags;

  // memory
  // TODO...
};

_Static_assert(offsetof(shmdata_t, req) == 16, "shmdata.req ABI offset changed");
_Static_assert(offsetof(shmdata_t, ack) == 24, "shmdata.ack ABI offset changed");
_Static_assert(offsetof(shmdata_t, ax) == 32, "shmdata register ABI offset changed");
_Static_assert(sizeof(shmdata_t) == 64, "shmdata ABI size changed");

shmdata_t *shmdata_create(const char *path);
shmdata_t *shmdata_attach(const char *path);
