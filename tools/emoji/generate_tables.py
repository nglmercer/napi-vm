"""Generate pinned RGI emoji tables for the regex engine.

The RGI_Emoji_* string properties enumerate Unicode sequences, so they are
generated from test data instead of hand-written. Reproduce with:

    python3 tools/emoji/generate_tables.py <emoji-test.txt> > \
        crates/napi-vm-core/src/regex/emoji_tables.rs

Provenance of the committed tables:
- Source: emoji-test.txt @ latest/ (version 18.0), filtered to entries
  with E-version <= 17.0, matching the "Unicode v17.0.0" corpus metadata of
  the generated test262 property-escape tests.
- RGI definition: fully-qualified union component sequences, per the
  emoji-test.txt header (ED-27/ED-28 in UTS #51).
- Emoji_Keycap_Sequence is a fixed UTS#51 formula kept in regex/parse.rs
  and is not tabulated here.
- Single-unit entries are emitted as scalar lists; only multi-unit
  sequences become string tables. Sequences within each table are
  longest-first so greedy matching tries longer alternatives first.

Validation: python3 tools/emoji/check_rgi_sets.py must report every set
exact (missing=0, bad_extra=0); the pinned counts below must also match.
"""
import re
import sys

HEADER = """//! Generated RGI emoji tables. DO NOT EDIT.
//!
//! Produced by `python3 tools/emoji/generate_tables.py`; see that script
//! for provenance. Validated exact against the pinned test262
//! property-escape string tests by `tools/emoji/check_rgi_sets.py`.

#![allow(clippy::all)]
"""

EXPECTED = {
    "RGI_EMOJI_ZWJ_SEQUENCE": 1614,
    "RGI_EMOJI_FLAG_SEQUENCE": 259,
    "RGI_EMOJI_TAG_SEQUENCE": 3,
    "RGI_EMOJI_MODIFIER_SEQUENCE": 665,
    "RGI_EMOJI_SINGLES": 1400 - 207,
    "BASIC_EMOJI_PAIRS": 207,
}


def to_units(cp):
    if cp > 0xFFFF:
        cp -= 0x10000
        return (0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3FF))
    return (cp,)


def units(cps):
    return tuple(u for cp in cps for u in to_units(cp))


def load(path, max_version):
    singles, seqs = set(), set()
    for line in open(path, encoding="utf-8"):
        m = re.match(
            r"([0-9A-F ]+)\s*;\s*(fully-qualified|component)\s+#\s+\S+\s+E([0-9.]+)",
            line,
        )
        if not m:
            continue
        if tuple(int(x) for x in m.group(3).split(".")) > max_version:
            continue
        cps = tuple(int(cp, 16) for cp in m.group(1).split())
        (singles if len(cps) == 1 else seqs).add(cps)
    return singles, seqs


def emit_table(name, sequences, out):
    ordered = sorted((units(s) for s in sequences), key=lambda s: (-len(s), s))
    assert len(ordered) == EXPECTED[name], (name, len(ordered))
    out.append(f"pub const {name}: &[&[u16]] = &[")
    for seq in ordered:
        out.append("    &[" + ", ".join(f"0x{u:04X}" for u in seq) + "],")
    out.append("];\n")


def emit_scalars(name, points, out):
    ordered = sorted(points)
    assert len(ordered) == EXPECTED[name], (name, len(ordered))
    out.append(f"pub const {name}: &[u32] = &[")
    line = ""
    for cp in ordered:
        chunk = f"0x{cp:04X}, "
        if len(line) + len(chunk) > 96:
            out.append("    " + line)
            line = chunk
        else:
            line += chunk
    out.append("    " + line)
    out.append("];\n")


def main():
    singles, seqs = load(sys.argv[1], (17, 0))
    out = [HEADER]
    zwj = {s for s in seqs if 0x200D in s}
    emit_table("RGI_EMOJI_ZWJ_SEQUENCE", zwj, out)
    flag = {
        s
        for s in seqs
        if len(s) == 2 and all(0x1F1E6 <= cp <= 0x1F1FF for cp in s)
    }
    emit_table("RGI_EMOJI_FLAG_SEQUENCE", flag, out)
    tag = {s for s in seqs if s[-1] == 0xE007F}
    emit_table("RGI_EMOJI_TAG_SEQUENCE", tag, out)
    modifier = {
        s
        for s in seqs
        if len(s) == 2 and s[1] in {0x1F3FB + i for i in range(5)} and 0x200D not in s
    }
    emit_table("RGI_EMOJI_MODIFIER_SEQUENCE", modifier, out)
    pairs = {s for s in seqs if len(s) == 2 and s[1] == 0xFE0F}
    emit_table("BASIC_EMOJI_PAIRS", pairs, out)
    emit_scalars("RGI_EMOJI_SINGLES", {cps[0] for cps in singles}, out)
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
