//! Edits of `providers.toml` as text. One provider or model table changes;
//! the rest of the document (comments, order, unknown keys, other tables)
//! is kept byte for byte. The result is validated as a whole by the
//! provider crate before anything is written.

use crate::contract::{IpcErrorCode, IpcResult, ModelPatch, ProviderPatch, ReasoningProfileEdit};
use crate::services::{error, invalid, not_found};
use cersei_provider::config::ProvidersConfig;
use serde_json::{Map, Value as Json};
use std::collections::BTreeSet;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, TableLike, Value};

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
    // The profile list is replaced as a whole. The default must still name a
    // profile of the list: a patch that drops or renames it must say what
    // the default becomes (another profile, or none).
    if let Some(profiles) = &patch.profiles {
        check_profiles(profiles)?;
        let sets_default = patch.reasoning_default.is_some() || patch.clear_reasoning_default;
        if !sets_default
            && let Some(current) = default_profile(table)
            && !profiles.iter().any(|p| p.id == current)
        {
            return Err(invalid(format!(
                "le profil par défaut « {current} » est retiré ou renommé : choisissez un autre \
                 profil par défaut, ou effacez-le dans la même modification"
            )));
        }
        if let Some(v) = &patch.reasoning_default
            && !profiles.iter().any(|p| &p.id == v)
        {
            return Err(invalid(format!(
                "le profil par défaut « {v} » n'est pas dans la liste des profils du modèle"
            )));
        }
        write_profiles(table, profiles)?;
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

/// The default profile a model names, if any.
fn default_profile(model: &Table) -> Option<String> {
    model
        .get("reasoning")
        .and_then(Item::as_table_like)
        .and_then(|r| r.get("default"))
        .and_then(Item::as_str)
        .map(str::to_string)
}

/// The checks the desktop makes before writing a profile list, in French:
/// ids (non-empty, unique), parameters (a JSON object TOML can hold) and
/// JSON Pointers. Protected fields are checked by the provider crate on the
/// whole file, before anything is written.
fn check_profiles(profiles: &[ReasoningProfileEdit]) -> IpcResult<()> {
    let mut seen = BTreeSet::new();
    for profile in profiles {
        if profile.id.trim().is_empty() {
            return Err(invalid("un profil de raisonnement a un identifiant vide"));
        }
        if !seen.insert(profile.id.as_str()) {
            return Err(invalid(format!(
                "identifiant de profil en double : « {} »",
                profile.id
            )));
        }
        let Some(parameters) = profile.parameters.as_object() else {
            return Err(invalid(format!(
                "profil « {} » : les paramètres doivent être un objet JSON",
                profile.id
            )));
        };
        for (key, value) in parameters {
            json_to_toml(value).map_err(|m| {
                invalid(format!(
                    "profil « {} », paramètre « {key} » : {m}",
                    profile.id
                ))
            })?;
        }
        for pointer in &profile.remove {
            check_pointer(pointer)
                .map_err(|m| invalid(format!("profil « {} » : {m}", profile.id)))?;
        }
    }
    Ok(())
}

/// The shape a JSON Pointer must have (RFC 6901), checked before writing.
fn check_pointer(pointer: &str) -> Result<(), String> {
    if pointer.is_empty() {
        return Err("un pointeur vide supprimerait toute la requête".into());
    }
    if !pointer.starts_with('/') {
        return Err(format!(
            "« {pointer} » n'est pas un pointeur JSON (il doit commencer par /)"
        ));
    }
    let mut chars = pointer.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.peek(), Some('0' | '1')) {
            return Err(format!(
                "« {pointer} » : le caractère ~ doit être écrit ~0 ou ~1"
            ));
        }
    }
    Ok(())
}

