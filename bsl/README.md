BSL: Barebones Specification Language

BSL is a small nested-table format: `name { key value ... child { ... } }`.
Dis86 configurations use a top-level `dis86 { ... }` table.

Function entries live in `dis86.functions`. Each has a `start` address, an
optional `end`, a `mode`, and `ret`/`args` annotations. To describe a body split
across disjoint ranges, add a `regions` child table:

```
    F_split {
      start 0000:0100 end ""
      mode near ret u16 args 0
      regions {
        _0 { start 0000:0100 end 0000:0120 }
        _1 { start 0000:0180 end 0000:01a0 }
      }
    }
```

Region starts are inclusive, ends are exclusive, and every region must stay in
one segment without overlapping another. The function `start` must lie in a
region. With `regions` present, `end` may be omitted or empty (`end ""`), which
derives the outer bound from the highest region end; an explicit `end` is an
upper bound. Bytes in the gaps are
not decoded, and a branch into a gap is reported and not followed. A `regions`
table must not be empty, indirect-call entries cannot declare one, and regions
may overlap another function's regions because each function decodes its own
set independently. See the top-level [`README.md`](../README.md) for the full
behavior.


## Text sections: typed tables and data holes

`dis86.text_section` describes data that lives inside a code segment. A region
with a `type` is a typed table (today, a jump table) and keeps the `access`
annotation:

```
    jt_main {
      start 0000:1300 end 0000:1320
      type u16[16] access 0000:1400
    }
```

A region with no `type` is a data hole: a negative code hint. Holes use the
same half-open `[start, end)` shape, must stay inside one code segment, and
must not declare `access`:

```
    banner_a { start 0000:1234 end 0000:1280 }
```

Holes subtract from function code in `--name`, `--codeseg-name`, and
`--analyze`. No instruction may start in or straddle a hole, and a function
must not start inside one. Fallthrough into a hole and a direct branch or call
target inside a hole are errors. A function range fully covered by a hole is
also an error.

`--analyze` does not follow calls or branches into data. Call targets in data
appear as `# CALL TARGET IN DATA` lines in the annotation output. Code that
starts immediately after an untyped hole without being a branch target or the
function entry gets a stderr warning, because it may still be reached
indirectly. Declared holes also stop false `# UNDECODABLE GAP` reports.

`--analyze` writes code/data/unknown coverage per code segment to stderr, so
stdout remains a pasteable annotation fragment. Holes count as annotated data;
unknown bytes are the part of the segment outside configured function regions
and text-section data.
