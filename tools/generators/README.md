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
| `rgi-check` | `emoji-test.txt`, `emoji-data.txt` (17.0), and the corpus `test/built-ins/RegExp/property-escapes/generated/strings` directory | nothing; exits nonzero on any mismatch | 0 mismatches on the pinned corpus |

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
- `rgi-check` exits 0: every set matches its test262 list exactly.

`rgi-check` replaces the Python checker, whose set definitions were wrong: its
`Basic_Emoji` omitted the skin-tone modifiers and the other text-default
singles, and its `RGI_Emoji` left out the basic pairs. The Rust checker defines
`Basic_Emoji` as the fully-qualified single code points plus every text-default
Emoji (Emoji minus Emoji_Presentation minus Emoji_Component) followed by U+FE0F,
and `RGI_Emoji` as the union of the RGI families and that basic set. The singles
still come from `emoji-test.txt`, the same file the tables are built from, so
the check compares the tables against test262 but does not independently verify
the emoji-test singles.

## Unit tests

`cargo test -p napi-vm-generators` covers the JSON string encoding, the tar
reader, and gzip/SHA-256 helpers.
