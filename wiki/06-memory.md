# Memory

Memory provides session persistence, so a conversation can be resumed later, possibly from another process. Long-term recall (a separate trait, described at the end) adds knowledge from past sessions to the system prompt.

## The Memory trait

```rust
#[async_trait]
pub trait Memory: Send + Sync {
    async fn store(&self, session_id: &str, messages: &[Message]) -> Result<()>;
    async fn load(&self, session_id: &str) -> Result<Vec<Message>>;
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>>;
    async fn sessions(&self) -> Result<Vec<SessionInfo>>;
    async fn delete(&self, session_id: &str) -> Result<()>;
    /// Directory for files the session keeps beside its transcript (optional).
    fn session_files_dir(&self, _session_id: &str) -> Option<PathBuf> { None }
}
```

## Built-in backends

### JsonlMemory

File-based storage: one `.jsonl` file per session, one JSON message per line. `store` rewrites the whole file. `load` returns an empty list for an unknown session. `search` always returns nothing, because this backend has no index.

```rust
use cersei::memory::JsonlMemory;

let memory = JsonlMemory::new("/path/to/sessions");

let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .memory(memory)
    .session_id("my-project-refactor")
    .build()?;
```

File structure:

```
/path/to/sessions/
  my-project-refactor.jsonl          # the conversation, one message per line
  my-project-refactor.raw.jsonl      # raw history: every message, including compacted ones
  my-project-refactor.compaction-1.jsonl  # snapshot taken before the first compaction
  other-session.jsonl
```

The `.raw` and `.compaction-N` files belong to their session: `sessions()` does not list them as sessions of their own.

### InMemory

A map held in the process, for tests and ephemeral agents:

```rust
use cersei::memory::InMemory;

let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .memory(InMemory::new())
    .session_id("test-1")
    .build()?;
```

## Resumable sessions

The key pattern: same `session_id` and same backend location means a resumed conversation.

```rust
// Session 1
let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .memory(JsonlMemory::new("./sessions"))
    .session_id("feature-x")
    .build()?;
agent.run("Start building the auth module").await?;

// Session 2 (later, even in another process)
let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .memory(JsonlMemory::new("./sessions"))
    .session_id("feature-x")  // same ID → loads previous messages
    .build()?;
agent.run("Continue. Add the JWT validation.").await?;
```

## Lifecycle

1. **Start of a run.** If a memory and a `session_id` are set, and the agent has no messages yet (messages supplied with `with_messages` take precedence and skip loading), the stored history is loaded. It is placed before the new prompt. The raw history and any compaction snapshots are loaded with it. Emits `AgentEvent::SessionLoaded { session_id, message_count }`.
2. **During the run.** New messages (prompts, responses, tool results) accumulate in the agent's active history.
3. **End of the run.** The active history and the raw history are stored. Emits `AgentEvent::SessionSaved { session_id }`.

For an interrupted run, or to save explicitly, call `agent.save_session().await?`. It returns `false` when no memory or session id is configured. `agent.clear_context()` empties the active history but keeps it as a snapshot, like a compaction does.

## Session management

```rust
let memory = JsonlMemory::new("./sessions");

// List all sessions
for s in memory.sessions().await? {
    println!("{}: {} messages ({}, model {:?})", s.id, s.message_count, s.created_at, s.model);
}

// Delete a session
memory.delete("old-session").await?;
```

`SessionInfo` has `id`, `created_at` (UTC), `message_count` and `model` (optional).

`JsonlMemory::delete` also removes the session's `<id>.raw` and `<id>.compaction-<n>` files and its `<id>.files` directory (the saved originals of reduced tool outputs).

## Building a custom backend

Example: PostgreSQL-backed memory. The `search`, `sessions` and `delete` bodies are sketches.

```rust
struct PgMemory { pool: sqlx::PgPool }

#[async_trait]
impl Memory for PgMemory {
    async fn store(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        let json = serde_json::to_string(messages)?;
        sqlx::query("INSERT INTO sessions (id, messages) VALUES ($1, $2)
                      ON CONFLICT (id) DO UPDATE SET messages = $2")
            .bind(session_id)
            .bind(&json)
            .execute(&self.pool)
            .await
            .map_err(|e| CerseiError::Other(e.into()))?;
        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<Vec<Message>> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT messages FROM sessions WHERE id = $1"
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CerseiError::Other(e.into()))?;

        match row {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(Vec::new()),
        }
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        // Full-text search with pg_trgm or vector search with pgvector
        todo!()
    }

    async fn sessions(&self) -> Result<Vec<SessionInfo>> {
        // SELECT id, message_count, created_at, model FROM sessions
        todo!()
    }

    async fn delete(&self, session_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| CerseiError::Other(e.into()))?;
        Ok(())
    }
}
```

The engine stores the raw history and the compaction snapshots through the same `store` and `load` calls, under the keys `<id>.raw` and `<id>.compaction-<n>`. A custom backend must treat those keys like any other id. `sessions()` may hide them, as `JsonlMemory` does.

## Long-term memory (structured)

Long-term recall is a separate trait, `LongTermMemory`, used with `Agent::builder().long_term_memory(...)` and `.memory_recall_tokens(n)`. Its recalled text goes into the system prompt of each run, within a token budget, and each finished exchange is recorded.

The structured implementation, `cersei_memory::structured::StructuredMemory`, keeps sourced episodes, entities and temporal facts in a Grafeo database, and recalls with a hybrid of vector search, lexical search and relational expansion. It is enabled by the `structured` feature of `cersei-memory`, which also enables `graph` (the Grafeo-backed `GraphMemory` store) and `embed`. This page does not describe it further: see [`docs/memory.md`](../docs/memory.md) and `crates/cersei-memory/src/structured/`.
