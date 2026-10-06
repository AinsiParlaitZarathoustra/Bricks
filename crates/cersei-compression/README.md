# cersei-compression

Reduction of tool outputs for the Cersei SDK, without losing access to the
originals. Used by `cersei-agent` on every tool result.

| Output | What happens |
| --- | --- |
| Command logs (`Bash`) | ANSI and progress cleaned, structured records rendered (`cargo --message-format=json`, `go test -json`), **diagnostic blocks found first and kept**, then the rule for the command that actually ran removes noise and summarises repetitive success lines, then a budget keeps diagnostics in priority and shows every omission in place. |
| Values (`cat`, `git diff`, `grep`, `kubectl get`, `Read` with `offset`/`limit`, …) | Never filtered; only cut beyond the hard cap, with paging instructions or a reference to the full text. |
| Large JSON | A summary envelope: shape, verbatim sample (3 items per array by default), and a separate list of omissions by JSON Pointer. |
| Source files read at `aggressive` | A Tree-sitter skeleton (Rust, Python, TypeScript, JavaScript/TSX, Go): signatures, types, docs and attributes kept, bodies replaced by markers with their original line ranges. Not counted as having read the file. |

Every reduced output starts with a `[bricks: …]` header saying what was done
and where the original is (a file saved by the `RawStore`, or the source file
itself), readable with the `Read` tool.

Levels: `off` (only the hard cap), `minimal` (logs, JSON ≥ 16 KiB),
`aggressive` (plus skeletons, JSON ≥ 4 KiB).

Rules: built-in (`src/rules/*.toml`), then `~/.bricks/rules/*.toml` in file-name
order, then `[compression.filters.<id>]` in `bricks.toml`; same id replaces,
`disabled = true` removes, invalid rules are reported and skipped. See
`docs/compression.md` at the repository root.

Measure on the fixture corpus and on your own files:

```
cargo run --release -p cersei-compression --example measure -- --read src/lib.rs --json data.json
```

## Credits

The rule pipeline and ANSI handling started as a port of
[**rtk** (Rust Token Killer)](https://github.com/rtk-ai/rtk) by **Patrick
Szymkowiak**, MIT licensed. See [`LICENSE`](LICENSE) for the attribution.
