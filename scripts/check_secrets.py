#!/usr/bin/env python3
"""Look for secrets in the files about to be versioned, without showing them.

Usage:
  scripts/check_secrets.py --staged         # the staged content (git index)
  scripts/check_secrets.py --tracked        # every tracked file (HEAD's index)
  scripts/check_secrets.py FILE...          # these files

It recognises the shapes Bricks' own redaction knows (Google API keys,
OpenAI / Anthropic keys, bearer tokens, `api_key=` / `access_token=`
parameters) and PEM private keys. A finding is printed as
`path:line: rule (value#<sha256 prefix>)` — never the value. Only files of
this repository are read (nothing in the home folder or elsewhere), and
nothing found is written anywhere.

Known synthetic fixtures are listed in `scripts/secret-allowlist.txt`, one
per line: `path<TAB>value#<sha256 prefix><TAB>why`. An entry excuses that
value in that file only.

Limits: it prevents the shapes it knows, not every possible secret (a
password in prose, a token of an unknown provider, an encoded secret pass).
Exit 0 when nothing is found, 1 otherwise, 2 on a usage error.
"""

import hashlib
import os
import re
import subprocess
import sys

RULES = [
    ("google-api-key", re.compile(r"AIza[0-9A-Za-z_\-]{35}")),
    ("anthropic-key", re.compile(r"sk-ant-[A-Za-z0-9_\-]{20,}")),
    ("openai-key", re.compile(r"sk-(?!ant-)[A-Za-z0-9_\-]{20,}")),
    ("bearer-token", re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._\-]{12,}")),
    ("key-parameter", re.compile(r"(?i)\b(?:api[_-]?key|access[_-]?token)=[A-Za-z0-9._\-]{8,}")),
    ("private-key", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA |)PRIVATE KEY-----")),
]

ROOT = subprocess.run(
    ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True
).stdout.strip()
ALLOWLIST = os.path.join(ROOT, "scripts", "secret-allowlist.txt")


def fingerprint(value):
    return "value#" + hashlib.sha256(value.encode()).hexdigest()[:12]


def allowlist():
    allowed = set()
    if os.path.exists(ALLOWLIST):
        for line in open(ALLOWLIST, encoding="utf-8"):
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) >= 2:
                allowed.add((parts[0], parts[1]))
    return allowed


def git_lines(*args):
    out = subprocess.run(["git", *args], capture_output=True, text=True, cwd=ROOT, check=True)
    return [l for l in out.stdout.split("\0" if "-z" in args else "\n") if l]


def read_staged(path):
    r = subprocess.run(["git", "show", f":{path}"], capture_output=True, cwd=ROOT)
    return r.stdout if r.returncode == 0 else None


def scan(path, data, allowed, findings):
    if data is None or b"\0" in data[:8192]:
        return  # deleted or binary
    text = data.decode("utf-8", errors="replace")
    for n, line in enumerate(text.splitlines(), 1):
        for rule, rx in RULES:
            for m in rx.finditer(line):
                fp = fingerprint(m.group(0))
                if (path, fp) not in allowed:
                    findings.append(f"{path}:{n}: {rule} ({fp})")


def main(argv):
    if len(argv) < 2:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    allowed = allowlist()
    findings = []
    if argv[1] == "--staged":
        for path in git_lines("diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z"):
            scan(path, read_staged(path), allowed, findings)
    elif argv[1] == "--tracked":
        for path in git_lines("ls-files", "-z"):
            full = os.path.join(ROOT, path)
            if os.path.isfile(full):
                scan(path, open(full, "rb").read(), allowed, findings)
    else:
        for path in argv[1:]:
            rel = os.path.relpath(os.path.abspath(path), ROOT)
            if rel.startswith(".."):
                print(f"check_secrets: {path} is outside the repository; not read", file=sys.stderr)
                return 2
            scan(rel, open(os.path.join(ROOT, rel), "rb").read(), allowed, findings)
    for f in findings:
        print(f)
    if findings:
        print(f"check_secrets: {len(findings)} possible secret(s); values not shown", file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
