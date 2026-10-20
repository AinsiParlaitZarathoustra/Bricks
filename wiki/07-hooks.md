# Hooks

This page is a summary. The reference is [Hooks API](/docs/api-hooks) on the documentation site.

- A hook implements `Hook`: `name`, `events` (the `HookEvent`s it wants) and `on_event`, which receives a `HookContext` and returns a `HookAction`.
- Add a hook with `Agent::builder().hook(...)`.
- A hook can let the agent continue, or change what happens next, as `HookAction` describes. Read the reference before you rely on a particular action.
