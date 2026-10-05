#!/usr/bin/env python3
"""Regression test for metadata emitted for an MZ overlay-entry annotation."""

import io
from pathlib import Path

from hydra.gen import load
from hydra.gen.appdata import gen_src


def main():
    # Use the same annotation source as the live dosemu2 smoke library.
    data = load.annotations(str(Path(__file__).with_name("mz_overlay_fixture.py")))
    out = io.StringIO()
    gen_src(data, out)
    generated = out.getvalue()

    expected = (
        '"F_mz_ovlhook",                {{ 0, 0x0000, 0x0020 }}',
        '"F_mz_ovlhook_OVERLAY",        {{ 1, 0x0000, 0x0000 }}',
    )
    for fragment in expected:
        if fragment not in generated:
            raise AssertionError(f"missing generated metadata fragment: {fragment!r}\n{generated}")


if __name__ == "__main__":
    main()
