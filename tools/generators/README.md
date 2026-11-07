# Regenerate language data tables

These Rust tools maintain committed Unicode, CLDR, and IANA tables used by the
core. Normal builds consume the committed data and do not run the generators.
Some tools fetch pinned network inputs; others require files you provide.

## Select a generator

| Binary | Inputs | Output |
| --- | --- | --- |
| `tz-data` | Pinned IANA tzdata archive, cached after download | Primary timezone/link/name table |
| `number-data` | CLDR 48.0.0 `numbers-48.tgz`, `units-48.tgz`, `core-48.tgz` | Compressed number/unit patterns, index, provenance, license |
| `emoji-tables` | `emoji-test.txt`, keeping entries through Emoji 17.0 | Committed regex emoji tables |
| `locale-data` | Pinned CLDR release files fetched over HTTPS | Locale/display data with source hashes |
| `rgi-check` | Emoji test/data files and Test262 property-string lists | Validation only; no generated output |

Consult each binary's source header for the exact pinned release and expected
input names. Updating a data release requires reviewing code/tests and
provenance together; do not substitute newer files under an old release label.

## Build and invoke

From the repository root:

```sh
cargo build --release --locked -p napi-vm-generators
```

The invocation shapes are:

```text
target/release/tz-data CACHE_DIR OUTPUT_DIR
target/release/number-data ARCHIVE_DIR OUTPUT_DIR
target/release/emoji-tables EMOJI_TEST_FILE OUTPUT_RS
target/release/locale-data OUTPUT_RS
target/release/rgi-check EMOJI_TEST_FILE EMOJI_DATA_FILE TEST262_STRINGS_DIR
```

Core output owners are `builtins/intl/cldr/`, `builtins/intl/data.rs`, and
`regex/emoji_tables.rs`. Generate into a reviewable location or preserve the
prior bytes before replacing committed outputs. Keep the CLDR license and
source manifests with the generated data.

## Verify generated data

Run `cargo test -p napi-vm-generators`, inspect source/archive hashes and table
counts, then run the affected builtin/regex and Test262 checks. Emoji and locale
tools format their Rust output. Number patterns use deterministic compression;
changes in compressor/settings are part of the byte-level output provenance.

`rgi-check` compares generated definitions with the selected corpus. Its singles
derive from the same emoji input as generation, so it is not an independent
verification of that input file. Separate data provenance from successful
comparison against a consumer corpus.

See [development](../../docs/development.md) and
[conformance execution](../test262/README.md).
