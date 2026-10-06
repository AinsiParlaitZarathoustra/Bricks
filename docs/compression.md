# Compression of tool outputs

What happens to a tool result before it enters the active context, how rules
are written and loaded, and how to read the original. Settings live in the
`[compression]` table of `bricks.toml`; rules in `~/.bricks/rules/*.toml` and in
`[compression.filters.<id>]` (see `docs/bricks.example.toml`).

## 1. Principles

* **Diagnostics before truncation.** Compilation errors, failed tests, panics,
  tracebacks and their context lines are found first, wherever they are in the
  log, and kept in priority. Noise and repetitive success lines are reduced
  after that.
* **Nothing reduced silently.** Every reduced output starts with
  `[bricks: …]`: what was done (rule, lines before → after, kept error blocks,
  exit code), any conservative choice, and **where the original is**.
* **Values are not filtered.** Outputs that are the value asked for (`cat`,
  `head`, `sed`, `jq`, `grep`, `ls`, `git diff/show/blame`, `kubectl get`,
  `terraform output`, `gh api`, a file `Read` with `offset`/`limit`) are only cut
  beyond the hard cap, with a way to read the rest.
* **Small gains are not worth a view.** A reduction that saves less than a
  tenth (header included) returns the output unchanged.
* **The hard cap applies at every level, `off` included** (`max_output_chars`,
  and `max_lines + max_error_lines` lines): beyond it, the output is cut keeping
  diagnostics, and the original is saved.

## 2. Levels

| Level | Logs | Files (`Read` without `offset`/`limit`) | JSON |
| --- | --- | --- | --- |
| `off` (default) | hard cap only | hard cap only (paged) | — |
| `minimal` | cleaned, rules, budget | exact text | summary from `json_summary_min_bytes` (16 KiB) |
| `aggressive` | same | Tree-sitter skeleton from `skeleton_min_lines` (150) | summary from a quarter of it |

`Agent::set_compression_level` changes it at runtime; `[compression] level` sets
it from `bricks.toml`.

`WebFetch` and `WebSearch` outputs are already paged and selected by the web
tools (`docs/web.md`): at every level they are passed unchanged, and only
above `max_output_chars` cut once on a line end — never thinned out line by
line, so a page window stays a contiguous, in-order part of the page.

## 3. Logs

Pipeline, in order:

1. **Clean**: ANSI escapes and control characters removed; a carriage return
   keeps only the final state of a progress line.
2. **Structured records** are rendered back to text when present:
   `cargo --message-format=json` (each diagnostic's `rendered` text; artifacts
   counted) and `go test -json` (the `Output` fields in order).
3. **Diagnostic blocks** are detected (built-in detectors for rustc/cargo,
   Rust test output and panics, Python tracebacks and pytest, Go tests, panics
   and build errors, Node errors and stacks, vitest/jest, TypeScript, ESLint,
   npm/pnpm/yarn, Terraform boxes, kubectl, Docker, uv; plus the rule's own
   `diagnostic_blocks`). Two lines of context are kept around each error.
4. **The rule** for the command acts on the other lines only: `replace`,
   `strip_lines_matching` / `keep_lines_matching`, `summarize_lines` (a run of
   matching lines becomes `… [N lines: label]`), `truncate_lines_at`.
   `match_output` (replace everything by a message) applies only to a
   successful run with no diagnostic.
5. **Exact consecutive repeats** become one line with `[repeated N×]`. Distinct
   lines are never merged.
6. **Budget**: errors first, then warnings, within `max_error_lines`
   (a block longer than `max_block_lines` keeps its head and tail); then
   `protect_lines`, then the rest within the rule's `max_lines` (head and tail
   per `head_lines`/`tail_lines`). Every gap is shown in place:
   `… [N lines omitted, M of them diagnostic]`. Blocks that do not fit at all
   are counted in the header.

The `Exit code N` line and the `--- stderr ---` separator the `Bash` tool puts
between stdout and stderr are always kept.

### Which rule applies

