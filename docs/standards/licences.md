# Licences

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

These rules apply to every dependency, asset, font and piece of vendored code in this repository. `AGENTS.md` carries the short form and links here. Adapted from [storytold/craft-contrib's licence standard](https://github.com/storytold/craft-contrib/blob/main/standards/licences.md); this copy is the one that applies here.

git-archiver itself is MIT ([`LICENSE`](../../LICENSE)).

## The allowlist

Dependencies and assets use SPDX identifiers. A licence MUST be on this list before it is used. Adding an identifier is a pull request of its own that changes this list and says which dependency or asset needs it.

- `ISC`
- `MIT`
- `BSD-2-Clause`
- `BSD-3-Clause`
- `Apache-2.0`
- `Apache-2.0 WITH LLVM-exception`
- `Unicode-3.0`
- `Zlib`
- `BSL-1.0`
- `0BSD`
- `Unlicense`
- `CDLA-Permissive-2.0`
- `CC0-1.0`
- `CC-BY-4.0`
- `CC-BY-SA-4.0`
- `OFL-1.1`
- `LicenseRef-Public-Domain`
- `MPL-2.0`, under the rules below

[`deny.toml`](../../deny.toml) enforces this list for crates with `cargo deny check`. It names only the identifiers a crate uses today, and each MPL-2.0 crate by name; a pull request that needs another adds it there.

A licence expression is allowed when we can take it under listed identifiers only: for `OR`, at least one side; for `AND` and `WITH`, every part. `MIT OR Apache-2.0 OR LGPL-2.1-or-later` is allowed, taken as MIT or Apache-2.0.

## MPL-2.0

`MPL-2.0` is allowed for third-party dependencies used unmodified. `MPL-2.0+` counts as `MPL-2.0`. Its copyleft applies file by file:

- Depending on an unmodified MPL-2.0 crate needs nothing more. Our code that uses it keeps its own licence.
- You SHOULD NOT copy MPL-2.0 code into our own files: that file would become MPL-2.0. Copying, vendoring or patching MPL-2.0 code, including through a `[patch]` entry in `Cargo.toml`, SHOULD have a maintainer's approval on the pull request first.
- A pull request that adds an MPL-2.0 dependency SHOULD say whether it ships in the binary and the container image, or only in tests. Today `cssparser`, `cssparser-macros`, `selectors` and `dtoa-short` are MPL-2.0, all reached only through the dev-dependency `scraper`.
- If an MPL-2.0 crate ships, the release notes MUST say where to get its source.

## Assets

An asset is any non-code material the repository includes, serves, embeds or shows in its documentation: fonts, icons, images, stylesheets, scripts, screenshots, and bytes embedded in code (`include_bytes!`, base64, data URIs).

1. Every asset MUST be either original work by a contributor who licenses it under the repository's licence, or under a licence on the allowlist, taken from its original source with the exact version recorded.
2. Every asset MUST have a row in [`ATTRIBUTION.md`](../../ATTRIBUTION.md), added in the same change as the asset: path, source with version, licence, and how it was made or changed.
3. A licence that requires its text to travel with the asset keeps that text next to it, as `assets/fonts/*-OFL.txt` do.
4. If an asset's origin or licence is unclear, leave it out and make a replacement, or ask on the pull request.

## Fonts

1. An `OFL-1.1` font MUST come from its open-source release, not from a copy a product installed.
2. An `OFL-1.1` font with a Reserved Font Name MUST be used unmodified under that name. A subset or modified copy MUST get a new name. Inter and JetBrains Mono declare no Reserved Font Name, so their Latin subsets keep their names.
3. Fonts installed on the machine are not committed or embedded.

## Vendored and ported code

Code copied, vendored or ported from another project MUST keep its upstream copyright and licence notice and MUST get an `ATTRIBUTION.md` row with its source URL, commit and licence. Apache-2.0 code MUST keep its `NOTICE`, and changed files MUST be marked as changed.
