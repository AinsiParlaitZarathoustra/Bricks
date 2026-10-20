# Agent Lifecycle

This page is a summary. The reference is [Agent API](/docs/api-agent) on the documentation site.

- The agent sends the conversation and the tool definitions to the provider. If the reply contains tool calls, the agent runs them and sends the results back. It repeats this until the model ends its turn.
- The loop stops on the model's end of turn, on cancellation, on an error, or when the no-progress guard fires. There is no fixed turn limit.
- `Agent::run` returns an `AgentOutput`: the final message, the usage, the stop reason, the number of turns, and the list of tool calls.
- `Agent::run_stream` returns an `AgentStream` of `AgentEvent`s. `AgentStream::collect` waits for the final output.
- Auto-compaction (`auto_compact(true)`) shortens the history when the context fills up. The builder's `session_id` and a memory keep the history between runs.
