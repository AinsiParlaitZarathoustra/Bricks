//! Edits of `providers.toml` as text. One provider or model table changes;
//! the rest of the document (comments, order, unknown keys, other tables)
//! is kept byte for byte. The result is validated as a whole by the
//! provider crate before anything is written.

use crate::contract::{IpcErrorCode, IpcResult, ModelPatch, ProviderPatch};
use crate::services::{error, invalid, not_found};
use cersei_provider::config::ProvidersConfig;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

/// One change of the providers file, addressed by ids.
#[derive(Debug, Clone)]
pub enum Edit {
    ProviderUpdate {
        provider_id: String,
        patch: ProviderPatch,
    },
    ModelUpdate {
        provider_id: String,
        model_id: String,
        patch: ModelPatch,
    },
    ProviderDelete {
        provider_id: String,
    },
    ModelDelete {
        provider_id: String,
        model_id: String,
    },
}

/// The text after `edit`, validated as a whole. `origin` labels the errors
/// (the file's path). On error nothing is returned, so nothing is written.
pub fn apply(text: &str, edit: &Edit, origin: &str) -> IpcResult<String> {
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        error(
            IpcErrorCode::Config,
            format!("{origin} : TOML invalide ({})", position(text, &e)),
        )
    })?;
    match edit {
        Edit::ProviderUpdate { provider_id, patch } => {
            let providers = providers_mut(&mut doc)?;
            let index = provider_index(providers, provider_id)?;
            let table = providers
                .get_mut(index)
                .ok_or_else(|| not_found(UNKNOWN_PROVIDER))?;
            apply_provider_patch(table, patch)?;
        }
        Edit::ModelUpdate {
            provider_id,
            model_id,
            patch,
        } => {
            let providers = providers_mut(&mut doc)?;
            let index = provider_index(providers, provider_id)?;
            let table = providers
                .get_mut(index)
                .ok_or_else(|| not_found(UNKNOWN_PROVIDER))?;
            let models = models_mut(table)?;
            let index = model_index(models, model_id)?;
            let model = models
                .get_mut(index)
                .ok_or_else(|| not_found(UNKNOWN_MODEL))?;
            apply_model_patch(model, patch)?;
        }
        Edit::ProviderDelete { provider_id } => {
            let providers = providers_mut(&mut doc)?;
            let index = provider_index(providers, provider_id)?;
            providers.remove(index);
        }
        Edit::ModelDelete {
            provider_id,
            model_id,
        } => {
            let providers = providers_mut(&mut doc)?;
            let index = provider_index(providers, provider_id)?;
            let table = providers
                .get_mut(index)
                .ok_or_else(|| not_found(UNKNOWN_PROVIDER))?;
            let models = models_mut(table)?;
            let index = model_index(models, model_id)?;
            models.remove(index);
        }
    }
    let next = doc.to_string();
    // The whole result, as the engine will read it: a refused change never
    // reaches the file. Messages of the validator name the field and never
    // carry a secret value.
    ProvidersConfig::from_toml_str(&next, origin)
        .map_err(|e| invalid(format!("modification refusée : {e}")))?;
    Ok(next)
}

const UNKNOWN_PROVIDER: &str = "fournisseur inconnu dans providers.toml";
const UNKNOWN_MODEL: &str = "modèle inconnu dans ce fournisseur";

/// `line N, column M` of a TOML error, from its span: never the source text
/// (a line may hold a key).
fn position(text: &str, e: &toml_edit::TomlError) -> String {
    match e.span() {
        Some(span) => {
            let offset = span.start.min(text.len());
            let line = text.as_bytes()[..offset]
                .iter()
                .filter(|b| **b == b'\n')
                .count()
                + 1;
            format!("ligne {line} : {}", e.message())
        }
        None => e.message().to_string(),
    }
}

