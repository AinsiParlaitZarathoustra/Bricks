//! The providers file (`providers.toml`): the view the desktop shows of it,
//! and the checked, atomic writes that change it. Nothing here returns a
//! secret value: views carry names and flags, and errors carry messages
//! that name a field, never the value of a key or header.

use super::providers_toml::{self, Edit};
use super::{error, io_error};
use crate::contract::{
    IpcErrorCode, IpcResult, ModelProfileView, ModelView, PricingView, ProviderView, ProvidersFile,
};
use crate::i18n::{msg, tr};
use bricks_provider::config::{AuthMode, ProviderConfig, ProvidersConfig};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;

/// Content hash of a file's bytes: the revision every write must present.
/// An absent file has the revision of empty content.
pub fn revision_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The file's bytes, or `None` when it does not exist.
fn read_file(path: &Path) -> IpcResult<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(&format!("fournisseurs ({})", path.display()), e)),
    }
}

/// The providers file as the desktop shows it. An absent file gives an
/// empty list with no error; an invalid one gives the validator's message
/// and an empty list.
pub fn view(path: &Path) -> ProvidersFile {
    let path_text = path.display().to_string();
    let bytes = match read_file(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return ProvidersFile {
                path: path_text,
                revision: revision_of(&[]),
                providers: Vec::new(),
                error: Some(e.message),
            };
        }
    };
    let revision = revision_of(bytes.as_deref().unwrap_or(&[]));
    let Some(bytes) = bytes else {
        return ProvidersFile {
            path: path_text,
            revision,
            providers: Vec::new(),
            error: None,
        };
    };
    let parsed = std::str::from_utf8(&bytes)
        .map_err(|_| tr!(msg::THE_PROVIDERS_TOML_FILE_ISNT_UTF).to_string())
        .and_then(|text| {
            ProvidersConfig::from_toml_str(text, &path_text).map_err(|e| e.to_string())
        });
    match parsed {
        Ok(config) => ProvidersFile {
            providers: config
                .providers
                .iter()
                .map(provider_view)
                .collect(),
            path: path_text,
            revision,
            error: None,
        },
        Err(message) => ProvidersFile {
            path: path_text,
            revision,
            providers: Vec::new(),
            error: Some(message),
        },
    }
}

fn provider_view(p: &ProviderConfig) -> ProviderView {
    ProviderView {
        id: p.id.clone(),
        name: p.name.clone(),
        endpoint: p.endpoint.clone(),
        protocol: p.protocol.as_str().to_string(),
        auth: p.auth.as_str().to_string(),
        auth_header: p.auth_header.clone(),
        api_key: p.key().map(str::to_string),
        has_credentials: credentials_available(p),
        path: p.path.clone(),
        header_names: p.headers.keys().cloned().collect(),
        models: p
            .models
            .iter()
            .map(|m| ModelView {
                id: m.id.clone(),
                selection: format!("{}/{}", p.id, m.id),
                name: m.name.clone(),
                api_model: m.api_model.clone(),
                protocol: m.protocol.map(|x| x.as_str().to_string()),
                endpoint: m.endpoint.clone(),
                input_modalities: m.input_modalities.iter().map(|x| x.to_string()).collect(),
                output_modalities: m.output_modalities.iter().map(|x| x.to_string()).collect(),
                streaming: m.streaming,
                async_tool_calls: m.async_tool_calls,
                max_input_tokens: m.limits.max_input_tokens,
                max_output_tokens: m.limits.max_output_tokens,
                profiles: m
                    .reasoning
                    .profiles
                    .iter()
                    .map(|x| ModelProfileView {
                        id: x.id.clone(),
                        label: x.label.clone(),
                        parameters: serde_json::Value::Object(x.parameters.clone()),
                        remove: x.remove.clone(),
                    })
                    .collect(),
                priced: m.pricing.is_some(),
                document_mime_types: m.document_mime_types.clone(),
                pricing: m.pricing.as_ref().map(|x| PricingView {
                    input: x.input.map(|d| d.to_string()),
                    output: x.output.map(|d| d.to_string()),
                    cache_read: x.cache_read.map(|d| d.to_string()),
                }),
            })
            .collect(),
    }
}

/// Applies `edit` to the file, if it still has `revision`. The result is
/// validated before anything is written, and written atomically with the
/// file's permissions kept. Errors leave the file as it was. An absent file
/// (revision of empty content, as [`view`] gives it) is created, with its
/// folder, from an empty providers file.
pub fn commit(path: &Path, revision: &str, edit: &Edit) -> IpcResult<()> {
    let origin = path.display().to_string();
    let existing = read_file(path)?;
    if revision_of(existing.as_deref().unwrap_or(&[])) != revision {
        return Err(error(
            IpcErrorCode::Conflict,
            tr!(msg::PROVIDERS_FILE_CHANGED),
        ));
    }
    let next = match &existing {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                error(
                    IpcErrorCode::Config,
                    tr!(msg::THE_PROVIDERS_TOML_FILE_ISNT_UTF),
                )
            })?;
            providers_toml::apply(text, edit, &origin)?
        }
        None => {
            let empty = format!(
                "schema_version = {}\n",
                bricks_provider::config::SCHEMA_VERSION
            );
            let next = providers_toml::apply(&empty, edit, &origin)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| io_error(&format!("fournisseurs ({origin})"), e))?;
            }
            next
        }
    };
    write_replacing(path, next.as_bytes())
        .map_err(|e| io_error(&format!("fournisseurs ({origin})"), e))
}

/// Replaces `path` with `bytes`: a temporary file in the same folder, with
/// the same permissions less any access by others, synced, then renamed over the target. A reader sees
/// the old or the new content, never a partial one.
fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent folder"))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("providers.toml");
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let permissions = std::fs::metadata(path).map(|m| m.permissions()).ok();
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Some(p) = permissions {
            std::fs::set_permissions(&tmp, p)?;
        }
        // The file holds the API keys: only its owner reads it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&tmp)?.permissions().mode();
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode & 0o700))?;
        }
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Whether a provider can be called: no credential is needed (`auth =
/// "none"`), or its key is written in the file.
pub(crate) fn credentials_available(p: &ProviderConfig) -> bool {
    matches!(p.auth, AuthMode::None) || p.key().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_are_content_hashes() {
        assert_eq!(revision_of(b"a"), revision_of(b"a"));
        assert_ne!(revision_of(b"a"), revision_of(b"b"));
        assert_eq!(revision_of(b"").len(), 64);
    }

    #[test]
    fn a_replacement_keeps_the_permissions_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("providers.toml");
        std::fs::write(&file, "one").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        write_replacing(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the mode of the file is kept");
        }
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["providers.toml".to_string()]);
    }
}
