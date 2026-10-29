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
