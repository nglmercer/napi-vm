"""Build runtime time-zone identifier tables from a pinned IANA tzdata release.

Input: a tzdata tarball (downloaded when missing) matching the database
bundled with the chrono-tz dependency, so validity and offsets agree.
No Test262 data is consulted.

Output: timezone-names.rs with the Zone/Link universe, link targets,
zone.tab country rows, and backzone links used to compute ECMA-402
primary time zone identifiers.
"""
import json
import sys
import tarfile
import urllib.request
from pathlib import Path

VERSION = "2025b"
URL = f"https://data.iana.org/time-zones/releases/tzdata{VERSION}.tar.gz"


def parse_members(archive):
    zones = set()
    links = {}
    for entry in archive:
        if not entry.isfile():
            continue
        try:
            text = archive.extractfile(entry).read().decode("utf-8", "replace")
        except Exception:
            continue
        for line in text.splitlines():
            line = line.strip()
            parts = line.split()
            if not parts:
                continue
            if parts[0] == "Zone":
                if len(parts) >= 2:
                    zones.add(parts[1])
            elif parts[0] == "Link":
                if len(parts) >= 3:
                    links[parts[2]] = parts[1]
    return zones, links


def parse_zone_tab(archive, name):
    rows = []
    for entry in archive:
        if entry.name.split("/")[-1] != name:
            continue
        text = archive.extractfile(entry).read().decode("utf-8", "replace")
        for line in text.splitlines():
            if not line or line.startswith("#"):
                continue
            fields = line.split("\t")
            if len(fields) < 3:
                fields = line.split()
                if len(fields) < 3:
                    continue
            codes = [c.strip() for c in fields[0].split(",")]
            tz = fields[2].strip()
            for code in codes:
                if code and tz:
                    rows.append((code, tz))
    return rows


def parse_backzone(archive):
    links = {}
    for entry in archive:
        if entry.name.split("/")[-1] != "backzone":
            continue
        text = archive.extractfile(entry).read().decode("utf-8", "replace")
        for line in text.splitlines():
            stripped = line.strip()
            body = stripped
            if stripped.startswith("#PACKRATLIST zone.tab Link"):
                body = stripped[len("#PACKRATLIST zone.tab ") :]
            parts = body.split()
            if parts and parts[0] == "Link" and len(parts) >= 3:
                links.setdefault(parts[2], parts[1])
    return links


def rust_list(name, items, per_line=6):
    lines = []
    for i in range(0, len(items), per_line):
        lines.append("    " + ",".join(json.dumps(x) for x in items[i : i + per_line]) + ",")
    return f"pub(super) const {name}: &[&str] = &[\n" + "\n".join(lines) + "\n];"


def rust_pairs(name, pairs):
    lines = [f"    ({json.dumps(k)},{json.dumps(v)})," for k, v in pairs]
    return f"pub(super) const {name}: &[(&str, &str)] = &[\n" + "\n".join(lines) + "\n];"


# Country codes for Link names that appear in neither zone.tab nor
# zone1970.tab and whose territory lies in a different country than their
# link target. Verified against tzdata geography (Svalbard/Jan Mayen are
# Norwegian territory under country code SJ; Chuuk/Truk are in the
# Federated States of Micronesia, FM, while Port Moresby is in Papua New
# Guinea, PG).
LINK_COUNTRIES = {
    "Atlantic/Jan_Mayen": "SJ",
    "Pacific/Truk": "FM",
}


def country_of(name, zone1970, zone_tab, links):
    for table in (zone1970, zone_tab):
        for code, tz in table:
            if tz == name:
                return code
    if name in LINK_COUNTRIES:
        return LINK_COUNTRIES[name]
    target = links.get(name)
    if target is not None:
        return country_of(target, zone1970, zone_tab, links)
    return None


