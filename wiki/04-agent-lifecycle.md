# Agent Lifecycle

This page is a summary. The full reference is [Agent API](/docs/api-agent) on the documentation site.

- The agent sends the conversation and tool definitions to the provider. If the model returns tool calls, the agent runs them and sends the results back, repeating until the model finishes its turn.
- The loop stops when the model ends its turn, when the run is cancelled, when an error occurs, or when the no-progress guard fires. There is no fixed turn limit.
- `Agent::run` returns an `AgentOutput` containing the final message, token usage, stop reason, turn count, and tool call records.
- `Agent::run_stream` returns an `AgentStream` of `AgentEvent` values. Call `AgentStream::collect` to wait for the final `AgentOutput`.
- Auto-compaction (`auto_compact(true)`) shortens the conversation when the context window fills up. Setting `session_id` and `memory(...)` on `AgentBuilder` saves and reloads session history across runs.