fn providers_mut(doc: &mut DocumentMut) -> IpcResult<&mut ArrayOfTables> {
    match doc.get_mut("providers") {
        None => Err(not_found(UNKNOWN_PROVIDER)),
        Some(item) => item.as_array_of_tables_mut().ok_or_else(inline_unsupported),
    }
}

fn models_mut(provider: &mut Table) -> IpcResult<&mut ArrayOfTables> {
    match provider.get_mut("models") {
        None => Err(not_found(UNKNOWN_MODEL)),
        Some(item) => item.as_array_of_tables_mut().ok_or_else(inline_unsupported),
    }
}

fn inline_unsupported() -> crate::contract::IpcError {
    error(
        IpcErrorCode::Config,
        "providers.toml : les fournisseurs et les modèles doivent être écrits en tables \
         [[providers]] et [[providers.models]] pour être modifiés depuis l'application",
    )
}

fn table_id(table: &Table) -> Option<&str> {
    table.get("id").and_then(Item::as_str)
}

fn provider_index(providers: &ArrayOfTables, id: &str) -> IpcResult<usize> {
    providers
        .iter()
        .position(|t| table_id(t) == Some(id))
        .ok_or_else(|| not_found(UNKNOWN_PROVIDER))
}

fn model_index(models: &ArrayOfTables, id: &str) -> IpcResult<usize> {
    models
        .iter()
        .position(|t| table_id(t) == Some(id))
        .ok_or_else(|| not_found(UNKNOWN_MODEL))
}

/// Sets a string value. An existing value keeps its own decoration (a
/// trailing comment on its line).
fn set_value(table: &mut dyn TableLike, key: &str, new: Value) {
    if let Some(item) = table.get_mut(key)
        && let Some(old) = item.as_value_mut()
    {
        let decor = old.decor().clone();
        *old = new;
        *old.decor_mut() = decor;
        return;
    }
    table.insert(key, Item::Value(new));
}

fn set_str(table: &mut dyn TableLike, key: &str, value: &str) {
    set_value(table, key, Value::from(value));
}

/// An empty string removes the key (the optional fields have no other way
/// to be cleared).
fn set_or_remove(table: &mut dyn TableLike, key: &str, value: &str) {
    if value.is_empty() {
        table.remove(key);
    } else {
        set_str(table, key, value);
    }
}

fn set_count(table: &mut dyn TableLike, key: &str, value: u64) -> IpcResult<()> {
    let n = i64::try_from(value).map_err(|_| invalid(format!("`{key}` est trop grand")))?;
    set_value(table, key, Value::from(n));
    Ok(())
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A text field the form sends: never empty (the form validates required
/// fields before sending; an empty value here is a refusal, not a removal).
fn filled<'a>(field: &str, value: &'a str) -> IpcResult<&'a str> {
    if value.trim().is_empty() {
        return Err(invalid(format!(
            "le champ « {field} » ne peut pas être vide"
        )));
    }
    Ok(value)
}

fn apply_provider_patch(table: &mut Table, patch: &ProviderPatch) -> IpcResult<()> {
    if let Some(v) = &patch.name {
        set_str(table, "name", filled("name", v)?);
    }
    if let Some(v) = &patch.endpoint {
        set_str(table, "endpoint", filled("endpoint", v)?);
    }
    if let Some(v) = &patch.protocol {
        set_str(table, "protocol", filled("protocol", v)?);
    }
    if let Some(v) = &patch.auth {
        set_str(table, "auth", filled("auth", v)?);
    }
    // The one field an empty value clears: the default header then applies.
    if let Some(v) = &patch.auth_header {
        set_or_remove(table, "auth_header", v);
    }
    if let Some(v) = &patch.api_key_env {
        let v = filled("variable d'environnement de la clé", v)?;
        if table.contains_key("api_key") {
            return Err(invalid(
                "cette clé est écrite dans le fichier : une variable d'environnement ne peut \
                 pas lui être ajoutée (la clé inline doit d'abord être retirée à la main)",
            ));
        }
        if !valid_env_name(v) {
            return Err(invalid(
                "le nom de la variable d'environnement doit être un identifiant (lettres, \
                 chiffres et _)",
            ));
        }
        set_str(table, "api_key_env", v);
    }
    Ok(())
}

