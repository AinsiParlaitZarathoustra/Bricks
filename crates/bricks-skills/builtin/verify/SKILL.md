---
name: verify
description: Check that recent changes work end to end, with the project's own tests and checks, and report the results faithfully. Use after making changes.
metadata:
  origin: bricks-builtin
---
# Verify recent changes

1. Identify what changed and what it is supposed to do.
2. Find how the project checks itself: its README, contribution notes, scripts, CI
   configuration, build files.
3. Run the relevant checks (tests, build, type checker, linters), the targeted ones
   first, then broader ones when the change is shared.
4. Exercise the changed behaviour directly when tests do not cover it.
5. Look for regressions next to the change.
6. Report precisely: what ran, what passed, what failed (with the output), what could
   not be checked and why. A check that did not run is not a pass.