/// A JSON value as TOML. TOML has no null: a null is refused, not dropped.
/// Integers stay integers, other numbers become floats.
fn json_to_toml(value: &Json) -> Result<Value, String> {
    Ok(match value {
        Json::Null => {
            return Err("une valeur vide (null) ne peut pas être écrite dans providers.toml".into());
        }
        Json::Bool(b) => Value::from(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::from(i),
            None if n.is_f64() => Value::from(n.as_f64().unwrap_or_default()),
            None => return Err("nombre hors de l'intervalle des entiers TOML".into()),
        },
        Json::String(s) => Value::from(s.as_str()),
        Json::Array(items) => {
            let mut array = Array::new();
            for item in items {
                array.push(json_to_toml(item)?);
            }
            Value::Array(array)
        }
        Json::Object(map) => Value::InlineTable(object_to_inline(map)?),
    })
}

fn object_to_inline(object: &Map<String, Json>) -> Result<InlineTable, String> {
    let mut table = InlineTable::new();
    for (key, value) in object {
        table.insert(key.as_str(), json_to_toml(value)?);
    }
    Ok(table)
}

/// A TOML item as JSON, to compare it with a value without rewriting it.
/// `None` for what has no JSON form (a NaN float).
fn item_to_json(item: &Item) -> Option<Json> {
    match item {
        Item::Value(v) => value_to_json(v),
        Item::Table(t) => table_to_json(t),
        Item::ArrayOfTables(a) => a
            .iter()
            .map(|t| table_to_json(t))
            .collect::<Option<Vec<_>>>()
            .map(Json::Array),
        Item::None => None,
    }
}

fn table_to_json(table: &dyn TableLike) -> Option<Json> {
    table
        .iter()
        .map(|(k, item)| item_to_json(item).map(|v| (k.to_string(), v)))
        .collect::<Option<Map<_, _>>>()
        .map(Json::Object)
}

fn value_to_json(value: &Value) -> Option<Json> {
    Some(match value {
        Value::String(s) => Json::String(s.value().clone()),
        Value::Integer(i) => Json::from(*i.value()),
        Value::Float(f) => Json::Number(serde_json::Number::from_f64(*f.value())?),
        Value::Boolean(b) => Json::Bool(*b.value()),
        Value::Datetime(d) => Json::String(d.value().to_string()),
        Value::Array(a) => Json::Array(a.iter().map(value_to_json).collect::<Option<Vec<_>>>()?),
        Value::InlineTable(t) => Json::Object(
            t.iter()
                .map(|(k, v)| value_to_json(v).map(|j| (k.to_string(), j)))
                .collect::<Option<Map<_, _>>>()?,
        ),
    })
}

/// Equal as JSON values; `1` and `1.0` are the same number.
fn same_json(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64().is_some() && x.as_f64() == y.as_f64(),
        },
        (Json::Array(x), Json::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same_json(a, b))
        }
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => a == b,
    }
}

/// Writes the model's profile list. The list replaces the old one; a profile
/// that keeps its id keeps its table, so its comments and unchanged values
/// stay as they are. The file's own form is kept: `[[...profiles]]` tables,
/// or an inline array when the model writes its reasoning inline.
fn write_profiles(model: &mut Table, profiles: &[ReasoningProfileEdit]) -> IpcResult<()> {
    if profiles.is_empty() {
        if let Some(reasoning) = model.get_mut("reasoning").and_then(Item::as_table_like_mut) {
            reasoning.remove("profiles");
        }
        return Ok(());
    }
    let inline = match model.get("reasoning") {
        Some(Item::Value(_)) => true,
        Some(item) => matches!(
            item.as_table_like().and_then(|r| r.get("profiles")),
            Some(Item::Value(_))
        ),
        None => false,
    };
    if !model.contains_key("reasoning") {
        model.insert("reasoning", Item::Table(Table::new()));
    }
    let reasoning = model
        .get_mut("reasoning")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            error(
                IpcErrorCode::Config,
                "ce modèle : [reasoning] n'est pas une table",
            )
        })?;
    if inline {
        let mut array = Array::new();
        for profile in profiles {
            let mut table = InlineTable::new();
            write_profile(&mut table, profile)?;
            array.push(Value::InlineTable(table));
        }
        reasoning.insert("profiles", Item::Value(Value::Array(array)));
    } else {
        let old: Vec<Table> = reasoning
            .get("profiles")
            .and_then(Item::as_array_of_tables)
            .map(|a| a.iter().cloned().collect())
            .unwrap_or_default();
        let mut next = ArrayOfTables::new();
        for profile in profiles {
            let mut table = old
                .iter()
                .find(|t| table_id(t) == Some(profile.id.as_str()))
                .cloned()
                .unwrap_or_else(Table::new);
            write_profile(&mut table, profile)?;
            next.push(table);
        }
        reasoning.insert("profiles", Item::ArrayOfTables(next));
    }
    Ok(())
}

