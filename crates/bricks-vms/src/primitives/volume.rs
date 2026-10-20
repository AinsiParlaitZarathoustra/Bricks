//! Volume registry — persistent host-side dirs bind-mounted into N sandboxes.

use crate::error::{Result, VmError};
use crate::types::VolumeId;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A volume is a labelled host-side directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    pub id: VolumeId,
    pub label: Option<String>,
    pub host_path: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Owns a root directory and tracks named volumes underneath it.
#[derive(Clone)]
pub struct VolumeRegistry {
    root: PathBuf,
    volumes: Arc<DashMap<VolumeId, Volume>>,
}

impl VolumeRegistry {
    /// Open (and create if missing) a registry rooted at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let registry = Self {
            root,
            volumes: Arc::new(DashMap::new()),
        };
        registry.load_existing()?;
        Ok(registry)
    }

    /// Default registry at `~/.cersei/vms/volumes`.
    pub fn default_user() -> Result<Self> {
        let base = dirs::home_dir()
            .ok_or_else(|| VmError::Invalid("no home directory".into()))?
            .join(".cersei")
            .join("vms")
            .join("volumes");
        Self::open(base)
    }

    fn load_existing(&self) -> Result<()> {
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id_str = entry.file_name().to_string_lossy().into_owned();
            let host_path = entry.path();
            let meta_path = host_path.join(".cersei-volume.json");
            let (label, created_at) = match std::fs::read_to_string(&meta_path) {
                Ok(s) => match serde_json::from_str::<VolumeMeta>(&s) {
                    Ok(m) => (m.label, m.created_at),
                    Err(_) => (None, chrono::Utc::now()),
                },
                Err(_) => (None, chrono::Utc::now()),
            };
            let id = VolumeId(id_str.clone());
            self.volumes.insert(
                id.clone(),
                Volume {
                    id,
                    label,
                    host_path,
                    created_at,
                },
            );
        }
        Ok(())
    }

    /// Create a new volume.
    pub fn create(&self, label: Option<String>) -> Result<Volume> {
        let id = VolumeId::new();
        let host_path = self.root.join(id.as_str());
        std::fs::create_dir_all(&host_path)?;
        let vol = Volume {
            id: id.clone(),
            label: label.clone(),
            host_path: host_path.clone(),
            created_at: chrono::Utc::now(),
        };
        let meta = VolumeMeta {
            label,
            created_at: vol.created_at,
        };
        std::fs::write(
            host_path.join(".cersei-volume.json"),
            serde_json::to_vec_pretty(&meta)?,
        )?;
        self.volumes.insert(id, vol.clone());
        Ok(vol)
    }

    pub fn get(&self, id: &VolumeId) -> Result<Volume> {
        self.volumes
            .get(id)
            .map(|v| v.clone())
            .ok_or_else(|| VmError::VolumeNotFound(id.to_string()))
    }

    pub fn list(&self) -> Vec<Volume> {
        self.volumes.iter().map(|kv| kv.value().clone()).collect()
    }

    /// Remove a volume. `force` blows the directory away even if non-empty.
    pub fn remove(&self, id: &VolumeId, force: bool) -> Result<()> {
        let Some((_, vol)) = self.volumes.remove(id) else {
            return Err(VmError::VolumeNotFound(id.to_string()));
        };
        if force {
            std::fs::remove_dir_all(&vol.host_path)?;
        } else {
            std::fs::remove_dir(&vol.host_path)?;
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct VolumeMeta {
    label: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}