The command line is tokenised with shell quoting; lists (`&&`, `||`, `;`) and
pipelines are split; `VAR=value` prefixes and redirections are dropped; and
wrappers are peeled: `sudo`, `env`, `time`, `nice`, `nohup`, `timeout`,
`stdbuf`, `command`, `exec`, `npx`, `bunx`, `pnpx`, `uvx`, `pnpm exec|dlx`,
`yarn exec|dlx`, `npm exec`, `bun x`, `uv run`, `poetry|pipenv|pdm|hatch|rye
run`, `python -m`. The program is the basename of the executable
(`./node_modules/.bin/vitest` → `vitest`). A rule matches a program and,
optionally, sub-command paths (`subcommands = ["compose up"]`), skipping options
(`options_with_value` names flags whose value is a separate word). Text inside
quotes is never a command: `echo "cargo test"` runs `echo`.

Conservative cases, reported in the header: a pipeline whose output was
reshaped by a later stage (`| grep`, `| tail`), several different commands, and
constructs that cannot be followed (`bash -c`, `eval`, `xargs`, sub-shells,
heredocs, command substitution) get the generic rule; an unknown program gets
the generic rule with "no specific rule for `x`".

### Built-in rules

| Ecosystem | Rules (ids) |
| --- | --- |
| Rust | `cargo-test` (test, nextest), `cargo-build` (build, check, clippy, doc, fix), `cargo-run`, `cargo-audit`, `cargo` |
| Go | `go-test`, `go-build` (build, vet, run, mod, …) |
| JS/TS | `npm-install`, `npm`, `pnpm-install`, `pnpm`, `yarn-install`, `yarn`, `bun-install`, `bun-test`, `bun`, `vitest`, `vite`, `turbo`, `eslint`, `next-build`, `tsc` |
| Python | `pytest`, `uv`, `pip-install`, `ruff`, `mypy` |
| DevOps | `docker-compose` (`docker compose` and `docker-compose`), `docker-build`, `docker`, `kubectl-read` (exact), `kubectl-change`, `kubectl`, `terraform-plan`, `terraform-init`, `terraform-exact`, `terraform`, `gh-run-log`, `gh-pr-checks`, `gh-exact`, `gh` |
| Other | `git-exact`, `git-log`, `git-status`, `git-transfer`, `git`, `exact-output`, `generic` (`program = "*"`) |

Each is checked against a fixture in `crates/cersei-compression/tests/fixtures/`:
`captured/` holds real outputs (cargo, go, pytest, python, uv, npm, pnpm, yarn,
cargo-audit); `reconstructed/` reproduces the documented formats of tools that
were not installed on the capturing machine (vitest, eslint, vite, next, turbo,
bun, ruff, mypy, uv, kubectl, terraform, gh, docker compose).

## 4. Writing rules

```toml
schema_version = 1

[filters.my-tool]                       # the id: letters, digits, - _ .
description = "What it does."
match = [{ program = "mytool", subcommands = ["build", "run all"] }]
options_with_value = ["-C", "--config"]
mode = "log"                            # or "exact": never filtered, only capped
strip_lines_matching = ['^\s*Downloading ']   # or keep_lines_matching (exclusive)
summarize_lines = [{ pattern = '^ok ', label = "passing steps" }]
protect_lines = ['^Summary:']
diagnostic_blocks = [
  { start = '^FAILED ', end = "blank", severity = "error", max_lines = 40 },
  { start = '^WARN ', end = "single", severity = "warning" },
  { start = '^BEGIN', until = '^END', inclusive = true },
]
replace = [{ pattern = '\d{4}-\d\d-\d\dT[\d:.]+Z ', replacement = "" }]
match_output = [{ pattern = 'nothing to do', message = "mytool: nothing to do" }]
truncate_lines_at = 400
head_lines = 20
tail_lines = 80
max_lines = 200
on_empty = "mytool: no output"
```

`end` is `single`, `blank` (up to the next blank line), `indented` (following
indented lines) or `until` (with `until` and `inclusive`). Regexes use the Rust
`regex` syntax (no look-around). Rules never execute anything.

### Priority, replacement, disabling

Layers, later winning: built-in → `~/.bricks/rules/*.toml` in byte order of
the file names → `[compression.filters]` of `bricks.toml`. A rule with an
existing id replaces it entirely; `disabled = true` removes it. When several
rules match a command, the most specific wins (longest sub-command path), then
the higher layer, then the later source. Rules are read when the configuration
is loaded (`BricksConfig::load`), so changes apply on the next start — no
recompilation.

