# Memory

This page is a summary. The full reference is [Memory API](/docs/api-memory) on the documentation site.

Bricks separates short-term session history from long-term structured memory:

- **Session history (`Memory` trait):** Saves and reloads conversation messages by `session_id` (for example with `JsonlMemory` in `bricks-memory`). Attach it with `Agent::builder().memory(...)` and `.session_id(...)`.
- **Long-term memory (`StructuredMemory`, feature `structured`):** Stores episodes, entities, and facts in a Grafeo database and indexes them for vector, lexical, and relational recall.
- **Scoped runs (`ScopedMemory`):** Combines a project store and a global store for a single run. Recall reads from both stores, while automatic capture writes only to the project store.
- **Notes and length limit:** Models can search memory with `memory_search` and save concise notes with `memory_write`. Each note is capped at `NOTE_MAX_CHARS` (280 characters).
- **Deleting entries:** `StructuredMemory::delete_entry(EntryKind, id)` removes an individual note, exchange, or fact. The desktop app exposes this in `MemoryView`.
