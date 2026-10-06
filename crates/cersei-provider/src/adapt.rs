//! The tool-serialization seam.
//!
//! Every protocol adapter turns a `ToolDefinition` into wire JSON through
//! [`adapt_tools`] with its dialect, instead of serializing `input_schema`
//! verbatim: tool names are sanitized to `^[a-zA-Z0-9_-]{1,64}$` (collisions
//! deduplicated) and `$ref`/`$schema`/`definitions` are inlined or stripped.
//!
//! `OpenAiStrict` has no wire site yet: no adapter sends `strict: true`. It
//! exists (and is tested) because the strict transform is the prerequisite for
//! ever doing so.

use cersei_types::ToolDefinition;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

/// Which provider schema dialect to serialize tools into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDialect {
    /// `{name, description, input_schema}`. Permissive; only the common
    /// cleanup (inline `$ref`, strip `$schema`/`definitions`) applies.
    AnthropicNative,
    /// OpenAI `{type:"function", function:{..., strict:true}}`. Strict mode
    /// requires `additionalProperties:false` and a full `required` list on
    /// every object node.
    OpenAiStrict,
    /// OpenAI `{type:"function", function:{...}}` without `strict`. The
    /// schema passes through apart from the common cleanup — in particular
    /// `additionalProperties` is preserved exactly as written.
    OpenAiLoose,
}

/// Normalize once, at the only three places schemas cross the provider
/// boundary. Returns the full provider-shaped tool JSON for `dialect`.
///
/// For every dialect: tool names are sanitized to `^[a-zA-Z0-9_-]{1,64}$`
/// and collisions deduplicated with a numeric suffix. All 34 shipped tools
/// already have valid names, so today this is the identity on names; it
/// guards the MCP/custom-tool path (F-A11). If a rename ever fires for a
/// dispatchable tool, dispatch needs a reverse map — that wiring lands with
/// MCP itself, which is currently dead code (§9).
pub fn adapt_tools(tools: &[ToolDefinition], dialect: SchemaDialect) -> Vec<Value> {
    let mut used_names: HashSet<String> = HashSet::new();
    tools
        .iter()
        .map(|t| {
            let name = unique_name(sanitize_name(&t.name), &mut used_names);
            let schema = adapt_schema(&t.input_schema, dialect);
            match dialect {
                SchemaDialect::AnthropicNative => json!({
                    "name": name,
                    "description": t.description,
                    "input_schema": schema,
                }),
                SchemaDialect::OpenAiLoose => json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.description,
                        "parameters": schema,
                    }
                }),
                SchemaDialect::OpenAiStrict => json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.description,
                        "parameters": schema,
                        "strict": true,
                    }
                }),
            }
        })
        .collect()
}

/// Map every character outside `[a-zA-Z0-9_-]` to `_`, cap at 64, and never
/// return the empty string (an empty name is rejected by every provider).
fn sanitize_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.truncate(64);
    if s.is_empty() {
        s.push_str("tool");
    }
    s
}

