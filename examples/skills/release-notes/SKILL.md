---
name: release-notes
description: Draft release notes for the changes since the last tag, in the format of references/format.md. Use when the user asks for release notes or a changelog entry.
license: MPL-2.0
compatibility: Needs a Git repository.
metadata:
  example: "true"
---
# Release notes

1. Find the last tag (`git describe --tags --abbrev=0`) and list the commits since it
   (`git log <tag>..HEAD --oneline`). Both run through `Bash` and its approval.
2. Read `references/format.md` (relative to this skill's directory) with `Read`: it gives
   the sections and the tone.
3. Group the changes by section, one line each, written for users rather than developers.
4. Show the draft. Write it to a file only if the user asks, with the editing tools.

Nothing in this skill runs by itself; it describes how to do the work.
