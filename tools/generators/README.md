# Table generators

Offline Rust generators for the Unicode, CLDR and IANA tables committed under
`crates/napi-vm-core`. They are not part of validation or the build; run them
only when a pinned upstream release changes. Each one reproduces the committed
output exactly, so a regeneration that changes nothing confirms the pins.

| Binary | Reads | Writes | Check after running |
| --- | --- | --- | --- |
| `tz-data` | IANA tzdata 2025b tarball (downloaded into a cache directory) | `builtins/intl/cldr/timezone-names.rs` | the generator's own primary-identifier self-check |
| `number-data` | cldr-numbers-full, cldr-units-full and cldr-core 48.0.0 npm tarballs (renamed `numbers-48.tgz`, `units-48.tgz`, `core-48.tgz`) | `builtins/intl/cldr/number-pattern-*`, `LICENSE-CLDR.txt` | archive SHA-256 values in `number-pattern-sources.json` |
| `emoji-tables` | `emoji-test.txt` (keeps entries up to Emoji 17.0) | `regex/emoji_tables.rs` | pinned entry counts per table |
| `locale-data` | CLDR 48 release files over HTTPS | `builtins/intl/data.rs` | SHA-256 of every fetched file, listed in the output |
| `rgi-check` | `emoji-test.txt`, `emoji-data.txt` (17.0), and the corpus `test/built-ins/RegExp/property-escapes/generated/strings` directory | nothing; exits nonzero on mismatch | see below |

## Regenerating

```sh
cargo build --release -p napi-vm-generators
target/release/tz-data .napi-vm/tzdata crates/napi-vm-core/src/builtins/intl/cldr
target/release/number-data <dir-with-npm-tarballs> crates/napi-vm-core/src/builtins/intl/cldr
target/release/emoji-tables <emoji-test.txt> crates/napi-vm-core/src/regex/emoji_tables.rs
target/release/locale-data crates/napi-vm-core/src/builtins/intl/data.rs
target/release/rgi-check <emoji-test.txt> <emoji-data.txt> .napi-vm/test262/test/built-ins/RegExp/property-escapes/generated/strings
```

`emoji-tables` and `locale-data` run `rustfmt --edition 2024` on their output,
because the committed files are rustfmt output. `number-data` compresses each
locale record with zlib-rs at level 9 and `mtime` 0; the committed blob is
byte-identical to that output.

## Verified in this change

- `tz-data`, `number-data`, `emoji-tables` and `locale-data` reproduce the
  committed outputs exactly, except for the header comment that now names the
  Rust tool.
- `rgi-check` produces the same report as the Python checker it replaces.

## Known `rgi-check` mismatches

With the inputs above, `rgi-check` reports these; the Python checker reported
the same:

- `Basic_Emoji`: 9 missing, including the five skin-tone modifiers
  U+1F3FB..U+1F3FF that the report prints. The check derives the basic set
  from `Emoji - Emoji_Component - Emoji_Modifier`, which excludes those code
  points while the test262 list includes them.
- `RGI_Emoji`: 207 missing, equal to the 207 `BASIC_EMOJI_PAIRS` in the
  committed tables. The check's singles come only from single-code-point
  entries, so the qualified pairs (`00A9 FE0F` and similar) are not in its set.

The `RGI_Emoji` count matches the table size, which points at the checker's
set definitions rather than the tables. Fixing the checker would change what
it verifies, so that is a separate change.

## Unit tests

`cargo test -p napi-vm-generators` covers the JSON string encoding, the tar
reader, and gzip/SHA-256 helpers.
