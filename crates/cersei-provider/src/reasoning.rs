//! Reasoning profiles: user-defined bundles of request parameters.
//!
//! A profile is nothing but a name, a label and JSON that is merged into the
//! request. Names such as `low`, `high` or `ultra` are suggestions a user may
//! use, rename, drop or extend; Bricks neither enumerates them nor sends them
//! to the server under their local name. What reaches the wire is exactly what
//! the profile's `parameters` say (for example `reasoning.effort = "high"` or a
//! thinking budget the server documents). The server remains the authority on
//! which values it accepts — nothing here validates or substitutes them.
//!
//! Merge order, applied on top of the request the protocol adapter built:
//!
//! 0. the model's `remove_parameters` (JSON Pointers; lets a model drop an
//!    option the engine sets but the server rejects);
//! 1. the model's `parameters`;
//! 2. the profile's `remove` (JSON Pointers);
//! 3. the profile's `parameters`.
//!
//! Objects merge recursively; scalars and arrays replace. Structural fields
//! (model, messages/input, system prompt, tools, tool choice, streaming
//! control) cannot be set or removed.

use crate::config::{pointer_root, Protocol, ReasoningConfig, ReasoningProfile};
use serde_json::{Map, Value};

/// Pick the profile for a request: an explicit id, else the model's default,
/// else none. An unknown id is an error — a refused profile is never replaced
/// silently by another one.
pub fn select_profile<'a>(
    reasoning: &'a ReasoningConfig,
    requested: Option<&str>,
) -> Result<Option<&'a ReasoningProfile>, String> {
    match requested {
        Some(id) => reasoning
            .profiles
            .iter()
            .find(|p| p.id == id)
            .map(Some)
            .ok_or_else(|| {
                if reasoning.profiles.is_empty() {
                    format!(
                        "reasoning profile `{id}` requested, but this model exposes no profiles"
                    )
                } else {
                    format!(
                        "unknown reasoning profile `{id}`; available: {}",
                        reasoning
                            .profiles
                            .iter()
                            .map(|p| p.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }),
        None => Ok(reasoning
            .default
            .as_deref()
            .and_then(|d| reasoning.profiles.iter().find(|p| p.id == d))),
    }
}

/// Recursively merge `patch` into `target`: objects merge key by key, anything
/// else (scalars, arrays, null) replaces.
pub fn deep_merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                match t.get_mut(k) {
                    Some(existing) => deep_merge(existing, v),
                    None => {
                        t.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

/// Remove the value a JSON Pointer designates. A pointer to nothing is a no-op.
pub fn remove_pointer(root: &mut Value, pointer: &str) {
    let Some((parent_ptr, last)) = pointer.rsplit_once('/') else {
        return;
    };
    let last = last.replace("~1", "/").replace("~0", "~");
    let Some(parent) = (if parent_ptr.is_empty() {
        Some(root)
    } else {
        root.pointer_mut(parent_ptr)
    }) else {
        return;
    };
    match parent {
        Value::Object(map) => {
            map.remove(&last);
        }
        Value::Array(items) => {
            if let Ok(i) = last.parse::<usize>() {
                if i < items.len() {
                    items.remove(i);
                }
            }
        }
        _ => {}
    }
}

fn guard_protected(protocol: Protocol, keys: impl Iterator<Item = String>) -> Result<(), String> {
    let protected = protocol.protected_fields();
    for k in keys {
        if protected.contains(&k.as_str()) {
            return Err(format!(
                "`{k}` is a structural field of the `{protocol}` protocol and cannot be changed \
                 by parameters or reasoning profiles"
            ));
        }
    }
    Ok(())
}

/// Apply the model parameters and the (optional) profile to a request body.
pub fn apply(
    body: &mut Value,
    protocol: Protocol,
    model_parameters: &Map<String, Value>,
    model_remove: &[String],
    profile: Option<&ReasoningProfile>,
) -> Result<(), String> {
    // Config validation already rejects these; enforce again here so a
    // hand-built configuration cannot overwrite engine-built content.
    guard_protected(protocol, model_parameters.keys().cloned())?;
    guard_protected(
        protocol,
        model_remove.iter().map(|r| pointer_root(r).to_string()),
    )?;
    if let Some(p) = profile {
        guard_protected(protocol, p.parameters.keys().cloned())?;
        guard_protected(
            protocol,
            p.remove.iter().map(|r| pointer_root(r).to_string()),
        )?;
    }

    for ptr in model_remove {
        remove_pointer(body, ptr);
    }
    deep_merge(body, &Value::Object(model_parameters.clone()));
    if let Some(p) = profile {
        for ptr in &p.remove {
            remove_pointer(body, ptr);
        }
        deep_merge(body, &Value::Object(p.parameters.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(id: &str, params: Value, remove: &[&str]) -> ReasoningProfile {
        ReasoningProfile {
            id: id.into(),
            label: None,
            parameters: params.as_object().cloned().unwrap_or_default(),
            remove: remove.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn cfg(default: Option<&str>, profiles: Vec<ReasoningProfile>) -> ReasoningConfig {
        ReasoningConfig {
            default: default.map(String::from),
            profiles,
        }
    }

    #[test]
    fn merge_is_recursive_for_objects_and_replacing_for_the_rest() {
        let mut body = json!({"a": {"x": 1, "y": [1, 2]}, "s": "old", "keep": true});
        deep_merge(
            &mut body,
            &json!({"a": {"y": [9], "z": 3}, "s": {"now": "object"}}),
        );
        assert_eq!(
            body,
            json!({"a": {"x": 1, "y": [9], "z": 3}, "s": {"now": "object"}, "keep": true})
        );
    }

    #[test]
    fn order_is_adapter_then_model_then_removals_then_profile() {
        let mut body = json!({"model": "m", "temperature": 0.7, "reasoning": {"effort": "adapter", "summary": "auto"}});
        let model_params = json!({"reasoning": {"effort": "model", "extra": 1}, "top_p": 0.9});
        let p = profile(
            "deep",
            json!({"reasoning": {"effort": "high"}}),
            &["/temperature", "/reasoning/summary"],
        );
        apply(
            &mut body,
            Protocol::Responses,
            model_params.as_object().unwrap(),
            &[],
            Some(&p),
        )
        .unwrap();
        assert_eq!(
            body,
            json!({"model": "m", "top_p": 0.9, "reasoning": {"effort": "high", "extra": 1}})
        );
    }

    #[test]
    fn removal_happens_after_model_parameters_and_before_profile_parameters() {
        // The model parameter is removed by the profile, then the profile re-adds its own.
        let mut body = json!({});
        let model_params = json!({"thinking": {"type": "enabled", "budget_tokens": 1}});
        let p = profile(
            "none",
            json!({"thinking": {"type": "disabled"}}),
            &["/thinking"],
        );
        apply(
            &mut body,
            Protocol::AnthropicMessages,
            model_params.as_object().unwrap(),
            &[],
            Some(&p),
        )
        .unwrap();
        assert_eq!(
            body,
            json!({"thinking": {"type": "disabled"}}),
            "no budget_tokens left over"
        );
    }

    #[test]
    fn model_removals_run_before_model_parameters() {
        // The model drops the engine's temperature, then sets its own.
        let mut body = json!({"temperature": 0.0, "max_tokens": 10});
        let params = json!({"temperature": 1}).as_object().cloned().unwrap();
        apply(
            &mut body,
            Protocol::ChatCompletions,
            &params,
            &["/temperature".to_string(), "/max_tokens".to_string()],
            None,
        )
        .unwrap();
        assert_eq!(body, json!({"temperature": 1}));
        // Structural fields cannot be removed at model level either.
        assert!(apply(
            &mut json!({"messages": []}),
            Protocol::ChatCompletions,
            &Map::new(),
            &["/messages".to_string()],
            None
        )
        .is_err());
    }

    #[test]
    fn remove_pointer_handles_arrays_escapes_and_misses() {
        let mut v = json!({"a/b": {"c~d": 1, "keep": 2}, "arr": [10, 20, 30]});
        remove_pointer(&mut v, "/a~1b/c~0d");
        remove_pointer(&mut v, "/arr/1");
        remove_pointer(&mut v, "/missing/deep");
        remove_pointer(&mut v, "/arr/9");
        assert_eq!(v, json!({"a/b": {"keep": 2}, "arr": [10, 30]}));
    }

    #[test]
    fn structural_fields_cannot_be_overwritten_or_removed() {
        for (k, v) in [
            ("messages", json!([])),
            ("model", json!("x")),
            ("tools", json!([])),
            ("stream", json!(false)),
        ] {
            let mut body =
                json!({"messages": [{"role": "user"}], "model": "m", "tools": [1], "stream": true});
            let before = body.clone();
            let params = Map::from_iter([(k.to_string(), v)]);
            assert!(
                apply(&mut body, Protocol::ChatCompletions, &params, &[], None).is_err(),
                "{k}"
            );
            assert_eq!(body, before, "{k}: nothing applied");
        }
        let mut body = json!({"messages": []});
        let p = profile("p", json!({}), &["/messages"]);
        assert!(apply(
            &mut body,
            Protocol::ChatCompletions,
            &Map::new(),
            &[],
            Some(&p)
        )
        .is_err());
        let p = profile("p", json!({"input": []}), &[]);
        assert!(apply(
            &mut json!({}),
            Protocol::Responses,
            &Map::new(),
            &[],
            Some(&p)
        )
        .is_err());
        // `input` is only structural for responses.
        assert!(apply(
            &mut json!({}),
            Protocol::ChatCompletions,
            &Map::new(),
            &[],
            Some(&p)
        )
        .is_ok());
    }

    #[test]
    fn selection_rules() {
        let c = cfg(
            Some("deep"),
            vec![
                profile("fast", json!({}), &[]),
                profile("deep", json!({}), &[]),
            ],
        );
        assert_eq!(select_profile(&c, None).unwrap().unwrap().id, "deep");
        assert_eq!(
            select_profile(&c, Some("fast")).unwrap().unwrap().id,
            "fast"
        );
        let err = select_profile(&c, Some("ultra")).unwrap_err();
        assert!(err.contains("unknown reasoning profile `ultra`") && err.contains("fast, deep"));
        // No default and no request -> no profile (not an implicit "off").
        let c2 = cfg(None, vec![profile("fast", json!({}), &[])]);
        assert!(select_profile(&c2, None).unwrap().is_none());
        // Empty list exposes nothing.
        let empty = cfg(None, vec![]);
        assert!(select_profile(&empty, None).unwrap().is_none());
        assert!(select_profile(&empty, Some("x"))
            .unwrap_err()
            .contains("exposes no profiles"));
    }

    #[test]
    fn profile_ids_are_never_sent_by_themselves() {
        let mut body = json!({"model": "m"});
        let p = profile("ultra", json!({}), &[]);
        apply(
            &mut body,
            Protocol::ChatCompletions,
            &Map::new(),
            &[],
            Some(&p),
        )
        .unwrap();
        assert_eq!(body, json!({"model": "m"}));
    }
}
