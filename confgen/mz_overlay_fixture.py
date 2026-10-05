"""Annotations matching the deterministic MZ/FBOV smoke executable."""

from hydra.annotations import CodeSegment, Function

CodeSegments = [CodeSegment(0, "mz_overlay_fixture")]
Functions = [
    Function(
        reimpl=False,
        name="F_mz_ovlhook",
        ret="u16",
        args=0,
        start_addr="overlay_0000:0000",
        end_addr="overlay_0000:0004",
        entry="0000:0020",
    ),
]
Structures = []
DataSection = []
TextSection = []
Callstack = []