/// The fields of one profile. Values equal to the ones in the file are left
/// alone (their text and their comments included).
fn write_profile(table: &mut dyn TableLike, profile: &ReasoningProfileEdit) -> IpcResult<()> {
    if table.get("id").and_then(Item::as_str) != Some(profile.id.as_str()) {
        set_str(table, "id", &profile.id);
    }
    match profile
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        Some(label) => {
            if table.get("label").and_then(Item::as_str) != Some(label) {
                set_str(table, "label", label);
            }
        }
        None => {
            table.remove("label");
        }
    }
    let parameters = profile.parameters.as_object().cloned().unwrap_or_default();
    if parameters.is_empty() {
        table.remove("parameters");
    } else if !table
        .get("parameters")
        .and_then(item_to_json)
        .is_some_and(|old| same_json(&old, &profile.parameters))
    {
        let inline = object_to_inline(&parameters).map_err(|m| {
            invalid(format!(
                "profil « {} » : paramètres non écrivables : {m}",
                profile.id
            ))
        })?;
        table.insert("parameters", Item::Value(Value::InlineTable(inline)));
    }
    let remove = Json::Array(
        profile
            .remove
            .iter()
            .map(|p| Json::String(p.clone()))
            .collect(),
    );
    if profile.remove.is_empty() {
        table.remove("remove");
    } else if !table
        .get("remove")
        .and_then(item_to_json)
        .is_some_and(|old| same_json(&old, &remove))
    {
        let mut array = Array::new();
        for pointer in &profile.remove {
            array.push(pointer.as_str());
        }
        table.insert("remove", Item::Value(Value::Array(array)));
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
    use serde_json::json;

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

    fn profile(id: &str, label: Option<&str>, parameters: Json, remove: &[&str]) -> ReasoningProfileEdit {
        ReasoningProfileEdit {
            id: id.into(),
            label: label.map(String::from),
            parameters,
            remove: remove.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn profiles_update(provider: &str, model: &str, profiles: Vec<ReasoningProfileEdit>) -> Edit {
        model_update(
            provider,
            model,
            ModelPatch {
                profiles: Some(profiles),
                ..Default::default()
            },
        )
    }

    /// The model's profiles as the engine reads them after the write.
    fn reloaded_profiles(text: &str, provider: &str, model: &str) -> Vec<(String, Option<String>, Json, Vec<String>)> {
        let config = ProvidersConfig::from_toml_str(text, "test").expect("valid after the write");
        let m = config
            .provider(provider)
            .unwrap()
            .models
            .iter()
            .find(|m| m.id == model)
            .unwrap();
        m.reasoning
            .profiles
            .iter()
            .map(|p| {
                (
                    p.id.clone(),
                    p.label.clone(),
                    Json::Object(p.parameters.clone()),
                    p.remove.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn an_unchanged_profile_list_leaves_the_file_byte_for_byte() {
        let same = profiles_update(
            "local",
            "small",
            vec![profile("fast", Some("Rapide"), json!({}), &[])],
        );
        assert_eq!(apply(PROVIDERS, &same, "test").unwrap(), PROVIDERS);
    }

    #[test]
    fn free_names_and_faithful_values_are_written_and_read_back() {
        let parameters = json!({
            "reasoning": {"effort": "high", "budget": 2048},
            "stop": ["a", "b"],
            "flag": true,
            "ratio": 0.5,
            "nested": {"list": [{"x": 1}, {"x": 2.5}], "empty": {}},
            "names": []
        });
        let edit = Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                profiles: Some(vec![
                    profile(
                        "approfondi",
                        Some("Approfondi"),
                        parameters.clone(),
                        &["/temperature", "/reasoning/summary"],
                    ),
                    profile("rapide", None, json!({}), &[]),
                ]),
                reasoning_default: Some("approfondi".into()),
                ..Default::default()
            },
        };
        let next = apply(PROVIDERS, &edit, "test").unwrap();
        let got = reloaded_profiles(&next, "local", "small");
        assert_eq!(
            got,
            vec![
                (
                    "approfondi".into(),
                    Some("Approfondi".into()),
                    parameters,
                    vec!["/temperature".to_string(), "/reasoning/summary".to_string()],
                ),
                ("rapide".into(), None, json!({}), vec![]),
            ]
        );
        // The provider's own header and the other models are kept.
        assert!(next.starts_with("# Fournisseurs de test"));
        assert!(next.contains("max_input_tokens = 32000"), "the big model is kept");
    }

    #[test]
    fn a_kept_profile_keeps_its_comments_and_an_equal_number_keeps_its_text() {
        let text = PROVIDERS.replace(
            "id = \"fast\"\nlabel = \"Rapide\"",
            "id = \"fast\"\n# réglage de test\nlabel = \"Rapide\"\nparameters = { ratio = 1.0 }",
        );
        // The same numeric value sent as an integer does not rewrite the line.
        let edit = profiles_update(
            "local",
            "small",
            vec![
                profile("fast", Some("Rapide"), json!({"ratio": 1}), &[]),
                profile("deep", None, json!({"ratio": 2}), &[]),
            ],
        );
        let next = apply(&text, &edit, "test").unwrap();
        assert!(next.contains("# réglage de test"), "{next}");
        assert!(next.contains("parameters = { ratio = 1.0 }"), "{next}");
        assert!(next.contains("[[providers.models.reasoning.profiles]]\nid = \"deep\""), "{next}");
    }

    #[test]
    fn a_profile_list_can_be_emptied_when_no_default_names_a_profile() {
        let text = PROVIDERS.replace("default = \"fast\"\n", "");
        let edit = profiles_update("local", "small", vec![]);
        let next = apply(&text, &edit, "test").unwrap();
        assert!(!next.contains("profiles"), "{next}");
        assert!(ProvidersConfig::from_toml_str(&next, "test").is_ok());
    }

    #[test]
    fn dropping_the_default_profile_is_refused_unless_the_patch_says_what_replaces_it() {
        // `small` defaults to `fast`: a list without `fast` must say what the default becomes.
        let without_default = profiles_update(
            "local",
            "small",
            vec![profile("deep", None, json!({"x": 1}), &[])],
        );
        let err = apply(PROVIDERS, &without_default, "test").unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid);
        assert!(err.message.contains("« fast »"), "{}", err.message);

        let replaced = Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                profiles: Some(vec![profile("deep", None, json!({"x": 1}), &[])]),
                reasoning_default: Some("deep".into()),
                ..Default::default()
            },
        };
        let next = apply(PROVIDERS, &replaced, "test").unwrap();
        assert!(next.contains("default = \"deep\""), "{next}");
        assert!(!next.contains("id = \"fast\""), "{next}");

        let cleared = Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                profiles: Some(vec![]),
                clear_reasoning_default: true,
                ..Default::default()
            },
        };
        let next = apply(PROVIDERS, &cleared, "test").unwrap();
        assert!(!next.contains("default ="), "{next}");
    }

    #[test]
    fn a_default_outside_the_new_list_is_refused_in_french() {
        let edit = Edit::ModelUpdate {
            provider_id: "local".into(),
            model_id: "small".into(),
            patch: ModelPatch {
                profiles: Some(vec![profile("fast", None, json!({}), &[])]),
                reasoning_default: Some("ghost".into()),
                ..Default::default()
            },
        };
        let err = apply(PROVIDERS, &edit, "test").unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid);
        assert!(err.message.contains("« ghost »"), "{}", err.message);
    }

    #[test]
    fn invalid_profile_lists_are_refused_before_anything_is_written() {
        let cases: Vec<(Vec<ReasoningProfileEdit>, &str)> = vec![
            (
                vec![
                    profile("a", None, json!({}), &[]),
                    profile("a", None, json!({}), &[]),
                ],
                "en double",
            ),
            (vec![profile("  ", None, json!({}), &[])], "identifiant vide"),
            (
                vec![profile("a", None, json!(["not", "an", "object"]), &[])],
                "objet JSON",
            ),
            (
                vec![profile("a", None, json!({"x": null}), &[])],
                "null",
            ),
            (
                vec![profile("a", None, json!({"x": 1}), &["no-slash"])],
                "pointeur JSON",
            ),
            (
                vec![profile("a", None, json!({"x": 1}), &["/bad~2escape"])],
                "~0 ou ~1",
            ),
            (
                vec![profile("a", None, json!({"x": 1}), &[""])],
                "pointeur vide",
            ),
        ];
        for (profiles, expected) in cases {
            let err = apply(PROVIDERS, &profiles_update("local", "small", profiles), "test")
                .expect_err("refused");
            assert_eq!(err.code, IpcErrorCode::Invalid, "{}", err.message);
            assert!(err.message.contains(expected), "{expected}: {}", err.message);
        }
    }

    #[test]
    fn a_structural_field_in_a_profile_is_refused_by_the_provider_rules() {
        let err = apply(
            PROVIDERS,
            &profiles_update(
                "local",
                "small",
                vec![profile("fast", Some("Rapide"), json!({"messages": []}), &[])],
            ),
            "test",
        )
        .unwrap_err();
        assert_eq!(err.code, IpcErrorCode::Invalid);
        assert!(err.message.contains("messages"), "{}", err.message);
    }

    #[test]
    fn a_new_profile_in_an_inline_reasoning_table_stays_inline() {
        let text = PROVIDERS.replace(
            "[providers.models.limits]\nmax_input_tokens = 8000\nmax_output_tokens = 1000\n[providers.models.reasoning]\ndefault = \"fast\"\n[[providers.models.reasoning.profiles]]\nid = \"fast\"\nlabel = \"Rapide\"\n",
            "reasoning = { default = \"fast\", profiles = [{ id = \"fast\", label = \"Rapide\" }] }\n[providers.models.limits]\nmax_input_tokens = 8000\nmax_output_tokens = 1000\n",
        );
        assert_ne!(text, PROVIDERS, "the fixture changed");
        let edit = profiles_update(
            "local",
            "small",
            vec![
                profile("fast", Some("Rapide"), json!({}), &[]),
                profile("deep", None, json!({"reasoning": {"effort": "high"}}), &[]),
            ],
        );
        let next = apply(&text, &edit, "test").unwrap();
        assert!(next.contains("reasoning = {"), "{next}");
        assert_eq!(reloaded_profiles(&next, "local", "small").len(), 2);
    }

    #[test]
    fn a_model_edit_keeps_the_context_window_it_does_not_touch() {
        let text = PROVIDERS.replace(
            "max_input_tokens = 8000\nmax_output_tokens = 1000\n",
            "max_input_tokens = 8000\nmax_output_tokens = 1000\ncontext_window_tokens = 16000\n",
        );
        let edit = model_update(
            "local",
            "small",
            ModelPatch {
                max_input_tokens: Some(9000),
                ..Default::default()
            },
        );
        let next = apply(&text, &edit, "test").unwrap();
        assert_eq!(
            next,
            text.replace("max_input_tokens = 8000", "max_input_tokens = 9000")
        );
        assert!(next.contains("context_window_tokens = 16000"));
    }
}
