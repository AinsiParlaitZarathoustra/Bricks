# Hooks

This page is a summary. The full reference is [Hooks API](/docs/api-hooks) on the documentation site.

- A hook implements the `Hook` trait: `name`, `events` (the `HookEvent` values it listens to), and `on_event`, which receives a `HookContext` and returns a `HookAction`.
- Register a hook with `Agent::builder().hook(...)`.
- A hook can let the agent continue (`HookAction::Continue`), block a tool call (`HookAction::Block`), modify tool input (`HookAction::ModifyInput`), or inject a message (`HookAction::InjectMessage`). Check the API reference for which events support each action.