fn apply_model_patch(table: &mut Table, patch: &ModelPatch) -> IpcResult<()> {
    if let Some(v) = &patch.name {
        set_str(table, "name", filled("name", v)?);
    }
    if let Some(v) = &patch.api_model {
        set_str(table, "api_model", filled("api_model", v)?);
    }
    if let Some(mods) = &patch.input_modalities {
        let mut list = Array::new();
        for m in mods {
            list.push(m.as_str());
        }
        set_value(table, "input_modalities", Value::Array(list));
    }
    if patch.max_input_tokens.is_some()
        || patch.max_output_tokens.is_some()
        || patch.context_window_tokens.is_some()
        || patch.clear_context_window
    {
        let limits = table
            .get_mut("limits")
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| error(IpcErrorCode::Config, "ce modèle n'a pas de table [limits]"))?;
        if let Some(v) = patch.max_input_tokens {
            set_count(limits, "max_input_tokens", v)?;
        }
        if let Some(v) = patch.max_output_tokens {
            set_count(limits, "max_output_tokens", v)?;
        }
        if patch.clear_context_window && patch.context_window_tokens.is_some() {
            return Err(invalid(
                "fenêtre de contexte : valeur et suppression demandées ensemble",
            ));
        }
        if patch.clear_context_window {
            limits.remove("context_window_tokens");
        } else if let Some(v) = patch.context_window_tokens {
            set_count(limits, "context_window_tokens", v)?;
        }
    }
    if patch.clear_reasoning_default && patch.reasoning_default.is_some() {
        return Err(invalid(
            "profil par défaut : valeur et suppression demandées ensemble",
        ));
    }
    if patch.clear_reasoning_default {
        if let Some(reasoning) = table.get_mut("reasoning").and_then(Item::as_table_like_mut) {
            reasoning.remove("default");
        }
    } else if let Some(v) = &patch.reasoning_default {
        let v = filled("profil par défaut", v)?;
        if !table.contains_key("reasoning") {
            table.insert("reasoning", Item::Table(Table::new()));
        }
        let reasoning = table
            .get_mut("reasoning")
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| {
                error(
                    IpcErrorCode::Config,
                    "ce modèle : [reasoning] n'est pas une table",
                )
            })?;
        set_str(reasoning, "default", v);
    }
    Ok(())
}

/// Test fixtures shared with the command tests.
#[cfg(test)]
pub(crate) mod fixtures {
    /// Three providers: a local one with two models (one with a reasoning
    /// profile), a cloud one referring to an environment variable, and one
    /// with an inline key and a header value (secrets that must never leave
    /// the engine). Comments everywhere, so their survival can be checked.
    pub const PROVIDERS: &str = r#"# Fournisseurs de test : en-tête à conserver
schema_version = 1

# Le fournisseur local
[[providers]]
id = "local"
name = "Local"   # libellé affiché
endpoint = "http://127.0.0.1:9/v1"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "small"
name = "Small"
api_model = "small-1"
streaming = true
tool_calls = true
[providers.models.limits]
max_input_tokens = 8000
max_output_tokens = 1000
[providers.models.reasoning]
default = "fast"
[[providers.models.reasoning.profiles]]
id = "fast"
label = "Rapide"

# Un modèle qui sera supprimé
[[providers.models]]
id = "big"
name = "Big"
api_model = "big-1"
[providers.models.limits]
max_input_tokens = 32000
max_output_tokens = 4000

# Le fournisseur cloud : sa clé vient de l'environnement
[[providers]]
id = "cloud"
name = "Cloud"
endpoint = "https://api.example.invalid/v1"
protocol = "anthropic_messages"
auth = "api_key_header"
auth_header = "x-api-key"
api_key_env = "BRICKS_TEST_CLOUD_KEY"

[[providers.models]]
id = "m1"
name = "M1"
api_model = "m1"
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100

# Un fournisseur à clé écrite dans le fichier
[[providers]]
id = "inline"
name = "Inline"
endpoint = "https://inline.example.invalid/v1"
protocol = "chat_completions"
auth = "bearer"
api_key = "SECRET-KEY-VALUE"

[providers.headers]
X-Tenant = "HEADER-SECRET-VALUE"

[[providers.models]]
id = "i1"
name = "I1"
api_model = "i1"
[providers.models.limits]
max_input_tokens = 1000
max_output_tokens = 100
"#;
}

