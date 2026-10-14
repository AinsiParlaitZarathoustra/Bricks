# Tools

Tools are capabilities the agent can invoke: reading files, running shell commands, searching codebases, calling APIs.

## The Tool Trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    fn permission_level(&self) -> PermissionLevel { PermissionLevel::None }
    /// Level for this particular input (default: `permission_level()`).
    fn permission_level_for(&self, _input: &serde_json::Value) -> PermissionLevel { self.permission_level() }
    fn category(&self) -> ToolCategory { ToolCategory::Custom }
    async fn execute(&self, input: serde_json::Value, ctx: &ToolContext) -> ToolResult;
}
```

`permission_details` and `preview` are optional hooks used by the approval flow; see [Permissions](08-permissions.md).

## Built-in Tools

### Tool sets

```rust
cersei::tools::all()          // every set below, plus AskUserQuestion, SyntheticOutput, Config, ToolSearch
cersei::tools::coding()       // filesystem() + shell() + web()
cersei::tools::filesystem()   // Read, Write, Edit, MultiEdit, ApplyPatch, Glob, Grep, CodeSearch, CodeScout, NotebookEdit
cersei::tools::shell()        // Bash (+ BashTaskStatus, BashTaskOutput, BashTaskStop), Job, PowerShell
cersei::tools::web()          // WebFetch, WebSearch
cersei::tools::planning()     // EnterPlanMode, ExitPlanMode, TodoWrite
cersei::tools::scheduling()   // CronCreate, CronList, CronDelete, Sleep, RemoteTrigger
cersei::tools::orchestration() // SendMessage, Task*, EnterWorktree, ExitWorktree
cersei::tools::none()         // empty (pure chat, no tools)
```

Each set returns `Vec<Box<dyn Tool>>`, passed to `Agent::builder().tools(...)`.

### Complete table

Permission levels are those declared in the code. Where a level depends on the input, the table says so.

| Tool | Permission | Set |
|------|------------|-----|
| `Read` | ReadOnly | `filesystem()` |
| `Write` | Write | `filesystem()` |
| `Edit` | Write | `filesystem()` |
| `MultiEdit` | Write | `filesystem()` |
| `ApplyPatch` | Write | `filesystem()` |
| `Glob` | ReadOnly | `filesystem()` |
| `Grep` | ReadOnly | `filesystem()` |
| `CodeSearch` | ReadOnly | `filesystem()` |
| `CodeScout` | ReadOnly | `filesystem()` |
| `NotebookEdit` | Write | `filesystem()` |
| `Bash` | Execute | `shell()` |
| `BashTaskStatus` | ReadOnly | `shell()` |
| `BashTaskOutput` | ReadOnly | `shell()` |
| `BashTaskStop` | Execute | `shell()` |
| `Job` | ReadOnly; Execute for action `stop` | `shell()` |
| `PowerShell` | Execute | `shell()` |
| `WebFetch` | ReadOnly | `web()` |
| `WebSearch` | ReadOnly | `web()` |
| `EnterPlanMode` | None | `planning()` |
| `ExitPlanMode` | None | `planning()` |
| `TodoWrite` | None | `planning()` |
| `CronCreate` | Execute | `scheduling()` |
| `CronList` | None | `scheduling()` |
| `CronDelete` | Execute | `scheduling()` |
| `Sleep` | None | `scheduling()` |
| `RemoteTrigger` | Execute | `scheduling()` |
| `SendMessage` | None | `orchestration()` |
| `TaskCreate`, `TaskGet`, `TaskUpdate`, `TaskList`, `TaskStop`, `TaskOutput` | None | `orchestration()` |
| `EnterWorktree` | Write | `orchestration()` |
| `ExitWorktree` | Write | `orchestration()` |
| `AskUserQuestion` | None | `all()` only |
| `SyntheticOutput` | None | `all()` only |
| `Config` | None | `all()` only |
| `ToolSearch` | None | `all()` only (indexes the tools registered before it) |

Tools outside the sets:

| Tool | Permission | Where it comes from |
|------|------------|---------------------|
| `Skill` | ReadOnly | Added by the agent when `Agent::builder().skills(...)` is set |
| `Agent` | None | `cersei_agent::agent_tool::AgentTool`, sub-agents; add it explicitly |
| `delegate` | None | `cersei_agent::delegate_tool::DelegateTool`; add it explicitly |
| MCP tools | from annotations (see below) | `Agent::builder().mcp_server(...)` |
| `SendVmMessage`, `SharedStateSet`, `SandboxSnapshot` | Write | `cersei_tools::vm_tools::all_vm_tools()`, feature `vms`; not in any set |
| `RecvVmMessage`, `SharedStateGet` | ReadOnly | same as above |
| `LSP` | ReadOnly | `cersei_tools::lsp_tool::LspTool`; defined, but no set includes it |

The `Bash` and `PowerShell` tools keep a persistent interpreter per agent session: `cd`, `export` and shell variables survive from one call to the next while the interpreter lives. A sub-agent has its own session.

### Glob and CodeSearch: what they walk

`Glob` walks with the walker shared by `Grep` and the code engine:
`.gitignore` and `.ignore` rules apply (with `!` re-inclusions), even outside
a git checkout, and ignored directories are pruned before the walk enters
them; hidden entries are skipped unless the pattern names one
(`.github/**`). `path` is taken literally (brackets, spaces); only `pattern`
is a glob, and its leading literal directories narrow the walk. The walk is
sorted and stops at the first match past `limit`, so it says
`Showing the first N matches; more matches exist. Narrow the pattern.`
instead of a total it did not count. A rare pattern can still visit the
whole tree: the walk stops after 200 000 entries (and says so), or when the
call is cancelled.

`CodeSearch` keeps one index per root and embedding configuration. Each
search takes an inventory of the sources (same ignore rules, plus
`node_modules`, `target`, `__pycache__`, `venv`, hidden entries, files over
500 KB): path, size and modification time of each indexable file. The index
is reused only for the same inventory and when no writing tool of the agent
ran since its build began; otherwise it is built again — files read once,
BM25 and vectors from the same chunks, on the blocking pool, shared by
concurrent searches, vectors of unchanged chunks reused. An external edit
that keeps both the size and the modification time of a file is not seen
until a writing tool runs or the inventory changes.

### Bash: shell state persistence

`Bash` runs in one persistent `bash` per agent session (`BashSession`, Unix only):

```
Turn 1: Bash("cd src && export FOO=bar")
Turn 2: Bash("pwd")       → outputs /project/src
Turn 3: Bash("echo $FOO") → outputs bar
```

If the interpreter ends (`exit`, `exec`, a crash, or a timeout that required destroying it), the next command starts a fresh one and says so.

## Creating Custom Tools

### Option 1: Implement Tool directly

```rust
struct WeatherTool { api_key: String }

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str { "get_weather" }
    fn description(&self) -> &str { "Get current weather for a city." }
    fn permission_level(&self) -> PermissionLevel { PermissionLevel::ReadOnly }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" }
            },
            "required": ["city"]
        })
    }

    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        let city = input["city"].as_str().unwrap_or("unknown");
        // Call weather API...
        ToolResult::success(format!("72F, sunny in {}", city))
    }
}
```

### Option 2: Derive macro (typed)

The generated code uses the paths `cersei_tools::`, `serde_json::`, `schemars::` and `async_trait::`. The crate using the derive needs `cersei-tools` and `async-trait` as direct dependencies; `serde_json` and `schemars` are also re-exported by `cersei::prelude`. `serde` with feature `derive` is needed for `#[derive(Deserialize)]`.

```rust
use cersei::prelude::*;
use serde::Deserialize;

#[derive(Tool)]
#[tool(name = "search_docs", description = "Search documentation", permission = "read_only")]
struct SearchTool;

#[derive(Deserialize, schemars::JsonSchema)]
struct SearchInput {
    /// The search query
    query: String,
    /// Max results to return
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize { 10 }

#[async_trait]
impl ToolExecute for SearchTool {
    type Input = SearchInput;
    async fn run(&self, input: SearchInput, _ctx: &ToolContext) -> ToolResult {
        ToolResult::success(format!("Found results for: {}", input.query))
    }
}
```

`#[tool(...)]` accepts `name`, `description`, `permission` (`none`, `read_only`, `write`, `execute`, `dangerous`) and `category` (`filesystem`, `shell`, `web`, `memory`, `orchestration`, `mcp`). The derive generates:
1. `name()`, `description()`, `permission_level()` and `category()` from `#[tool(...)]`
2. `input_schema()` from the `JsonSchema` derive on `Input`
3. `execute()`, which deserializes the JSON into `Input` and calls `run()`

### Option 3: Closure-backed tool

For simple tools you can wrap a closure:

```rust
struct FnTool {
    name: String,
    description: String,
    schema: serde_json::Value,
    handler: Box<dyn Fn(serde_json::Value) -> String + Send + Sync>,
}

#[async_trait]
impl Tool for FnTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn input_schema(&self) -> serde_json::Value { self.schema.clone() }
    async fn execute(&self, input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
        ToolResult::success((self.handler)(input))
    }
}
```

Register any of them with `Agent::builder().tool(MyTool)` or `.tools(vec![...])`.

## ToolContext

Every tool receives a `ToolContext`:

```rust
pub struct ToolContext {
    pub working_dir: PathBuf,                    // agent's working directory
    pub session_id: String,                      // session identifier
    pub permissions: Arc<dyn PermissionPolicy>,  // the agent's permission policy
    pub cost_tracker: Arc<CostTracker>,          // cumulative token usage
    pub mcp_manager: Option<Arc<McpManager>>,    // MCP server connections
    pub extensions: Extensions,                  // user-injected data
}
```

### Extensions (type map)

Inject shared data at build time, read it from any tool:

```rust
let extensions = cersei::tools::Extensions::default();
extensions.insert(my_pool.clone());

let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .extensions(extensions)
    .tool(MyDbTool)
    .build()?;

// Inside MyDbTool::execute:
async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
    let pool = ctx.extensions.get::<MyPool>().expect("pool injected at build time");
    // use pool...
    ToolResult::success("ok")
}
```

`Extensions::get::<T>()` returns `Option<Arc<T>>`.

## ToolResult

```rust
pub struct ToolResult {
    pub content: String,           // text sent back to the model
    pub is_error: bool,            // true = model sees it as an error
    pub metadata: Option<Value>,   // structured data (for UI rendering, etc.)
    // also: report (a structured ToolReport), set via from_report
}

ToolResult::success("output text")
ToolResult::error("something went wrong")
ToolResult::success("ok").with_metadata(json!({ "diff": "..." }))
```

## Permission Levels

| Level | Meaning | Examples |
|-------|---------|----------|
| `None` | No side effects on the machine | Task tools, TodoWrite, Sleep |
| `ReadOnly` | Reads the filesystem or network | Read, Glob, Grep, WebFetch, Skill |
| `Write` | Modifies files | Write, Edit, MultiEdit, ApplyPatch, NotebookEdit |
| `Execute` | Runs commands or changes scheduled work | Bash, PowerShell, CronCreate |
| `Dangerous` | Irreversible or destructive | MCP tools annotated `destructiveHint` |
| `Forbidden` | Never allowed | No built-in tool declares it |

The engine checks the level only through the agent's permission policy. `Forbidden` is refused by the approval rules loaded from `bricks.toml` (`ApprovalRules`, in `cersei_agent::control`; see [Permissions](08-permissions.md)), but an agent built with `AllowAll` will run such a tool. Do not rely on `Forbidden` alone as a safety rail.

## Tool Categories

Used for grouping in tool listings and UIs:

```rust
pub enum ToolCategory {
    FileSystem,     // Read, Write, Edit, Glob, Grep, ...
    Shell,          // Bash, PowerShell, Job
    Web,            // WebFetch, WebSearch
    Memory,         // memory tools
    Orchestration,  // Agent, Task*, SendMessage, worktrees
    Mcp,            // MCP-provided tools
    Custom,         // user-defined (default)
}
```

## MCP Tools

MCP (Model Context Protocol) servers are declared on the agent. They connect on the first run, and their tools are offered alongside the built-in ones:

```rust
use cersei::mcp::McpServerConfig;

let agent = Agent::builder()
    .provider(provider_from_config(None, "my-provider/my-model")?)
    .mcp_server(McpServerConfig::stdio("db", "npx", &["-y", "@my/db-mcp"]))
    .tools(cersei::tools::shell())
    .build()?;
```

Each MCP tool's permission level comes from the server's annotations: `readOnlyHint` gives ReadOnly, `destructiveHint` gives Dangerous, and anything else gives Execute. These are hints from the server, not guarantees.

To use the tools of a `McpManager` you created yourself, call `cersei_tools::mcp_tool::tools_of(&Arc<McpManager>).await`, an async function that returns the `Vec<Box<dyn Tool>>` to pass to `.tools(...)`.