An invalid rule (bad regex, unknown field, wrong type, missing `match`) yields a
diagnostic naming the file, the rule id and the field; only that rule is
skipped — an earlier valid rule with the same id stays active. A file that is
not valid TOML is reported and skipped as a whole; other files still load.
Diagnostics are shown as `Status` events at the first run
(`BricksConfig::diagnostics` also lists them).

## 5. Source skeletons

At `aggressive`, a source file read without `offset`/`limit` and at least
`skeleton_min_lines` long is shown as a Tree-sitter skeleton: imports,
attributes and decorators, signatures with types and generics,
struct/class/interface/enum/type declarations with their fields and variants,
and comments outside function bodies (doc comments, docstrings, JSDoc). Function
bodies, and module- or class-level initialisers longer than 8 lines, become a
marker naming the original range (`// ⋯ 12 lines omitted (L34–L45)`, Python
`...  # ⋯ …`). Nested declarations keep their signature. Every kept line keeps
its original line number.

Covered: Rust, Python, TypeScript, JavaScript/JSX/TSX (TSX grammar), Go. Other
languages are shown as text.

A skeleton is an exploration view, not source: it is flagged as such, it does
not count as having read the file (the read-before-edit guard still requires an
exact read), and the header says how to read a range
(`Read offset=<first line − 1> limit=<n>`) or the whole file. A body containing
an `ERROR` or `MISSING` node is kept in full; if more than 30 % of the file is
uncertain, or the skeleton would hide less than 15 %, the text is shown as is.

## 6. JSON summaries

A large JSON document becomes an envelope:

```json
{
  "bricks_view": "json-summary/v1",
  "note": "...",
  "source": { "bytes": 1062449, "root_type": "object", "raw": "..." },
  "limits": { "sample_items": 3, "max_depth": 6, "max_keys": 40, "max_string_chars": 200 },
  "shape": { "type": "object", "keys": { "items": { "type": "array", "length": 1500, "items": { ... } } } },
  "sample": { "items": [ {...}, {...}, {...} ], "kind": "List" },
  "omitted": [ { "pointer": "/items", "what": "array: 3 of 1500 items shown (sample)" } ]
}
```

The document's own data appears only under `sample`, copied verbatim (numbers
and strings exactly as written, so `12.50` and big integers keep their form);
no marker is ever inserted into it, so keys such as `omitted` or `_truncated` in
the document cannot collide with the envelope. Omissions are listed apart, as
JSON Pointers into the original. Depth, keys, string length, input size
(`json` limits) and output size are bounded; the parser streams over the input
and keeps only the sample. Invalid JSON is shown as text with the line and
column of the error.

## 7. Reading the original

* Command and tool outputs: the header names a file saved by the output store —
  `full output: Read file_path="…/00007-Bash-call_x.txt" (N lines; use
  offset/limit to page)`. The store belongs to the session: with a session
  memory it is the session's files directory (`JsonlMemory`:
  `<session>.files/`), kept across restores and deleted with the session;
  without one, a per-session directory under the system temp directory.
  `AgentBuilder::raw_output_dir` or `[compression] raw_output_dir` set an
  explicit location instead. A reopened store never overwrites earlier files.
  Reading a stored file is never reduced again.
* File views (skeleton, JSON summary): the header names the file itself and the
  `Read` call for a range or the whole of it.
* Old tool results later removed from the active context to save space are
  replaced by a placeholder naming their saved original.
* `Agent::raw_history()` holds every tool result unreduced (`docs/context.md`).

If the store cannot write, the header says the original was **not** saved; no
promise is made beyond what was actually kept.

## 8. Measuring

```
cargo run --release -p cersei-compression --example measure -- --read path/to/file.rs --json data.json
```

prints, per fixture and file: lines, bytes, estimated tokens before → after
(method: `cersei_types::tokens::ESTIMATION_METHOD`), time, and how many of the
fixture's must-keep lines survived. Percentages reported by other projects are
references, not targets that justify losing information.
