# Memory

This page is a summary. The reference is [Memory API](/docs/api-memory) on the documentation site.

- A conversation memory implements the `Memory` trait: `store` and `load` a session, `search`, `sessions` and `delete`. `JsonlMemory` keeps each session as JSON lines in a directory.
- Attach a memory with `Agent::builder().memory(...)` and a `session_id`. The history of that session is loaded when the agent is built.
- `MemoryManager` builds a context for the system prompt from the `CLAUDE.md` instruction files and the `MEMORY.md` index of the memory files. It does not add graph results.
- `GraphMemory` stores memories as graph nodes, with tags, links, confidence decay and revalidation. It needs the `graph` feature of `bricks-memory`, which the facade does not enable.
