# Maintenance: build, publishing, dependency advisories

## Build

The workspace `Cargo.lock` is versioned. Builds and checks use it as is:

```bash
cargo build -p bricks-cli --locked --release
cargo test --workspace --locked
```

`--locked` fixes the resolved versions; it does not make two binaries built
on different machines identical byte for byte. `examples/benchmark` is
outside the workspace: it resolves its own lock locally (ignored by Git) and
is built from its folder (`cargo build --release`).

HTTP clients (providers, web, MCP, embeddings) come from
`cersei_types::http`. They use rustls with the bundled WebPKI roots, and
the Hickory resolver in Rust, not the system's `getaddrinfo`, so a static
musl binary resolves names without libc's NSS. The resolver reads the system
configuration. On macOS, Hickory 0.26 refuses a scoped `fe80::…%en0` server
in `/etc/resolv.conf`: those entries are skipped and the other servers kept
(a warning is logged). No public DNS server is ever added silently. SSRF
filtering (`cersei-web`) checks the addresses of the same resolution.

Linux musl builds were not checked on the development machine (no musl
target installed). The command to run where the targets and a musl linker
exist:

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --locked --release -p bricks-cli --target x86_64-unknown-linux-musl
cargo build --locked --release -p bricks-cli --target aarch64-unknown-linux-musl
```

## Publishing to crates.io

`sync_cargo.sh` publishes an explicit list of library crates in dependency
order and skips versions already on crates.io. Before anything else,
`scripts/check_publish_order.py` checks the list against `cargo metadata`:
an unknown or private crate, a duplicate, a missing internal dependency
(normal, build or optional) or a wrong order stops the script.
`scripts/test_sync_cargo.sh` checks the script with fake `cargo` and `curl`
commands. It publishes nothing and reaches no network.

```bash
./sync_cargo.sh --dry-run          # cargo publish --dry-run per crate, no upload
bash scripts/test_sync_cargo.sh
```

A dry run of a crate whose internal dependencies are not on crates.io yet at
that version fails at `cargo publish --dry-run` (the registry cannot resolve
them). This is expected before the first real publication of a version.

`bricks-cli`, `cersei-testkit` and `longmem-bench` are not published by the
script. `bricks-skills` (formerly `cersei-skills`, renamed
in Sprint 12) is published before `cersei-tools`, which depends on it; the
former name stays on crates.io at its last version and is not updated.

## Pre-commit hook (optional)

`scripts/hooks/pre-commit` makes three quick checks of what is staged:
`git diff --cached --check` (whitespace errors, conflict markers), rustfmt
in check mode on the staged `.rs` files, and `scripts/check_secrets.py
--staged` (values never printed; the staged allowlist only). It judges the
**index**, not the working tree: it exports the staged content through a
copy of the index file, so a partially staged file is checked as it will be
committed, and the real index, the working tree and the files are never
modified. It runs no build, Clippy, test or audit — the full validation
stays as it is.

Nothing is installed automatically:

```bash
scripts/hooks/install.sh status
scripts/hooks/install.sh enable     # links .git/hooks/pre-commit to the script
scripts/hooks/install.sh disable    # removes that link only
```

An existing pre-commit hook is never replaced or removed, and
`core.hooksPath` is never changed (when it is set, call the script from your
own hook). `git commit --no-verify` bypasses it once. A missing tool
(`rustfmt`, `python3`) is reported, never installed. Tested by
`scripts/test_pre_commit.sh` in temporary repositories.

## Secrets

`scripts/check_secrets.py --staged` (before a commit) or `--tracked` reports
key-shaped values without printing them. `docs/secrets-history.md` describes
what the history contains and the rotation and purge steps; neither has been
carried out.

## Dependency advisories

`cargo audit` on 2026-10-09 (Sprint 11.1), before and after:

| advisory | package | kind | chain | before | after |
|---|---|---|---|---|---|
| RUSTSEC-2026-0118 | hickory-proto 0.25.2 | vulnerability (DNSSEC validation) | reqwest 0.12 `hickory-dns` feature; cersei-web | present | **removed**: Hickory 0.26.3 |
| RUSTSEC-2026-0119 | hickory-proto 0.25.2 | vulnerability (name compression cost when encoding) | same | present | **removed**: Hickory 0.26.3 |
| RUSTSEC-2026-0002 | lru 0.12.5 | unsound | tantivy 0.22.1 → cersei-tools | present | **removed**: tantivy 0.26.2 uses lru 0.16.4 (patched ≥ 0.16.3) |
| RUSTSEC-2026-0253 | lru 0.12.5 → 0.16.4 | unsound | tantivy → cersei-tools | present | **residual**, see below |
| RUSTSEC-2024-0384 | instant | unmaintained | notify 6 / measure_time 0.8 (tantivy 0.22) | present | **removed**: notify 8, tantivy 0.26 |
| RUSTSEC-2017-0008 | serial | unmaintained | portable-pty 0.8 (optional `pty` feature) | present | **removed**: portable-pty 0.9 |
| RUSTSEC-2025-0141 | bincode 2.0.1 | unmaintained | grafeo 0.5.44 → cersei-memory | present | **residual** |
| RUSTSEC-2025-0057 | fxhash 0.2.1 | unmaintained | bm25 2.3.2 → cersei-web | present | **residual** |
| RUSTSEC-2024-0436 | paste 1.0.15 | unmaintained | tikv-jemalloc-ctl 0.6 → cersei-agent, feature `jemalloc-bench` only | present | **residual** |

Before: 9 advisories (2 vulnerabilities, 2 unsound, 5 unmaintained). After: 4
(0 vulnerabilities, 1 unsound, 3 unmaintained).

Resolved versions of the DNS/HTTP stack: reqwest 0.12.28 (without its
`hickory-dns` feature), hickory-resolver, hickory-proto and hickory-net 0.26.3,
injected through `reqwest::dns::Resolve`. reqwest 0.13 was not adopted: its
`rustls` feature switches to aws-lc-rs and the platform verifier, without
the WebPKI roots — a change of TLS contract beyond this cleanup.

### Dependency update of Sprint 12.5 (2026-10-11)

A simulation (`cargo update --dry-run`) proposed 22 changes. Applied, by
`cargo update -p …`, in two groups, the lockfile diff inspected after each
(exactly these packages changed, nothing transitive):

| package | before | after | why |
|---|---|---|---|
| `uuid` | 1.27.0 | 1.28.0 | compatible release |
| `serde_json` | 1.0.151 | 1.0.152 | patch |
| `tokio-util` | 0.7.19 | 0.7.20 | patch |
| `smallvec` | 1.16.2 | 1.16.3 | patch |
| `indoc` | 2.0.7 | 2.0.8 | patch (proc-macro) |
| `zstd-sys` | 2.1.0+zstd.1.5.7 | 2.1.1+zstd.1.5.7 | binding patch, same zstd |
| `cc` | 1.6.0 | 1.7.0 | native build tool |
| `cxx`, `cxx-build`, `cxxbridge-cmd`, `cxxbridge-flags`, `cxxbridge-macro` | 1.0.202 | 1.0.205 | updated together (one bridge version); `usearch` unchanged |

Left out on purpose:

* `mac_address` 1.2.1 — pulls `nix` 0.30 → 0.31, a migration this sprint
  excludes.
* `tree-sitter-language` 0.1.9 — Tree-sitter components stay together.
* `syn` 3.0.7 — the version in place is kept.
* `pest` 2.9.3 — adds two new native crates, `psm` and `stacker`.

`clap` 4.6.7, `rmcp` 3.5.1, `tokio` 1.53.2 and `usearch` 2.26.4 were already
current. reqwest/TLS/DNS, Tree-sitter, `nix`, `toml`, `dirs`, `schemars`,
`syn` and `usearch` were not migrated.

`cargo audit` (cargo-audit 0.22.2, advisory database of 2026-10-09, 1296
advisories, 585 dependencies), before and after the update: **0
vulnerabilities**, and the same four advisories — `lru` 0.16.4
RUSTSEC-2026-0253 (unsound), `bincode` 2.0.1 RUSTSEC-2025-0141, `fxhash`
0.2.1 RUSTSEC-2025-0057 and `paste` 1.0.15 RUSTSEC-2024-0436 (unmaintained).
No advisory is ignored. Zero vulnerabilities is not zero advisories, nor a
general guarantee.

### Residuals

**RUSTSEC-2026-0253, lru 0.16.4 (unsound).** `LruCache::pop()` is not
panic-safe when the `Drop` of a stored key panics and the panic is caught;
fixed in lru ≥ 0.18.2. tantivy 0.26.2 (the latest) depends on `lru ^0.16`, so
the patched version cannot be selected without a fork. In tantivy the only
cache is `LruCache<usize, Block>` (`store/reader.rs`): a `usize` key cannot
panic in `Drop`, so the condition cannot happen in Bricks. No fork; to revisit
when tantivy moves to lru 0.18.

**bincode 2.0.1 (unmaintained)** through grafeo 0.5.44, the latest; no
replacement upstream yet. **fxhash 0.2.1 (unmaintained)** through bm25 2.3.2,
the latest. **paste 1.0.15 (unmaintained)** through tikv-jemalloc-ctl, still
used by its 0.7; compiled only with the benchmark feature `jemalloc-bench`
(a proc-macro, absent from the shipped binary). None of these is a known
vulnerability; they are tracked here until their parent crates move.
