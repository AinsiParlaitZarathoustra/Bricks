# Maintenance: build, publishing, dependency advisories

## Build

The workspace `Cargo.lock` is versioned. Builds and tests use it as it is:

```bash
cargo build -p bricks-cli --locked --release
cargo test --workspace --locked
```

`--locked` fixes the resolved versions. It does not make binaries built on
different machines identical byte for byte. The toolchain is pinned to Rust
1.99.1 (`rust-toolchain.toml`).

`examples/benchmark` is outside the workspace. It resolves its own lock
locally (ignored by Git) and is built from its own folder
(`cargo build --release`).

HTTP clients (providers, web, MCP, embeddings) come from `bricks_types::http`.
They use rustls with the bundled WebPKI roots and the Hickory resolver written
in Rust, not the system `getaddrinfo`. A static musl binary can therefore
resolve names without libc's NSS. The resolver reads the system DNS
configuration. On macOS, Hickory refuses a scoped `fe80::...%en0` server in
`/etc/resolv.conf`. Bricks skips those entries, keeps the other servers and
logs a warning. It never adds a public DNS server on its own. The web safety
checks (`bricks-web`) use the same DNS answers.

Linux musl builds were not checked on the development machine. To try them
where the targets and a musl linker exist:

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --locked --release -p bricks-cli --target x86_64-unknown-linux-musl
cargo build --locked --release -p bricks-cli --target aarch64-unknown-linux-musl
```

## Publishing to crates.io

`sync_cargo.sh` publishes a fixed list of library crates, in dependency order.
It skips versions already on crates.io. Flags: `--dry-run` and
`--allow-dirty`. Log in with `cargo login` or set `CARGO_REGISTRY_TOKEN`.

Before anything else, `scripts/check_publish_order.py` checks the list against
`cargo metadata`. An unknown or private crate, a duplicate, a missing internal
dependency or a wrong order stops the script. `scripts/test_sync_cargo.sh`
tests the script with fake `cargo` and `curl` commands. It publishes nothing
and uses no network.

```bash
./sync_cargo.sh --dry-run          # cargo publish --dry-run per crate, no upload
bash scripts/test_sync_cargo.sh
```

A dry run fails for a crate whose internal dependencies are not yet on
crates.io at that version. This is expected before the first real
publication of a version.

`bricks-cli` and `bricks-testkit` are not published by the script.
`bricks-skills` is published before `bricks-tools`, which depends on it. The
`bricks` crate is last.

## Pre-commit hook (optional)

`scripts/hooks/pre-commit` makes three quick checks on what is staged:

* `git diff --cached --check` (whitespace errors, conflict markers),
* rustfmt in check mode on the staged `.rs` files,
* `scripts/check_secrets.py --staged` (values are never printed).

It checks the index, not the working tree, so a partly staged file is checked
as it will be committed. It never changes the index or your files. It runs no
build, Clippy, tests or audit.

Nothing is installed automatically:

```bash
scripts/hooks/install.sh status
scripts/hooks/install.sh enable     # links .git/hooks/pre-commit to the script
scripts/hooks/install.sh disable    # removes that link only
```

An existing pre-commit hook is never replaced or removed. `core.hooksPath` is
never changed (if it is set, call the script from your own hook).
`git commit --no-verify` skips the hook once. A missing tool (`rustfmt`,
`python3`) is reported, never installed. `scripts/test_pre_commit.sh` tests
the hook in temporary repositories.

## Secrets

`scripts/check_secrets.py --staged` (before a commit) or `--tracked` reports
values shaped like keys, without printing them. Known fake values in tests are
listed in `scripts/secret-allowlist.txt`.
[secrets-history.md](secrets-history.md) describes what the Git history
contains, with the rotation and purge steps. Neither has been carried out.

## Dependency advisories

Check for known problems with:

```bash
cargo install cargo-audit
cargo audit
```

Rules we follow:

* No advisory is ignored. Zero vulnerabilities is not zero advisories, and it
  is not a general guarantee.
* A vulnerability is fixed by updating the dependency. If the fix needs a
  newer parent crate, we wait for it or replace the parent. We do not fork.
* An "unmaintained" or "unsound" notice is checked by hand: is the code path
  reachable in Bricks? The answer is written down until the parent crate
  moves.
* Optional or benchmark-only dependencies (for example the `jemalloc-bench`
  feature of `bricks-agent`) are not in the shipped binary.

To update dependencies, run `cargo update --dry-run` first, then update
packages one group at a time with `cargo update -p <name>`. Check the lock
file diff after each group. Do not mix a version update with a migration
(reqwest, TLS, DNS, Tree-sitter, `nix`, `toml`, `dirs`, `schemars`, `syn`,
`usearch`).

The DNS and HTTP stack uses reqwest 0.12 without its `hickory-dns` feature.
Hickory 0.26 is plugged in through `reqwest::dns::Resolve`. The workspace
keeps the WebPKI roots (`rustls-tls-webpki-roots`) on purpose.

Advisories known to remain unfixed at the last check, all unmaintained
crates and not known vulnerabilities: `bincode` (through `grafeo`), `fxhash`
(through `bm25`) and `paste` (through `tikv-jemalloc-ctl`, benchmark feature
only). Run `cargo audit` for the current list.
