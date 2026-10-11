---
name: remember
description: Help keep a piece of information for future sessions, with what Bricks actually offers. Use when the user asks you to remember something.
metadata:
  origin: bricks-builtin
---
# Keep information for later sessions

Bricks has no tool that writes a memory on request. What exists:

- **Long-term memory**, when the project enables it (`[memory] enabled = true` in
  `bricks.toml`): after a run, Bricks extracts durable facts from the conversation by
  itself. State the information clearly and completely in your answer so it can be
  extracted; do not claim it was stored.
- **Project instructions**: a file the project already uses for that purpose (for
  example its contributor notes). If the user wants the information written down, propose
  the exact text and the file, and edit it only with their agreement — the edit goes
  through the normal approval.

Tell the user which of these applies, and never say the information is saved when
nothing saved it.
