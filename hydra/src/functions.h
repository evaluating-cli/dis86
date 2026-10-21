typedef struct hydra_function_metadata  hydra_function_metadata_t;
typedef struct hydra_function_def       hydra_function_def_t;

struct hydra_function_def
{
  const char *name;
  addr_t addr;
};


struct hydra_function_metadata
{
  size_t                 n_defs;
  hydra_function_def_t * defs;
};

void                         hydra_function_metadata_init(void);
int                          hydra_function_metadata_set(const hydra_function_metadata_t *md);
const hydra_function_def_t * hydra_function_find(const char *name);
const char *                 hydra_function_name(addr_t s);
bool                         hydra_function_addr(const char *name, addr_t *_out);

/* The name of the function at an absolute guest address, or NULL.
 *
 * The metadata holds image-relative addresses, so the guest segment is
 * brought back into that space first. An address below the code segment
 * (DOS or ROM code) has no metadata entry, and asking for one must not
 * abort the way addr_relative_to_segment does. */
const char *                 hydra_function_name_at(u16 cs, u16 off);
