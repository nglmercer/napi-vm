"""Validate candidate RGI emoji sets against test262 match/non-match lists.

Usage: python3 tools/emoji/check_rgi_sets.py
Exits nonzero with a diff summary when a candidate mismatches.
"""
import re
import sys

CORPUS = ".napi-vm/test262/test/built-ins/RegExp/property-escapes/generated/strings"


def utf16_units(s):
    out = []
    for ch in s:
        cp = ord(ch)
        if cp > 0xFFFF:
            cp -= 0x10000
            out += [0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3FF)]
        else:
            out.append(cp)
    return tuple(out)


def load_case(name):
    src = open(f"{CORPUS}/{name}.js", encoding="utf-8").read()
    match = re.search(r"matchStrings: \[(.*?)\],", src, re.S).group(1)
    non = re.search(r"nonMatchStrings: \[(.*?)\]", src, re.S).group(1)

    def decode(blob):
        return set(
            utf16_units(js_decode(m)) for m in re.findall(r'"((?:[^"\\]|\\.)*)"', blob)
        )

    def js_decode(m):
        out = []
        i = 0
        while i < len(m):
            c = m[i]
            if c != "\\":
                out.append(c)
                i += 1
                continue
            e = m[i + 1]
            if e == "u":
                if m[i + 2] == "{":
                    j = m.index("}", i)
                    out.append(chr(int(m[i + 3 : j], 16)))
                    i = j + 1
                else:
                    out.append(chr(int(m[i + 2 : i + 6], 16)))
                    i += 6
            elif e == "n":
                out.append("\n")
                i += 2
            elif e == "x":
                out.append(chr(int(m[i + 2 : i + 4], 16)))
                i += 4
            else:
                out.append(e)
                i += 2
        return "".join(out)

    return decode(match), decode(non)


def parse_ranges(path, prop):
    """Property -> sorted list of code points from emoji-data.txt."""
    points = set()
    for line in open(path, encoding="utf-8"):
        m = re.match(r"([0-9A-F]+)(?:\.\.([0-9A-F]+))?\s*;\s*(\w+)", line)
        if not m or m.group(3) != prop:
            continue
        lo = int(m.group(1), 16)
        hi = int(m.group(2), 16) if m.group(2) else lo
        points.update(range(lo, hi + 1))
    return points


def to_units(cp):
    if cp > 0xFFFF:
        cp -= 0x10000
        return (0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3FF))
    return (cp,)


def fully_qualified_sequences(path, max_version=None):
    seqs, singles = set(), set()
    for line in open(path, encoding="utf-8"):
        m = re.match(
            r"([0-9A-F ]+)\s*;\s*(fully-qualified|component)\s+#\s+\S+\s+E([0-9.]+)",
            line,
        )
        if not m:
            continue
        if max_version is not None:
            ver = tuple(int(x) for x in m.group(3).split("."))
            if ver > max_version:
                continue
        cps = tuple(int(cp, 16) for cp in m.group(1).split())
        (singles if len(cps) == 1 else seqs).add(cps)
    return singles, seqs


def check(name, candidate):
    match, non = load_case(name)
    missing = match - candidate
    extra = candidate & non
    extra_match = candidate - match - non
    print(f"{name}: match={len(match)} candidate={len(candidate)} "
          f"missing={len(missing)} bad_extra={len(extra)} unlisted={len(extra_match)}")
    for s in sorted(missing)[:5]:
        print("  MISSING:", " ".join(f"{u:04X}" for u in s))
    for s in sorted(extra)[:5]:
        print("  BAD EXTRA:", " ".join(f"{u:04X}" for u in s))
    return not missing and not extra


def main():
    ok = True
    keycap = {tuple([ord(b), 0xFE0F, 0x20E3]) for b in "0123456789#*"}
    ok &= check("Emoji_Keycap_Sequence", keycap)

    singles16, seqs16 = fully_qualified_sequences("/tmp/emoji-test-latest.txt", (17, 0))

    def units(cps):
        return tuple(u for cp in cps for u in to_units(cp))

    zwj = {units(s) for s in seqs16 if 0x200D in s}
    flag = {
        units(s)
        for s in seqs16
        if len(s) == 2 and all(0x1F1E6 <= cp <= 0x1F1FF for cp in s)
    }
    tag = {units(s) for s in seqs16 if s[-1] == 0xE007F}
    ok &= check("RGI_Emoji_ZWJ_Sequence", zwj)
    ok &= check("RGI_Emoji_Flag_Sequence", flag)
    ok &= check("RGI_Emoji_Tag_Sequence", tag)

    modseq = {
        units(s)
        for s in seqs16
        if len(s) == 2
        and s[1] in {0x1F3FB + i for i in range(5)}
        and 0x200D not in s
    }
    ok &= check("RGI_Emoji_Modifier_Sequence", modseq)

    emoji = parse_ranges("/tmp/emoji-data-17.txt", "Emoji")
    component = parse_ranges("/tmp/emoji-data-17.txt", "Emoji_Component")
    modifier = parse_ranges("/tmp/emoji-data-17.txt", "Emoji_Modifier")
    presentation = parse_ranges("/tmp/emoji-data-17.txt", "Emoji_Presentation")
    basic = set()
    for c in emoji - component - modifier:
        if c in presentation:
            basic.add(units((c,)))
        else:
            basic.add(units((c,)) + (0xFE0F,))
    ok &= check("Basic_Emoji", basic)

    singles18, _ = fully_qualified_sequences("/tmp/emoji-test-latest.txt", (17, 0))
    rgi_singles = {units(s) for s in singles18}
    rgi = set()
    rgi |= flag | tag | modseq | keycap | zwj | rgi_singles
    ok &= check("RGI_Emoji", rgi)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
