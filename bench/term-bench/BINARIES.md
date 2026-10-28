# Linux binaries for Terminal-Bench

The harnesses of this folder upload a static Linux binary into each task
container. These binaries are build outputs: since Sprint 11.1 (commit
`54079af`, 0.5.0) they are no longer tracked by Git (they are ignored), and each
harness looks for them at its usual path or at the path given by an environment
variable.

| binary | used by | variable to point elsewhere |
|---|---|---|
| `tbench-agent-linux-amd64` | `tbench_agent.py`, `run_tb_cersei.sh` | `TBENCH_AGENT_BINARY_AMD64` |
| `tbench-agent-linux-arm64` | same | `TBENCH_AGENT_BINARY_ARM64` |

## Build `tbench-agent` (current harness)

`tbench-agent` is the `[[bin]]` of `crates/bricks-tbench`. It must be a static
(musl) build, as the install error message of `tbench_agent.py` asks. From the
repository root, with the lockfile:

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --release --locked -p bricks-tbench --target x86_64-unknown-linux-musl
cargo build --release --locked -p bricks-tbench --target aarch64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release/tbench-agent bench/term-bench/tbench-agent-linux-amd64
cp target/aarch64-unknown-linux-musl/release/tbench-agent bench/term-bench/tbench-agent-linux-arm64
```

Cross-compiling from macOS also needs a musl linker for each target (for
example `cargo zigbuild`, or a Linux container such as `rust:alpine`); none is
installed by these scripts.

## Recover the historical binaries

The binaries were tracked until Sprint 11.1: `tbench-agent-*` were added in
commit `54999e4` and removed in commit `54079af`. Their blobs are still reachable
from `54999e4`, which holds the versions listed below. To get one back without
rebuilding:

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
