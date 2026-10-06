_TypeSizes = {
    'u8':  1,
    'i8':  1,
    'u16': 2,
    'i16': 2,
    'u32': 4,
    'i32': 4,
}

def _pointer_width(typename):
    spec = _pointer_spec(typename)
    if spec is None:
        return None
    return 4 if spec[0] == 'far' else 2

def _pointer_spec(typename):
    import re
    match = re.fullmatch(r'(near|near_ss|near_es|far)<([A-Za-z_]\w*)>', typename)
    if match:
        return match.groups()
    return None

def basetype_size_in_bytes(typename):
    return _TypeSizes.get(typename, None)

class Type:
    def __init__(self, basetype, dimensions=()):
        self.basetype = basetype
        self.dimensions = list(dimensions)

    @property
    def is_array(self):
        return bool(self.dimensions)

    @property
    def array_len(self):
        return self.dimensions[0] if self.dimensions else None

    @array_len.setter
    def array_len(self, value):
        if not self.dimensions:
            raise Exception('Cannot set an array bound on a scalar type')
        self.dimensions[0] = value

    @staticmethod
    def from_str(s):
        import re
        s = s.strip()
        m = re.fullmatch(r'([^\[\]]+)((?:\[[0-9]*\])*)', s)
        if not m:
            raise Exception(f'Invalid type: "{s}"')
        base, suffix = m.groups()
        if base.startswith(('near<', 'near_ss<', 'near_es<', 'far<')) and not _pointer_width(base):
            raise Exception(f'Invalid guest pointer annotation: "{s}"')
        dims = [int(n) if n else None for n in re.findall(r'\[([0-9]*)\]', suffix)]
        if any(n == 0 for n in dims):
            raise Exception(f'Array bounds must be positive: "{s}"')
        return Type(base, dims)

    def as_basetype(self):
        return Type(self.basetype)

    def get_ctype_str_parts(self):
        assert isinstance(self.basetype, str)
        basetype = self.guest_pointer_ctype() or self.basetype

        if not self.is_array:
            return (basetype, '')
        else:
            if any(n is None for n in self.dimensions):
                raise Exception(f'No array length provided for: {self} ... required by get_ctype_str_parts()')
            return (basetype, ''.join(f'[{n}]' for n in self.dimensions))

    def storage_type(self):
        return {2: 'u16', 4: 'u32'}.get(_pointer_width(self.basetype), self.basetype)

    def guest_pointer_ctype(self):
        spec = _pointer_spec(self.basetype)
        if spec is None:
            return None
        kind, pointee = spec
        return f'dis86_{kind}_ptr_{pointee}'

    def guest_pointer_typedef(self):
        ctype = self.guest_pointer_ctype()
        return None if ctype is None else (self.storage_type(), ctype)

    def fmt_ctype_str(self, name):
        start, end = self.get_ctype_str_parts()
        return f'{start:<15} {name}{end}'

    def size_in_bytes(self):
        basesz = _pointer_width(self.basetype) or basetype_size_in_bytes(self.basetype)
        if basesz is None: return None
        if any(n is None for n in self.dimensions): return None
        for n in self.dimensions:
            basesz *= n
            if basesz > 0xffff:
                raise Exception(f'Type size exceeds 16-bit guest layout: {self}')
        return basesz

    def __str__(self):
        s = self.basetype
        for n in self.dimensions:
            s += f'[{"" if n is None else n}]'
        return s

class Off:
    def __init__(self, off):
        self.off = int(off, 16)
        assert 0 <= self.off and self.off < (1<<16)

    def __str__(self):
        return f'0x{self.off:04x}'

class Addr:
    def __init__(self, addr):
        parts = addr.split(':')
        if len(parts) != 2: raise Exception(f'Invalid address: "{addr}"')

        seg_str = parts[0]
        off_str = parts[1]

        self.overlay = False
        if seg_str.startswith('overlay_'):
            self.overlay = True
            seg_str = seg_str[8:]

        self.seg = int(seg_str, 16)
        self.off = int(off_str, 16)

    def bytes_since(self, start):
        assert self.overlay == start.overlay
        assert self.seg == start.seg
        return self.off - start.off

    def __str__(self):
        pre = 'overlay_' if self.overlay else ''
        return pre + f'{self.seg:04x}:{self.off:04x}'

### FIXME RENAME
UNKNOWN=-1

FUNCTION_ALL_NAMES = set()
def _verify_unique(name):
    if name in FUNCTION_ALL_NAMES:
        raise Exception(f'Duplicate function name: {name}')
    FUNCTION_ALL_NAMES.add(name)

