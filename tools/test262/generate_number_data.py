"""Extract runtime number/unit patterns from pinned Unicode CLDR npm archives.

Input archives: cldr-numbers-full and cldr-units-full 48.0.0, unmodified.
No Test262 data is consulted. Locale records are independently compressed so a
worker only decodes the locales it actually formats.
"""
import gzip
import hashlib
import json
from pathlib import Path
import sys
import tarfile

UNITS = "acre bit byte celsius centimeter day degree fahrenheit fluid-ounce foot gallon gigabit gigabyte gram hectare hour inch kilobit kilobyte kilogram kilometer liter megabit megabyte meter microsecond mile mile-scandinavian milliliter millimeter millisecond minute month nanosecond ounce percent petabyte pound second stone terabit terabyte week yard year".split()

def main():
    source, output = map(Path, sys.argv[1:])
    output.mkdir(parents=True, exist_ok=True)
    records = {}
    identities = {}
    license_text = None
    for kind in ["numbers", "units"]:
        path = source / f"{kind}-48.tgz"
        identities[kind] = hashlib.sha256(path.read_bytes()).hexdigest()
        with tarfile.open(path) as archive:
            if license_text is None:
                license_text = archive.extractfile("package/LICENSE").read()
            for entry in archive:
                pieces = entry.name.split("/")
                if len(pieces) != 4 or pieces[1] != "main":
                    continue
                locale, filename = pieces[2:]
                if filename not in ["numbers.json", "currencies.json", "units.json"]:
                    continue
                data = json.load(archive.extractfile(entry))["main"][locale]
                result = records.setdefault(locale, {})
                if filename == "numbers.json":
                    result["numbers"] = data["numbers"]
                elif filename == "currencies.json":
                    result["currencies"] = data["numbers"]["currencies"]
                else:
                    result["units"] = {}
                    for width in ["long", "short", "narrow"]:
                        selected = result["units"][width] = {}
                        for key, value in data["units"][width].items():
                            name = key.split("-", 1)[-1]
                            compound = name.split("-per-")
                            if key == "per" or (len(compound) <= 2 and all(u in UNITS for u in compound)):
                                selected[name if key != "per" else key] = value
    blob = bytearray()
    seen = {}
    index = []
    for locale, data in sorted(records.items()):
        raw = json.dumps(data, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()
        if raw not in seen:
            compressed = gzip.compress(raw, mtime=0)
            seen[raw] = (len(blob), len(blob) + len(compressed))
            blob.extend(compressed)
        start, end = seen[raw]
        index.append(f'    ("{locale}", {start}, {end}),')
    (output / "number-patterns.bin").write_bytes(blob)
    core = source / "core-48.tgz"
    identities["core"] = hashlib.sha256(core.read_bytes()).hexdigest()
    with tarfile.open(core) as archive:
        systems = json.load(archive.extractfile("package/supplemental/numberingSystems.json"))["supplemental"]["numberingSystems"]
    digits = [(key, value["_digits"]) for key, value in sorted(systems.items()) if value["_type"] == "numeric"]
    (output / "number-pattern-index.rs").write_text(
        "// Generated from Unicode CLDR 48.0.0 by tools/test262/generate_number_data.py.\n"
        + "pub(super) const INDEX: &[(&str, usize, usize)] = &[\n" + "\n".join(index) + "\n];\n"
        + "pub(super) const UNITS: &[&str] = &[" + ",".join(json.dumps(u) for u in UNITS) + "];\n"
        + "pub(super) const NUMBERING_SYSTEMS: &[(&str, &str)] = &[\n" + "\n".join("    (" + json.dumps(k) + "," + json.dumps(v, ensure_ascii=False) + ")," for k, v in digits) + "\n];\n"
    )
    (output / "LICENSE-CLDR.txt").write_bytes(license_text)
    (output / "number-pattern-sources.json").write_text(json.dumps({"cldr_version": "48.0.0", "archive_sha256": identities, "locales": len(records), "unique_records": len(seen), "blob_sha256": hashlib.sha256(blob).hexdigest()}, indent=2) + "\n")
    print(len(records), "locales;", len(seen), "records;", len(blob), "bytes")

if __name__ == "__main__":
    main()
