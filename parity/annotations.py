"""Annotations for the #96 parity fixture image (parity/fixture.asm).

The fixture is two functions in one code segment. The entry function
[0x0000, 0x01CA) is the observable surface (outbox at module 0x0200, park
sentinel 0x0260, scratch 0x0270+; all outside the annotated range so the
linear decoder never treats their bytes as instructions). It ends with a
near call into the one-instruction leaf [0x01CA, 0x01CE), which returns AX.
A raw `jmp $` stall sits at module 0x01CE, outside both functions, so the
fixture defines its own landing address without the decoder ever seeing it.

Two annotations (not one) are required: Hydra resolves hooks by exact entry
address, and the emitted C lowers the call through the generated callstub
(`F_parity_leaf(m)` -> hydra_impl_call_near_off(0x1CA)), which dispatches to
the registered F_parity_leaf hook. A single function would leave the call
target unregistered and the caller would block forever.

Self-contained by design: the entry function sets its own ds/es/ss/sp and
reads no preloaded data.

The `end` offsets are exclusive and pinned to the last instruction byte
(entry's `ret` at module 0x01C9 -> end 0x01CA; leaf's `ret` at module
0x01CD -> end 0x01CE); when the fixture changes, derive the new values from
`ndisasm -b16` on the module bytes and update this comment.
"""

from hydra.annotations import CodeSegment, Function

CodeSegments = [CodeSegment(0, "parity_fixture")]
Functions = [
    Function(
        reimpl=True,
        name="F_parity_main",
        ret="u16",
        args=0,
        start_addr="0000:0000",
        end_addr="0000:01CA",
        flags="NEAR",
    ),
    Function(
        reimpl=True,
        name="F_parity_leaf",
        ret="u16",
        args=0,
        start_addr="0000:01CA",
        end_addr="0000:01CE",
        flags="NEAR",
    ),
]
Structures = []
DataSection = []
TextSection = []
Callstack = []
