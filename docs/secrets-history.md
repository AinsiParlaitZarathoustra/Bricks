# Secrets in the Git history

Status on 2026-10-09. No value is reproduced here. Each one is named by
`value#` and the first characters of its SHA-256.

## What was found

Scanned: every commit reachable from `main` (the only branch, no tag), with
gitleaks (redacted output) and a direct search of the two commits named by the
audit. The current tree (`HEAD`) holds no real secret. Its only matches are
fake values in redaction tests, listed in `scripts/secret-allowlist.txt`.

| commit | date, author | path | shape | values |
|---|---|---|---|---|
| `f5b577d` | 2026-04-23, Adib Mohsin (Cersei) | `bench/term-bench/runner-google.sh` | Google API key | 2 distinct: `value#1d83bfc089`, `value#94e6739878` |
| `a165181` | 2026-04-18, Adib Mohsin (Cersei) | `bench/tb-results/abstract-20260417-171012/*/agent/abstract-output.jsonl` (9 files) | Google API key | 1: `value#94e6739878` (same as above) |
| `a165181` | same | same files | OpenAI-shaped key | 1: `value#824ae0cf7c` |
| `a165181` | same | `.../sanitize-git-repo__*/{agent,verifier}/...` | generic API key | 3 distinct (`value#62de11afe2`, `value#6c3b7674b5`, `value#f4b31e2e38`). The task of that run is a repository with planted fake secrets, so these are likely not real. |
| `54999e4` | 2026-06-18, Adib Mohsin | `crates/cersei-agentrl/src/scrub.rs` | generic API key | 1 (`value#108213c5f6`), a fixture of a redaction test |

A match on a format is not proof that a key works. None was tried against any
service. These commits come from the Cersei history, before Bricks. The keys
may belong to its author, not to the owner of this fork.

## Rotation (for the owner of each key)

1. Treat every real-looking value above as compromised: two Google keys and
   one OpenAI-shaped key.
2. The owner revokes or rotates each one in the provider's console (Google
   Cloud: APIs & Services, Credentials. OpenAI: API keys). Then check the
   provider's usage logs since April 2026.
3. Rotate **before** any history purge (GitHub's guidance). A purge does not
   revoke anything, and copies outside this repository (forks, clones,
   caches) keep the old objects.

Status: **not confirmed**. The owner(s) still have to do this and confirm it.

## Purge plan (not executed)

A purge rewrites history. Do it only after an explicit decision.

* Refs: `main` only. Every commit from `a165181` on is rewritten. That is
  about 100 commits, 28 of them signed. Their signatures would no longer
  verify. Re-signing is a separate choice.
* First, make a protected mirror backup (`git clone --mirror`), kept off
  GitHub.
* Tool: `git filter-repo` (to install). For example:

  ```bash
  git filter-repo --path bench/tb-results --path bench/term-bench/runner-google.sh --invert-paths
  ```

  Then run `--replace-text` with a local file of the exact values (never
  committed) for any remaining occurrence, and scan again with gitleaks.
* Then: `git push --force` of `main` (and nothing else), a GitHub support
  request to drop cached views and pull-request refs, and a fresh clone for
  everyone. An old clone pushed back brings everything back.
* Removing `runner-google.sh` from the current branch, or ignoring files,
  changes neither the history nor the keys.

## Prevention in place

* `scripts/check_secrets.py --staged` before a commit (or `--tracked`). It
  looks for Google, OpenAI and Anthropic keys, bearer tokens, `api_key=`
  parameters and PEM private keys. It prints `path:line: rule (value#...)`,
  never the value. It catches the shapes it knows, not every secret.
* `scripts/test_check_secrets.sh` tests it with values built at run time.
* `.gitignore` keeps benchmark results, logs and caches out of Git.
