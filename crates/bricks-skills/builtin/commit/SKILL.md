---
name: commit
description: Prepare a Git commit of the current changes with a message that explains why. A procedure only — it commits nothing by itself, and the user's and the repository's rules come first.
metadata:
  origin: bricks-builtin
---
# Prepare a commit

Activating this skill commits nothing. Follow it only when the user asked for a commit,
and follow the user's instructions and the repository's own rules (message format,
signing, authorship, what must not be committed) when they differ from these steps.

1. Look at the state: `git status` and `git diff` (staged and unstaged).
2. Decide what belongs in the commit; leave out unrelated changes and anything that looks
   like a secret, a credential or a local configuration file. Ask when unsure.
3. Stage exactly those files (by name, not everything at once).
4. Write a concise message: a summary line, then why the change was made.
5. Create the commit. Do not push, amend, rebase or force anything unless the user asked
   for it.

Each command goes through `Bash` and its approval.