def main():
    source, output = map(Path, sys.argv[1:])
    source.mkdir(parents=True, exist_ok=True)
    archive_path = source / f"tzdata{VERSION}.tar.gz"
    if not archive_path.exists():
        print(f"downloading {URL}", file=sys.stderr)
        urllib.request.urlretrieve(URL, archive_path)
    with tarfile.open(archive_path) as archive:
        zones, links = parse_members(archive)
    with tarfile.open(archive_path) as archive:
        zone_tab = parse_zone_tab(archive, "zone.tab")
    with tarfile.open(archive_path) as archive:
        zone1970 = parse_zone_tab(archive, "zone1970.tab")
    with tarfile.open(archive_path) as archive:
        backzone = parse_backzone(archive)
    universe = sorted(zones | set(links))
    print(f"zones={len(zones)} links={len(links)} zone.tab={len(zone_tab)} zone1970={len(zone1970)} backzone={len(backzone)}", file=sys.stderr)
    for probe in ["GMT", "UTC", "Etc/UTC", "Etc/UCT", "Etc/GMT", "Etc/GMT0", "US/Eastern", "Australia/Canberra", "Factory"]:
        kind = "zone" if probe in zones else ("link" if probe in links else "absent")
        print(f"  {probe}: {kind} -> {links.get(probe)}", file=sys.stderr)
    out = f"""// Generated from IANA tzdata {VERSION} by tools/test262/generate_tz_data.py.
"""
    out += rust_list("ZONE_NAMES", sorted(zones)) + "\n"
    out += rust_pairs("LINK_TARGETS", sorted(links.items())) + "\n"
    out += rust_pairs("ZONE_TAB", zone_tab) + "\n"
    out += rust_pairs("ZONE1970_TAB", zone1970) + "\n"
    out += rust_pairs("BACKZONE_LINKS", sorted(backzone.items())) + "\n"
    out += rust_pairs("LINK_COUNTRIES", sorted(LINK_COUNTRIES.items())) + "\n"
    # Self-check: mirror the ECMA primary computation for pinned cases.
    link_map = dict(links)
    zone_set = set(zones)
    tab_tz = {tz for _, tz in zone_tab}

    def primary(identifier):
        canonical = next(
            (z for z in list(zones) + list(links) if z.lower() == identifier.lower()), None
        )
        if canonical is None:
            return None
        current, hops = canonical, 0
        while current in link_map and hops < 8:
            current, hops = link_map[current], hops + 1
        zone = current
        if canonical in link_map and canonical not in tab_tz:
            if zone.startswith("Etc/"):
                primary = zone
            else:
                cc_id = country_of(canonical, zone1970, zone_tab, link_map)
                cc_zone = country_of(zone, zone1970, zone_tab, link_map)
                if cc_id is not None and cc_id == cc_zone:
                    primary = zone
                else:
                    rows = [tz for cc, tz in zone_tab if cc == cc_id]
                    if len(rows) == 1:
                        primary = rows[0]
                    else:
                        primary = backzone.get(canonical)
        else:
            primary = canonical
        if primary in ("Etc/UTC", "Etc/GMT", "GMT"):
            primary = "UTC"
        return primary

    pinned = {
        "Europe/Prague": "Europe/Prague",
        "Europe/Bratislava": "Europe/Bratislava",
        "Australia/Canberra": "Australia/Sydney",
        "Atlantic/Jan_Mayen": "Arctic/Longyearbyen",
        "Pacific/Truk": "Pacific/Chuuk",
        "Etc/UCT": "UTC",
        "Etc/GMT0": "UTC",
        "AFRICA/ABIDJAN": "Africa/Abidjan",
        "utc": "UTC",
        "US/Eastern": "America/New_York",
    }
    bad = {k: (primary(k), v) for k, v in pinned.items() if primary(k) != v}
    assert not bad, f"primary mismatches: {bad}"
    print("primary self-check ok", file=sys.stderr)
    output.mkdir(parents=True, exist_ok=True)
    (output / "timezone-names.rs").write_text(out)
    print(f"wrote {output / 'timezone-names.rs'}", file=sys.stderr)


main()
