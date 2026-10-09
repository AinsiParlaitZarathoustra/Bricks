# Linux binaries for Terminal-Bench

The harnesses of this folder upload a static Linux binary into each task
container. These binaries are build outputs: since Sprint 11.1 they are no
longer tracked by Git (they are ignored), and each harness looks for them at
its usual path or at the path given by an environment variable.

| binary | used by | variable to point elsewhere |
|---|---|---|
| `tbench-agent-linux-amd64` | `tbench_agent.py`, `run_tb_cersei.sh` | `TBENCH_AGENT_BINARY_AMD64` |
| `tbench-agent-linux-arm64` | same | `TBENCH_AGENT_BINARY_ARM64` |
| `abstract-linux-amd64` | `abstract_tbench.py`, `run_dry_tb.sh`, `run_tb_full.sh` | `ABSTRACT_BINARY_AMD64` |
| `abstract-linux-arm64` | same | `ABSTRACT_BINARY_ARM64` |

## Build `tbench-agent` (current harness)

`tbench-agent` is the `[[bin]]` of `crates/cersei-tbench`. It must be static
(musl) to run in both Alpine and Debian containers. From the repository root,
with the lockfile:

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --release --locked -p cersei-tbench --target x86_64-unknown-linux-musl
cargo build --release --locked -p cersei-tbench --target aarch64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release/tbench-agent bench/term-bench/tbench-agent-linux-amd64
cp target/aarch64-unknown-linux-musl/release/tbench-agent bench/term-bench/tbench-agent-linux-arm64
```

Cross-compiling from macOS also needs a musl linker for each target (for
example `cargo zigbuild`, or a Linux container such as `rust:alpine`); none is
installed by these scripts. On a Linux host of the same architecture, the
first command alone is enough.

## Recover the historical binaries

The binaries tracked until Sprint 11.1 are still in the history (last
changed in commit `54999e4`). To get one back without rebuilding:

```bash
git show 54999e4:bench/term-bench/tbench-agent-linux-amd64 > bench/term-bench/tbench-agent-linux-amd64
chmod +x bench/term-bench/tbench-agent-linux-amd64
shasum -a 256 bench/term-bench/tbench-agent-linux-amd64
```

Compare the checksum with the table below before running it.

| file | bytes | SHA-256 |
|---|---|---|
| `tbench-agent-linux-amd64` | 15064528 | `c9c96ee00773b29dafce19f38aa27bb46d5335b36b3eea06d9bfcc5f4432f259` |
| `tbench-agent-linux-arm64` | 12340016 | `e17c7903de076b09c74fc3f44a0053dd43db30d091e2c5e6685f9dae1aeacc7d` |
| `abstract-linux-amd64` | 23349008 | `b9bf1971b60b8bca38672a0bfb692ad11c2b516b986c65b65676fd5c2632834b` |
| `abstract-linux-arm64` | 19846056 | `d1039d0630fe66e89934c5c1587bf21803cea7e971ffd429604cc003e3166bdc` |

## `abstract-*`: historical

`abstract` was the binary of the old `abstract-cli` crate (Cersei era); it no
longer exists in this workspace, and the exact sources and options of the two
recorded binaries are unknown. `abstract_tbench.py`, `run_dry_tb.sh`,
`run_tb_full.sh` and `runner-google.sh` (which calls `run_dry_tb.sh`) are kept
to read or reproduce old results only. A checksum identifies the recorded file;
it does not prove where it comes from. Run it only in the benchmark's
disposable containers.