#[cfg(test)]
mod tests {
    use super::fixtures::PROVIDERS;
    use super::*;

    fn model_update(provider: &str, model: &str, patch: ModelPatch) -> Edit {
        Edit::ModelUpdate {
            provider_id: provider.into(),
            model_id: model.into(),
            patch,
        }
    }

    fn provider_update(provider: &str, patch: ProviderPatch) -> Edit {
        Edit::ProviderUpdate {
            provider_id: provider.into(),
            patch,
        }
    }

    #[test]
    fn a_model_update_changes_only_its_own_value() {
        let edit = model_update(
            "local",
            "small",
            ModelPatch {
                max_input_tokens: Some(9000),
                ..Default::default()
            },
        );
        let next = apply(PROVIDERS, &edit, "test").unwrap();
        assert_eq!(
            next,
            PROVIDERS.replace("max_input_tokens = 8000", "max_input_tokens = 9000")
        );
    }

    #[test]
    fn a_cleared_reasoning_default_is_removed_and_the_profiles_stay() {
        let edit = model_update(
            "local",
            "small",
            ModelPatch {
                clear_reasoning_default: true,
                ..Default::default()
            },
        );
        let next = apply(PROVIDERS, &edit, "test").unwrap();
        assert!(!next.contains("default = \"fast\""));
        assert!(next.contains("[[providers.models.reasoning.profiles]]"));
        assert_eq!(next, PROVIDERS.replace("default = \"fast\"\n", ""));
    }

    #[test]
    fn an_optional_limit_is_added_then_removed() {
        let set = model_update(
            "local",
            "big",
            ModelPatch {
                context_window_tokens: Some(40000),
                ..Default::default()
            },
        );
        let with = apply(PROVIDERS, &set, "test").unwrap();
        assert!(with.contains("context_window_tokens = 40000"));
        let clear = model_update(
            "local",
            "big",
            ModelPatch {
                clear_context_window: true,
                ..Default::default()
            },
        );
        let without = apply(&with, &clear, "test").unwrap();
        assert_eq!(
            without, PROVIDERS,
            "adding then clearing gives the file back"
        );
    }