class Function:
    def __init__(self, reimpl, name, ret, args, start_addr, end_addr, flags=0, regargs=None, entry=None):
        _verify_unique(name);
        self.name = name
        self.ret = ret
        self.args = args
        self.start_addr = Addr(start_addr)
        self.end_addr = Addr(end_addr) if end_addr else None
        self.entry_stub = Addr(entry) if entry else None
        self.is_overlay_entry = self.start_addr.overlay and self.entry_stub is not None
        self.regargs = ','.join(regargs) if regargs else None
        self.flags = flags
        self.reimpl = reimpl

class Global:
    def __init__(self, name, typ, off, flags=''):
        self.name = name
        self.typ = Type.from_str(typ)
        self.off = off
        self.flags = flags

def validate_data_section(ds):
    mem = [0] * (1<<16)
    for g in ds:
        if g.flags == 'SKIP_VALIDATE':
            continue
        off = g.off
        sz = g.typ.size_in_bytes()
        if sz is None:
            if g.typ.is_array and any(n is None for n in g.typ.dimensions):
                raise Exception('Global %s uses an array without all fixed bounds: %s' % (g.name, g.typ))
            print('WARN: Cannot determine size for %s' % g.name)
            continue
        if off < 0 or off + sz > (1 << 16):
            raise Exception('Global %s layout [%#x, %#x) exceeds the 64 KiB data section' % (g.name, off, off + sz))
        for i in range(off, off+sz):
            if mem[i] != 0:
                raise Exception('Overlap detect in %s: [0x%04x, 0x%04x]' % (g.name, off, off+sz))
            mem[i] = 1

class TextData:
    def __init__(self, name, typ, start_addr, end_addr, access_at=None):
        self.name = name
        self.typ = Type.from_str(typ)
        self.start_addr = Addr(start_addr)
        self.end_addr = Addr(end_addr)
        self.access_at = access_at

        ## infer array size
        nbytes = self.end_addr.bytes_since(self.start_addr)
        if nbytes < 0: raise Exception(f"Negatively sized text-section region: {name}")
        if not self.typ.is_array: raise Exception(f"Expected array for text-section region: {name}")
        eltsz = self.typ.as_basetype().size_in_bytes()
        if eltsz is None:
            raise Exception(f"Cannot determine element size for text-section region: {name}")
        for dim in self.typ.dimensions[1:]:
            if dim is None:
                raise Exception(f"Unknown inner array bound in text-section region: {name}")
            eltsz *= dim
        if eltsz is None or eltsz == 0:
            raise Exception(f"Cannot determine element size for text-section region: {name}")
        if nbytes % eltsz != 0: raise Exception(f"Expected text-section region to be a multiple of {eltsz}: {name}")
        array_len = nbytes // eltsz

        ## use it or verify
        if self.typ.array_len is not None:
            if self.typ.array_len != array_len: raise Exception(f"Misconfiguration: config specified {self.typ.array_len} elements, but region contains {array_len}: {name}")
        self.typ.array_len = array_len

CALLSTACK_CONF_VALID_TYPES = { 'HANDLER', 'IGNORE_ADDR', 'JUMPRET', }

class Struct:
    def __init__(self, name, size, members):
        self.name = name
        self.size = size
        self.members = members

        if not self.name.endswith('_t'):
            raise Exception(f'Struct names should end with _t: {name}')
        if not isinstance(size, int) or size <= 0 or size > 0xffff:
            raise Exception(f'Struct size must fit a non-empty 16-bit guest layout: {name} ({size})')

        if name in _TypeSizes:
            raise Exception(f'Type name has already been defined: {name}')
        _TypeSizes[name] = size

        ## validate
        off = 0
        for mbr in self.members:
            if mbr.off > off:
                raise Exception(f'Skipped bytes range {off}-{mbr.off} in struct "{name}"')
            if mbr.off < off:
                raise Exception(f'Overlapping byte range {mbr.off}-{off} in struct "{name}"')
            sz = mbr.size_in_bytes()
            if sz is None:
                raise Exception(f'Member in struct has no known size: {name}.{mbr.name}')
            off += mbr.size_in_bytes()

        if off != self.size:
            raise Exception(f'Size mismtch: struct size is {self.size} but members use {off} bytes')

    def struct_name(self):
        return self.name[:-2]

class Member:
    def __init__(self, name, typ, off):
        self.name = name
        self.typ  = Type.from_str(typ)
        self.off  = off

    def size_in_bytes(self):
        return self.typ.size_in_bytes()

class CallstackConf:
    def __init__(self, name, typ, addr):
        if typ not in CALLSTACK_CONF_VALID_TYPES:
            raise Exception(f'Not a valid callstack conf type: {typ}')
        self.name = name
        self.typ  = typ
        self.addr = Addr(addr)

class CodeSegment:
    def __init__(self, seg, name):
        self.seg = seg
        self.name = name
