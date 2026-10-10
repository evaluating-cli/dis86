import unittest
import io
import subprocess
import tempfile
from pathlib import Path

from hydra.annotations import Function, Global, Member, Struct, Type, validate_data_section
from hydra.gen import dis86 as dis86_gen
from hydra.gen.appdata import build_func_data, gen_hdr


class AnnotationTypeTests(unittest.TestCase):
    def test_c_dimension_order_and_packed_size(self):
        typ = Type.from_str('u16[3][5]')
        self.assertEqual(typ.dimensions, [3, 5])
        self.assertEqual(typ.size_in_bytes(), 30)
        self.assertEqual(typ.fmt_ctype_str('grid'), 'u16             grid[3][5]')

    def test_pointer_annotations_use_guest_width_storage(self):
        for name in ('near<u16>', 'near_ss<u16>', 'near_es<u16>'):
            typ = Type.from_str(name)
            self.assertEqual(typ.size_in_bytes(), 2)
            self.assertIn('u16', typ.fmt_ctype_str('ptr'))
        self.assertEqual(Type.from_str('far<u16>').size_in_bytes(), 4)

    def test_generated_struct_member_keeps_guest_pointer_alias(self):
        layout = Struct('annotation_test_ptr_holder_t', 2, [
            Member('ptr', 'near_es<u16>', 0),
        ])
        output = io.StringIO()
        gen_hdr({
            'functions': [], 'structures': [layout], 'data_section': [], 'callstack': [],
        }, out=output)
        text = output.getvalue()
        self.assertIn('typedef u16 dis86_near_es_ptr_u16;', text)
        self.assertIn('dis86_near_es_ptr_u16 ptr;', text)

    def test_generated_array_of_guest_pointers_declares_alias(self):
        layout = Struct('annotation_test_ptr_array_t', 6, [
            Member('ptrs', 'near<u16>[3]', 0),
        ])
        output = io.StringIO()
        gen_hdr({
            'functions': [], 'structures': [layout], 'data_section': [], 'callstack': [],
        }, out=output)
        text = output.getvalue()
        self.assertIn('typedef u16 dis86_near_ptr_u16;', text)
        self.assertIn('dis86_near_ptr_u16 ptrs[3];', text)

    def test_struct_layout_requires_explicit_padding(self):
        name = 'annotation_test_layout_t'
        Struct(name, 4, [Member('first', 'u16', 0), Member('second', 'u16', 2)])
        Struct('annotation_test_padding_t', 4, [
            Member('tag', 'u8', 0),
            Member('padding', 'u8[2]', 1),
            Member('value', 'u8', 3),
        ])
        with self.assertRaisesRegex(Exception, 'Skipped bytes'):
            Struct('annotation_test_gap_t', 4, [Member('first', 'u16', 0), Member('last', 'u8', 3)])

    def test_bad_types_and_unknown_bounds_fail_layout_requests(self):
        with self.assertRaisesRegex(Exception, 'Invalid type'):
            Type.from_str('u16[2')
        with self.assertRaisesRegex(Exception, 'No array length'):
            Type.from_str('u16[][4]').fmt_ctype_str('grid')
        with self.assertRaisesRegex(Exception, 'without all fixed bounds'):
            validate_data_section([Global('unknown_bound', 'u16[]', 0)])
        with self.assertRaisesRegex(Exception, 'exceeds the 64 KiB'):
            validate_data_section([Global('past_end', 'u16[2]', 0xffff)])

    def test_type_parsing_matches_rust_parser(self):
        self.assertEqual(Type.from_str('u16 ').basetype, 'u16')
        with self.assertRaisesRegex(Exception, 'Invalid'):
            Type.from_str('near<u16[3]>')
        with self.assertRaisesRegex(Exception, 'Invalid'):
            Type.from_str('far<near<u16>>')

    def test_segmented_guest_memory_fixture(self):
        source = Path(__file__).with_name('test_guest_memory_fixture.c')
        with tempfile.TemporaryDirectory(prefix='dis86-guest-memory-') as tmp:
            exe = Path(tmp) / 'guest-memory-fixture'
            subprocess.run(['cc', '-std=c11', '-Wall', '-Werror', str(source), '-o', str(exe)], check=True)
            result = subprocess.run([str(exe)], check=True, capture_output=True, text=True)
            self.assertIn('guest memory fixture passed', result.stdout)


class RetUnknownFlagTests(unittest.TestCase):
    def _func(self, name, flags):
        return Function(reimpl=False, name=name, ret=None, args=None,
                        start_addr='0000:1234', end_addr='0000:1300', flags=flags)

    def _gen_functions_text(self, funcs):
        buf = io.StringIO()
        prev, dis86_gen.out = dis86_gen.out, buf
        try:
            dis86_gen.gen_functions(funcs)
        finally:
            dis86_gen.out = prev
        return buf.getvalue()

    def test_ret_unknown_emits_interim_near_with_marker(self):
        # Interim default: small-model near (status quo), with an inert but
        # greppable marker. Must NOT fall through to the far default, which
        # would silently flip noreturn functions from near to far.
        text = self._gen_functions_text([self._func('ret_unknown_a', 'RET_UNKNOWN')])
        self.assertIn('mode near', text)
        self.assertIn('ret_kind_unknown 1', text)

    def test_near_and_default_modes_unchanged(self):
        text = self._gen_functions_text([
            self._func('ret_unknown_near_b', 'NEAR'),
            self._func('ret_unknown_far_b', 0),
        ])
        near_line = next(l for l in text.splitlines() if 'ret_unknown_near_b' in l)
        far_line = next(l for l in text.splitlines() if 'ret_unknown_far_b' in l)
        self.assertIn('mode near', near_line)
        self.assertNotIn('ret_kind_unknown', near_line)
        self.assertIn('mode far', far_line)
        self.assertNotIn('ret_kind_unknown', far_line)

    def test_ret_unknown_callstub_maps_to_near_with_todo(self):
        # The raw flag string must never reach the C HYDRA_DEFINE_CALLSTUB
        # bitmask (it has no C macro meaning); the interim NEAR matches the
        # BSL, and the TODO marks it unresolved.
        dat = build_func_data([self._func('ret_unknown_c', 'RET_UNKNOWN')])
        self.assertEqual(dat[0].flags, 'NEAR')
        self.assertTrue(dat[0].ret_unknown)
        output = io.StringIO()
        gen_hdr({
            'functions': [self._func('ret_unknown_d', 'RET_UNKNOWN')],
            'structures': [], 'data_section': [], 'callstack': [],
        }, out=output)
        text = output.getvalue()
        stub_line = next(l for l in text.splitlines() if 'ret_unknown_d' in l and 'CALLSTUB' in l)
        self.assertIn(', NEAR', stub_line)
        self.assertNotIn('RET_UNKNOWN', stub_line)
        self.assertIn('TODO: return kind unknown', stub_line)


if __name__ == '__main__':
    unittest.main()