/// Deduplicate post-sanitization collisions with `_2`, `_3`, … while keeping
/// the result within the 64-char cap.
fn unique_name(base: String, used: &mut HashSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    for n in 2u32.. {
        let suffix = format!("_{n}");
        let mut candidate = base.clone();
        candidate.truncate(64 - suffix.len());
        candidate.push_str(&suffix);
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("u32 suffixes exhausted")
}

/// Adapt one `input_schema` for `dialect`. Non-object schemas (`true`,
/// `false`, or malformed) pass through untouched — there is nothing to strip
/// and inventing structure here would be worse than sending them as-is.
fn adapt_schema(schema: &Value, dialect: SchemaDialect) -> Value {
    let definitions = collect_definitions(schema);
    let mut stack: Vec<String> = Vec::new();
    rewrite(schema, dialect, &definitions, &mut stack)
}

/// The `$ref` targets reachable from the schema root: `definitions` (draft-07,
/// what schemars 0.8 emits) and `$defs` (2019-09+).
fn collect_definitions(schema: &Value) -> Map<String, Value> {
    let mut defs = Map::new();
    for key in ["definitions", "$defs"] {
        if let Some(Value::Object(m)) = schema.get(key) {
            for (k, v) in m {
                defs.insert(k.clone(), v.clone());
            }
        }
    }
    defs
}

fn rewrite(
    node: &Value,
    dialect: SchemaDialect,
    defs: &Map<String, Value>,
    stack: &mut Vec<String>,
) -> Value {
    match node {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| rewrite(v, dialect, defs, stack))
                .collect(),
        ),
        Value::Object(map) => {
            // `$ref` first: draft-07 semantics replace the whole node with the
            // resolved target (siblings are ignored). A cyclic or unresolvable
            // ref degrades to the node minus its `$ref` key — the key itself
            // must go, and an empty `{}` is a permissive schema, which the
            // tool's own deserializer still backstops.
            if let Some(Value::String(r)) = map.get("$ref") {
                if let Some(def_name) = local_def_name(r) {
                    if !stack.iter().any(|s| s == def_name) {
                        if let Some(target) = defs.get(def_name) {
                            stack.push(def_name.to_string());
                            let resolved = rewrite(target, dialect, defs, stack);
                            stack.pop();
                            return resolved;
                        }
                    }
                }
            }

            let mut out = Map::new();
            for (k, v) in map {
                // Stripped in every dialect: `$ref` survives only via the
                // resolution above; `$schema` is noise everywhere;
                // `definitions`/`$defs` are dead once refs are inlined.
                if k == "$ref" || k == "$schema" || k == "definitions" || k == "$defs" {
                    continue;
                }
                out.insert(k.clone(), rewrite(v, dialect, defs, stack));
            }

            if dialect == SchemaDialect::OpenAiStrict {
                let is_object_node = out.contains_key("properties")
                    || out.get("type").and_then(Value::as_str) == Some("object");
                if is_object_node {
                    out.insert("additionalProperties".to_string(), Value::Bool(false));
                    let all_props: Vec<Value> = out
                        .get("properties")
                        .and_then(Value::as_object)
                        .map(|p| p.keys().cloned().map(Value::String).collect())
                        .unwrap_or_default();
                    out.insert("required".to_string(), Value::Array(all_props));
                }
            }

            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// `#/definitions/Name` or `#/$defs/Name` → `Name`. Anything else (external
/// URLs, JSON-pointer paths into the schema body) is not resolvable here.
fn local_def_name(r: &str) -> Option<&str> {
    r.strip_prefix("#/definitions/")
        .or_else(|| r.strip_prefix("#/$defs/"))
        .filter(|name| !name.is_empty() && !name.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape schemars 0.8's `schema_for!` emits: `$schema` at the root, a
    /// `$ref` inside `properties`, the target under `definitions`, plus
    /// constructs that must survive untouched.
    fn schemars_like_tool() -> ToolDefinition {
        ToolDefinition {
            name: "Read".to_string(),
            description: "Reads a file".to_string(),
            input_schema: json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "file_path": { "type": "string", "format": "path", "default": "/" },
                    "mode": { "enum": ["full", "head"] },
                    "range": { "$ref": "#/definitions/Range" },
                    "variant": { "oneOf": [ { "type": "string" }, { "type": "integer" } ] },
                    "alt": { "anyOf": [ { "type": "string" }, { "type": "null" } ] },
                },
                "required": ["file_path"],
                "definitions": {
                    "Range": {
                        "type": "object",
                        "properties": {
                            "start": { "type": "integer", "minimum": 0 },
                            "end": { "type": "integer" },
                        },
                        "required": ["start"],
                    }
                }
            }),
        }
    }

    fn adapt_one(dialect: SchemaDialect) -> Value {
        adapt_tools(&[schemars_like_tool()], dialect)
            .pop()
            .expect("one tool in, one tool out")
    }

    fn schema_of(tool: &Value, dialect: SchemaDialect) -> &Value {
        match dialect {
            SchemaDialect::AnthropicNative => &tool["input_schema"],
            SchemaDialect::OpenAiLoose | SchemaDialect::OpenAiStrict => {
                &tool["function"]["parameters"]
            }
        }
    }

    /// True if `key` appears as an object key anywhere in the tree.
    fn contains_key(v: &Value, key: &str) -> bool {
        match v {
            Value::Object(m) => m.contains_key(key) || m.values().any(|v| contains_key(v, key)),
            Value::Array(items) => items.iter().any(|v| contains_key(v, key)),
            _ => false,
        }
    }

    // ─── OpenAiStrict ────────────────────────────────────────────────────────

    #[test]
    fn strict_forces_additional_properties_false_and_full_required_recursively() {
        let tool = adapt_one(SchemaDialect::OpenAiStrict);
        let schema = schema_of(&tool, SchemaDialect::OpenAiStrict);
        assert_eq!(schema["additionalProperties"], json!(false));
        let mut required: Vec<&str> = schema["required"]
            .as_array()
            .expect("strict requires a full required list")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        required.sort_unstable();
        assert_eq!(required, ["alt", "file_path", "mode", "range", "variant"]);
        // The inlined nested object gets the same treatment.
        let nested = &schema["properties"]["range"];
        assert_eq!(nested["additionalProperties"], json!(false));
        let mut nested_req: Vec<&str> = nested["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        nested_req.sort_unstable();
        assert_eq!(nested_req, ["end", "start"]);
        assert_eq!(tool["function"]["strict"], json!(true));
    }

    // ─── OpenAiLoose / AnthropicNative: common cleanup only ──────────────────

    #[test]
    fn loose_and_native_inline_refs_strip_schema_and_touch_nothing_else() {
        for dialect in [SchemaDialect::OpenAiLoose, SchemaDialect::AnthropicNative] {
            let tool = adapt_one(dialect);
            let schema = schema_of(&tool, dialect);
            assert!(!contains_key(schema, "$schema"), "{dialect:?}: {schema:#}");
            assert!(!contains_key(schema, "$ref"), "{dialect:?}: {schema:#}");
            assert!(
                !contains_key(schema, "definitions"),
                "{dialect:?}: {schema:#}"
            );
            // Preserved exactly as written — loose must NOT invent strict
            // constraints, and the author's `required` list survives.
            assert_eq!(schema["additionalProperties"], json!(false), "{dialect:?}");
            assert_eq!(schema["required"], json!(["file_path"]), "{dialect:?}");
            assert_eq!(
                schema["properties"]["range"]["properties"]["end"]["type"], "integer",
                "{dialect:?}: the ref target must be inlined in place"
            );
        }
    }

    #[test]
    fn wrappers_match_each_provider_wire_shape() {
        let native = adapt_one(SchemaDialect::AnthropicNative);
        assert!(native.get("input_schema").is_some());
        assert!(native.get("type").is_none());

        let loose = adapt_one(SchemaDialect::OpenAiLoose);
        assert_eq!(loose["type"], "function");
        assert_eq!(loose["function"]["name"], "Read");
        assert!(loose["function"].get("strict").is_none());
    }

    // ─── Names ───────────────────────────────────────────────────────────────

    #[test]
    fn invalid_names_are_sanitized_and_collisions_deduped() {
        let tool = |name: &str| ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
        };
        let tools = [tool("my.tool"), tool("my tool"), tool("my_tool"), tool("")];
        let out = adapt_tools(&tools, SchemaDialect::AnthropicNative);
        let names: Vec<&str> = out.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["my_tool", "my_tool_2", "my_tool_3", "tool"]);
        let re_valid = |n: &str| {
            !n.is_empty()
                && n.len() <= 64
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        assert!(names.iter().all(|n| re_valid(n)), "{names:?}");
    }

    #[test]
    fn valid_names_pass_through_untouched_and_long_names_are_capped() {
        assert_eq!(sanitize_name("Read"), "Read");
        assert_eq!(sanitize_name("mcp__server-1_list"), "mcp__server-1_list");
        let long = "x".repeat(80);
        assert_eq!(sanitize_name(&long).len(), 64);
    }

    // ─── Degenerate schemas ──────────────────────────────────────────────────

    #[test]
    fn ref_cycle_terminates_and_the_ref_key_still_dies() {
        let tool = ToolDefinition {
            name: "cyclic".to_string(),
            description: String::new(),
            input_schema: json!({
                "type": "object",
                "properties": { "node": { "$ref": "#/definitions/Node" } },
                "definitions": {
                    "Node": {
                        "type": "object",
                        "properties": { "next": { "$ref": "#/definitions/Node" } }
                    }
                }
            }),
        };
        let out = adapt_tools(&[tool], SchemaDialect::AnthropicNative);
        let schema = &out[0]["input_schema"];
        // One level inlined; the cyclic re-entry degraded to a permissive
        // node; and no `$ref` key survived anywhere.
        assert_eq!(schema["properties"]["node"]["type"], "object");
        assert!(!contains_key(schema, "$ref"), "{schema:#}");
        assert!(!contains_key(schema, "definitions"), "{schema:#}");
    }

    #[test]
    fn unresolvable_ref_is_dropped_not_kept() {
        let tool = ToolDefinition {
            name: "ext".to_string(),
            description: String::new(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "$ref": "https://example.com/schema.json", "description": "kept" }
                }
            }),
        };
        let out = adapt_tools(&[tool], SchemaDialect::AnthropicNative);
        let schema = &out[0]["input_schema"];
        assert!(!contains_key(schema, "$ref"), "{schema:#}");
        assert_eq!(schema["properties"]["x"]["description"], "kept");
    }

    #[test]
    fn non_object_schema_passes_through() {
        let tool = ToolDefinition {
            name: "odd".to_string(),
            description: String::new(),
            input_schema: json!(true),
        };
        let out = adapt_tools(&[tool], SchemaDialect::AnthropicNative);
        assert_eq!(out[0]["input_schema"], json!(true));
    }
}
