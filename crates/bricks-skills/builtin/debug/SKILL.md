---
name: debug
description: Investigate a bug or unexpected behaviour, find its root cause and propose a fix. Use when something is broken and the cause is not known yet.
metadata:
  origin: bricks-builtin
---
# Diagnose a problem

The user's description of the problem comes with this activation, or in their message.

1. Reproduce or locate the symptom: the error message, the failing test, the command
   that shows it. Search the code for the messages and names involved (`Grep`, `Glob`,
   `Read`).
2. Form hypotheses about the cause and check each one against the code and, when it
   helps, by running a targeted command or test.
3. Identify the root cause, not only the line where it shows.
4. Propose the fix: the change and why it addresses the cause. Make it if the user
   asked you to fix the problem; otherwise describe it.
5. Say how to confirm the fix (the test or command), and run it when you made the change.

Do not hide a symptom (silencing an error, skipping a test) and call it a fix.
