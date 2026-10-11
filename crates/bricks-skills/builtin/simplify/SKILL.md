---
name: simplify
description: Review the code changed in this session for reuse, quality and efficiency, then fix the problems found. Use after editing several files or finishing a feature.
metadata:
  origin: bricks-builtin
---
# Simplify the changes of this session

1. List what changed: the files you edited in this session (`git diff` through `Bash`
   when the project is a Git repository, otherwise your own record of the edits).
2. Review each change for:
   - code that duplicates an existing function or utility of the project;
   - naming, structure and error handling that do not match the surrounding code;
   - work done twice, or more often than needed;
   - complexity the task does not require.
3. Fix what you find with the editing tools. Stay within the changed code: no new
   feature, no refactoring beyond what the problems need.
4. Run the relevant checks (build, tests, linters) to confirm nothing broke.
5. Report what you changed and why, and what you chose to leave.

Every edit and command goes through the normal tools and their approvals.
