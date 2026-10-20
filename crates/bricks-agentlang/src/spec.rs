//! The language specification surfaced to LLMs.
//!
//! Injected into an agent's system prompt when it is allowed to author
//! sub-agents in AgentTemplate. Kept short and example-heavy — models learn the
//! DSL far better from worked examples than from formal grammar.

pub const AGENTLANG_SPEC: &str = r#"# AgentTemplate language

A tiny functional language you can author and run on top of the Bricks runtime.
Programs are line-oriented; each line is an assignment or a call.

## Syntax
- Variables start with `$`:           `$path = "/tmp/a.txt"`
- Namespaced builtin calls:           `io.read($path)`
- Method chaining (the previous result flows in as the chain value):
                                       `io.read($src).write($dst)`
- Named arguments:                     `agent.send(to: "worker-2", $payload)`
- Literals: strings ('single' or "double"), numbers, true/false, null, arrays `[1, 2]`
- Comments start with `#`

## Builtins
| Call                              | Effect                                  | Permission |
|-----------------------------------|-----------------------------------------|------------|
| io.read($file)                    | read a file's contents                  | read       |
| io.write($file, content: $data)   | write a file                            | write      |
| io.edit($file, ...)               | edit a file                             | write      |
| io.glob($pattern)                 | list files matching a glob              | read       |
| io.grep($pattern)                 | search file contents                    | read       |
| io.delete($file)                  | delete a file                           | write      |
| net.get($url)                     | fetch a URL                             | read       |
| net.search($query)               | web search                              | read       |
| agent.send(to: $id, $payload)     | message another agent by id             | write      |
| agent.tools.call($name, {..})     | invoke a registered/built-in tool       | exec       |
| agent.tools.list()                | list available tool names               | none       |
| agent.tools.register(..)          | register a new tool                     | dangerous  |
| agent.permission.ask('rw')        | ask for a permission; returns a bool    | none       |
| kv.get($key) / kv.set($key, $v)   | shared key/value state                  | read/write |

## Examples
```
# read a config, then write a transformed copy
$cfg = io.read("/etc/app.toml")
io.write("/tmp/app.toml", content: $cfg)

# chain: read then write in one expression
io.read("/tmp/in.txt").write("/tmp/out.txt")

# coordinate with another agent
$tools = agent.tools.list()
agent.send(to: "planner", $tools)
```
"#;

/// Returns the language spec for embedding in a system prompt.
pub fn language_spec() -> &'static str {
    AGENTLANG_SPEC
}