    #[test]
    fn an_environment_variable_name_is_checked_and_refused_beside_an_inline_key() {
        for name in ["bad name", "1STARTS_WITH_DIGIT", "A-DASH"] {
            let err = apply(
                PROVIDERS,
                &provider_update(
                    "cloud",
                    ProviderPatch {
                        api_key_env: Some(name.into()),
                        ..Default::default()
                    },
                ),
                "test",
            )
            .unwrap_err();
            assert_eq!(err.code, IpcErrorCode::Invalid, "{name}");
        }
        let err = apply(
            PROVIDERS,
            &provider_update(
                "inline",
                ProviderPatch {
                    api_key_env: Some("BRICKS_OK".into()),
                    ..Default::default()
                },
            ),
            "test",
        )
        .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid);
        assert!(!err.message.contains("SECRET-KEY-VALUE"));
    }

    #[test]
    fn an_unknown_provider_or_model_is_not_found() {
        let err = apply(
            PROVIDERS,
            &provider_update("nope", ProviderPatch::default()),
            "test",
        )
        .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::NotFound);
        let err = apply(
            PROVIDERS,
            &model_update("local", "nope", ModelPatch::default()),
            "test",
        )
        .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::NotFound);
    }

    #[test]
    fn a_change_valid_alone_but_not_as_a_whole_is_refused() {
        // `local` has no key: asking for an api_key_header auth without a
        // header and without a key is refused by the validator of the file.
        let err = apply(
            PROVIDERS,
            &provider_update(
                "local",
                ProviderPatch {
                    auth: Some("api_key_header".into()),
                    ..Default::default()
                },
            ),
            "test",
        )
        .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid, "{}", err.message);
    }

    #[test]
    fn a_broken_file_is_refused_without_quoting_its_line() {
        let text = "schema_version = 1\n[[providers]]\napi_key = \"SECRET-LINE\" ]\n";
        let err = apply(
            text,
            &Edit::ProviderDelete {
                provider_id: "x".into(),
            },
            "test",
        )
        .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Config);
        assert!(!err.message.contains("SECRET-LINE"), "{}", err.message);
        assert!(err.message.contains("ligne 3"), "{}", err.message);
    }

    #[test]
    fn deleting_every_model_keeps_the_provider_valid() {
        let mut text = PROVIDERS.to_string();
        for model in ["small", "big"] {
            text = apply(
                &text,
                &Edit::ModelDelete {
                    provider_id: "local".into(),
                    model_id: model.into(),
                },
                "test",
            )
            .unwrap();
        }
        assert!(text.contains("id = \"local\""), "the provider stays");
        assert!(!text.contains("small-1"));
    }

    #[test]
    fn an_empty_auth_header_removes_the_key_and_the_default_header_applies() {
        // `cloud` sends `x-api-key` explicitly; clearing it leaves the default.
        let edit = provider_update(
            "cloud",
            ProviderPatch {
                auth_header: Some(String::new()),
                ..Default::default()
            },
        );
        let next = apply(PROVIDERS, &edit, "test").expect("valid without auth_header");
        assert!(
            !next.contains("auth_header = \"x-api-key\""),
            "the key is gone"
        );
        assert!(
            next.contains("api_key_env = \"BRICKS_TEST_CLOUD_KEY\""),
            "the rest is kept"
        );
        assert_eq!(next, PROVIDERS.replace("auth_header = \"x-api-key\"\n", ""));
    }

    #[test]
    fn an_empty_text_field_is_refused_with_a_french_message_naming_it() {
        let empty = |s: &str| s.to_string();
        let cases: Vec<(Edit, &str)> = vec![
            (
                provider_update(
                    "local",
                    ProviderPatch {
                        name: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "name",
            ),
            (
                provider_update(
                    "local",
                    ProviderPatch {
                        name: Some(empty("   ")),
                        ..Default::default()
                    },
                ),
                "name",
            ),
            (
                provider_update(
                    "local",
                    ProviderPatch {
                        endpoint: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "endpoint",
            ),
            (
                provider_update(
                    "local",
                    ProviderPatch {
                        protocol: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "protocol",
            ),
            (
                provider_update(
                    "local",
                    ProviderPatch {
                        auth: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "auth",
            ),
            (
                provider_update(
                    "cloud",
                    ProviderPatch {
                        api_key_env: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "variable",
            ),
            (
                model_update(
                    "local",
                    "small",
                    ModelPatch {
                        name: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "name",
            ),
            (
                model_update(
                    "local",
                    "small",
                    ModelPatch {
                        api_model: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "api_model",
            ),
            (
                model_update(
                    "local",
                    "small",
                    ModelPatch {
                        reasoning_default: Some(empty("")),
                        ..Default::default()
                    },
                ),
                "profil",
            ),
        ];
        for (edit, field) in cases {
            let err = apply(PROVIDERS, &edit, "test").expect_err("an empty value is refused");
            assert_eq!(err.code, IpcErrorCode::Invalid, "{edit:?}");
            assert!(
                err.message.contains("ne peut pas être vide"),
                "{}",
                err.message
            );
            assert!(
                err.message.contains(field),
                "names the field: {}",
                err.message
            );
        }
    }
}
