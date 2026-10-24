//! The skills of the settings page: the catalogue of the store and the
//! built-in skills, with the state each candidate has there, and the changes
//! made to the store (create, import, remove). Built on `bricks_skills`, the
//! same store and service the sessions use; nothing is implemented twice.
//!
//! The catalogue reads no project and no home folder of another client, and
//! grants no trust. The service re-reads the store when it changed, so a
//! change made here shows at the next call; `reload` reads it again anyway.

use crate::contract::{
    IpcErrorCode, IpcResult, SkillChange, SkillProblem, SkillSourceView, SkillsCatalog,
};
use crate::services::error;
use bricks_agent::control::{SkillListing, SkillsListing};
use bricks_skills::{
    CandidateState, Field, InstallOrigin, Installed, Registry, Scope, SkillAccess, SkillStore,
    SkillsService, StoreError, Trust, bricks_roots,
};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Entries per page, as the engine's listing.
const PER_PAGE: usize = 20;

/// The catalogue of the store (`<bricks_home>/skills`) and of the built-in
/// skills. One service is kept; it re-reads the store when the store changed.
pub struct SkillsCatalogStore {
    store: SkillStore,
    service: Mutex<Option<Arc<SkillsService>>>,
}

impl SkillsCatalogStore {
    /// `dir`: the store's folder (`<bricks_home>/skills`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            store: SkillStore::new(dir),
            service: Mutex::new(None),
        }
    }

    /// The store the catalogue reads and the changes write to.
    pub fn store(&self) -> SkillStore {
        self.store.clone()
    }

    /// The catalogue, one page of it. `project_id` is echoed back: the
    /// catalogue no longer reads project folders. `reload` reads the store
    /// again; otherwise the last reading is kept until the store changes.
    pub fn catalog(
        &self,
        project_id: Option<String>,
        query: &str,
        page: u32,
        reload: bool,
    ) -> IpcResult<SkillsCatalog> {
        let service = {
            let mut slot = self.service.lock();
            let service = slot
                .get_or_insert_with(|| {
                    Arc::new(SkillsService::new(
                        bricks_roots(Some(self.store.dir())),
                        bricks_skills::builtin::BUILTIN,
                        None,
                    ))
                })
                .clone();
            if reload {
                service.reload();
            }
            service
        };
        let listing = listing(&service, query, page as usize);
        let listing = serde_json::to_value(listing)
            .map_err(|e| error(IpcErrorCode::Internal, format!("liste des skills : {e}")))?;
        Ok(SkillsCatalog {
            project_id,
            listing,
            sources: sources(&service.registry()),
        })
    }
}

/// The listing of one page of the candidates, as the engine's `SkillsListing`
/// (its states, its details and its bounds), without a session: nothing is
/// active.
fn listing(service: &Arc<SkillsService>, query: &str, page: usize) -> SkillsListing {
    let reg = service.registry();
    let trust = Trust::default();
    let q = query.trim().to_lowercase();
    let mut entries: Vec<SkillListing> = Vec::new();
    // The first allowed candidate of each name wins it (sources are in priority order).
    let mut winner: Option<(String, String)> = None; // (key, source)
    for c in &reg.candidates {
        let root = reg.root(c);
        let mut details = Vec::new();
        let state = if !trust.allows(root) {
            "disabled"
        } else if let Some((_, by)) = winner.as_ref().filter(|(k, _)| *k == c.key) {
            details.push(format!("hidden by the same name in {by}"));
            "shadowed"
        } else {
            winner = Some((c.key.clone(), c.source.clone()));
            match &c.state {
                CandidateState::Valid => "available",
                CandidateState::Invalid { error } => {
                    details.push(error.clone());
                    "invalid"
                }
                CandidateState::Incompatible { reasons } => {
                    details.push(format!("uses {}", reasons.join(", ")));
                    "incompatible"
                }
            }
        };
        details.extend(c.notes.iter().cloned());
        if !q.is_empty()
            && !c.key.to_lowercase().contains(&q)
            && !c
                .description
                .as_deref()
                .is_some_and(|d| d.to_lowercase().contains(&q))
        {
            continue;
        }
        entries.push(SkillListing {
            name: c.key.clone(),
            source: c.source.clone(),
            state: state.into(),
            description: c.description.clone(),
            path: c.dir.as_ref().map(|d| d.display().to_string()),
            revision: c.revision.clone(),
            details,
        });
    }
    let total = entries.len();
    let entries = entries
        .into_iter()
        .skip(page.saturating_mul(PER_PAGE))
        .take(PER_PAGE)
        .collect();
    SkillsListing {
        generation: reg.generation,
        project_decision: "pending".into(),
        entries,
        total,
        page,
        diagnostics: reg
            .diagnostics
            .iter()
            .map(diagnostic_text)
            .chain(
                reg.truncated
                    .then(|| "the scan stopped at a limit".to_string()),
            )
            .collect(),
        catalog_omitted: SkillAccess::new(Arc::clone(service)).catalog().omitted,
    }
}

fn diagnostic_text(d: &bricks_skills::Diagnostic) -> String {
    format!(
        "{}: {}{}",
        d.source,
        d.path
            .as_ref()
            .map(|p| format!("{}: ", p.display()))
            .unwrap_or_default(),
        d.message
    )
}

/// Each source in order, with its folder and how many candidates it holds.
fn sources(reg: &Registry) -> Vec<SkillSourceView> {
    reg.roots
        .iter()
        .enumerate()
        .map(|(index, root)| SkillSourceView {
            source: match root.scope {
                Scope::Store => "store",
                Scope::Project => "project",
                Scope::Personal => "user",
                Scope::Builtin => "builtin",
                Scope::Explicit => "explicit",
            }
            .into(),
            path: root
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            skills: u32::try_from(reg.candidates.iter().filter(|c| c.root == index).count())
                .unwrap_or(u32::MAX),
        })
        .collect()
}

/// A skill written from its name, description and instructions, installed
/// in the store.
pub fn create_skill(
    store: &SkillStore,
    name: &str,
    description: &str,
    instructions: &str,
) -> IpcResult<SkillChange> {
    match store.create(name, description, instructions, InstallOrigin::Created) {
        Ok(installed) => Ok(installed_change(installed)),
        Err(e) => refusal(e),
    }
}

/// A copy of the skill folder `path` (chosen by the person), installed in the
/// store.
pub fn import_skill(store: &SkillStore, path: &Path) -> IpcResult<SkillChange> {
    match store.install(path, InstallOrigin::Imported) {
        Ok(installed) => Ok(installed_change(installed)),
        Err(e) => refusal(e),
    }
}

/// Removes the installed skill `name`. Only the store is searched: a name
/// is never a path.
pub fn remove_skill(store: &SkillStore, name: &str) -> IpcResult<SkillChange> {
    match store.remove(name) {
        Ok(name) => Ok(SkillChange {
            name: Some(name),
            problem: None,
            notes: Vec::new(),
        }),
        Err(e) => refusal(e),
    }
}

fn installed_change(installed: Installed) -> SkillChange {
    SkillChange {
        name: Some(installed.name),
        problem: None,
        notes: installed.notes,
    }
}

/// A refused change is an answer, not a failure: the form shows what to
/// correct. Only a failure of the file system is an error.
fn refusal(e: StoreError) -> IpcResult<SkillChange> {
    let (field, kind) = match &e {
        StoreError::Io { message } => {
            return Err(error(IpcErrorCode::Io, message.clone()));
        }
        StoreError::Invalid { field, .. } => (field_name(*field), "invalid"),
        StoreError::Incompatible { .. } => ("document", "incompatible"),
        StoreError::Reserved { .. } => ("name", "reserved"),
        StoreError::Exists { .. } => ("name", "exists"),
        StoreError::NotInstalled { .. } => ("name", "not_installed"),
        StoreError::Unsafe { .. } => ("folder", "unsafe"),
    };
    Ok(SkillChange {
        name: None,
        problem: Some(SkillProblem {
            field: field.into(),
            kind: kind.into(),
            message: e.to_string(),
        }),
        notes: Vec::new(),
    })
}

fn field_name(field: Field) -> &'static str {
    match field {
        Field::Name => "name",
        Field::Description => "description",
        Field::Instructions => "instructions",
        Field::Document => "document",
        Field::Folder => "folder",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `SKILL.md` with a name and a description, as the parser reads it.
    fn write_skill(store: &Path, dir: &str, description: &str) {
        let d = store.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {dir}\ndescription: {description}\n---\nbody of {dir}\n"),
        )
        .unwrap();
    }

    /// A temporary folder holding the store (`skills`) and an outside folder.
    struct Folders {
        _t: tempfile::TempDir,
        store: PathBuf,
        outside: PathBuf,
    }

    fn folders() -> Folders {
        let t = tempfile::tempdir().unwrap();
        let store = t.path().join("skills");
        let outside = t.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        Folders {
            _t: t,
            store,
            outside,
        }
    }

    fn entry<'a>(listing: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
        listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("no entry `{name}` in {listing}"))
    }

    #[test]
    fn the_store_and_the_built_in_skills_are_listed() {
        let f = folders();
        write_skill(&f.store, "alpha", "Alpha skill");
        let store = SkillsCatalogStore::new(f.store.clone());
        let cat = store.catalog(None, "", 0, false).unwrap();
        assert_eq!(cat.project_id, None);
        let alpha = entry(&cat.listing, "alpha");
        assert_eq!(alpha["state"], "available");
        assert!(alpha["source"].as_str().unwrap().starts_with("store"));
        assert_eq!(alpha["description"], "Alpha skill");
        assert_eq!(entry(&cat.listing, "create-skills")["source"], "builtin");

        let kinds: Vec<&str> = cat.sources.iter().map(|s| s.source.as_str()).collect();
        assert_eq!(kinds, ["store", "builtin"], "no project or home source: {kinds:?}");
        let installed = &cat.sources[0];
        assert_eq!(installed.path, f.store.display().to_string());
        assert_eq!(installed.skills, 1);
        let builtin = cat.sources.iter().find(|s| s.source == "builtin").unwrap();
        assert_eq!(builtin.skills as usize, bricks_skills::builtin::BUILTIN.len());
    }

    #[test]
    fn a_store_folder_named_like_a_built_in_skill_shadows_it() {
        let f = folders();
        write_skill(&f.store, "create-skills", "Installed copy");
        let store = SkillsCatalogStore::new(f.store.clone());
        let cat = store.catalog(None, "create-skills", 0, false).unwrap();
        let shadowed = cat.listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["state"] == "shadowed")
            .expect("the built-in one is shadowed");
        assert_eq!(shadowed["source"], "builtin");
        assert!(
            shadowed["details"][0].as_str().unwrap().contains("store"),
            "{shadowed}"
        );
    }

    #[test]
    fn a_broken_skill_is_listed_as_invalid_with_its_error() {
        let f = folders();
        let dir = f.store.join("broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: [\n---\nbody\n").unwrap();
        let store = SkillsCatalogStore::new(f.store.clone());
        let cat = store.catalog(None, "broken", 0, false).unwrap();
        let broken = entry(&cat.listing, "broken");
        assert_eq!(broken["state"], "invalid");
        assert!(!broken["details"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_query_filters_and_the_pages_are_of_twenty() {
        let f = folders();
        for i in 0..25 {
            write_skill(&f.store, &format!("skill-{i:02}"), "Bulk");
        }
        let store = SkillsCatalogStore::new(f.store.clone());
        let first = store.catalog(None, "SKILL-", 0, false).unwrap();
        assert_eq!(first.listing["total"], 25);
        assert_eq!(first.listing["entries"].as_array().unwrap().len(), 20);
        let second = store.catalog(None, "skill-", 1, false).unwrap();
        assert_eq!(second.listing["page"], 1);
        assert_eq!(second.listing["entries"].as_array().unwrap().len(), 5);
        let none = store.catalog(None, "no such skill", 0, false).unwrap();
        assert_eq!(none.listing["total"], 0);
    }

    #[test]
    fn an_installed_skill_shows_at_the_next_call_and_reload_reads_again() {
        let f = folders();
        let store = SkillsCatalogStore::new(f.store.clone());
        let before = store.catalog(None, "late", 0, false).unwrap();
        assert_eq!(before.listing["total"], 0);
        let generation = before.listing["generation"].as_u64().unwrap();

        SkillStore::new(&f.store)
            .create("late", "Added later", "Do the late thing.", InstallOrigin::Created)
            .unwrap();
        let seen = store.catalog(None, "late", 0, false).unwrap();
        assert_eq!(seen.listing["total"], 1);
        assert_eq!(entry(&seen.listing, "late")["state"], "available");
        assert!(seen.listing["generation"].as_u64().unwrap() > generation);

        let reloaded = store.catalog(None, "late", 0, true).unwrap();
        assert_eq!(reloaded.listing["total"], 1);
        assert!(reloaded.listing["generation"].as_u64().unwrap() > seen.listing["generation"].as_u64().unwrap());

        SkillStore::new(&f.store).remove("late").unwrap();
        let gone = store.catalog(None, "late", 0, false).unwrap();
        assert_eq!(gone.listing["total"], 0, "a removed skill leaves the catalogue");
    }

    #[test]
    fn a_created_skill_is_installed_and_listed() {
        let f = folders();
        let store = SkillStore::new(&f.store);
        let change = create_skill(&store, "gamma", "Gamma skill", "Gamma instructions.").unwrap();
        assert_eq!(change.name.as_deref(), Some("gamma"));
        assert!(change.problem.is_none());

        let cat = SkillsCatalogStore::new(f.store.clone())
            .catalog(None, "gamma", 0, false)
            .unwrap();
        assert_eq!(entry(&cat.listing, "gamma")["state"], "available");
    }

    #[test]
    fn a_refused_creation_names_the_field_it_is_about() {
        let f = folders();
        let store = SkillStore::new(&f.store);
        let problem = |c: SkillChange| c.problem.expect("refused");

        let reserved = problem(create_skill(&store, "create-skills", "Clash", "Text.").unwrap());
        assert_eq!((reserved.field.as_str(), reserved.kind.as_str()), ("name", "reserved"));

        let bad_name = problem(create_skill(&store, "Bad Name!", "Fine", "Text.").unwrap());
        assert_eq!((bad_name.field.as_str(), bad_name.kind.as_str()), ("name", "invalid"));

        let empty = problem(create_skill(&store, "delta", "   ", "Text.").unwrap());
        assert_eq!((empty.field.as_str(), empty.kind.as_str()), ("description", "invalid"));

        let no_text = problem(create_skill(&store, "delta", "Fine", "  ").unwrap());
        assert_eq!((no_text.field.as_str(), no_text.kind.as_str()), ("instructions", "invalid"));

        create_skill(&store, "epsilon", "First", "Text.").unwrap();
        let taken = problem(create_skill(&store, "epsilon", "Second", "Text.").unwrap());
        assert_eq!((taken.field.as_str(), taken.kind.as_str()), ("name", "exists"));
        assert!(!taken.message.is_empty());
    }

    #[test]
    fn an_import_copies_the_folder_or_says_what_is_wrong_with_it() {
        let f = folders();
        let store = SkillStore::new(&f.store);
        let src = f.outside.join("zeta");
        std::fs::create_dir_all(src.join("scripts")).unwrap();
        std::fs::write(src.join("SKILL.md"), "---\nname: zeta\ndescription: Zeta skill\n---\nBody.\n").unwrap();
        std::fs::write(src.join("scripts/run.sh"), "echo hi\n").unwrap();

        let change = import_skill(&store, &src).unwrap();
        assert_eq!(change.name.as_deref(), Some("zeta"));
        assert!(f.store.join("zeta/scripts/run.sh").is_file());

        let missing = import_skill(&store, &f.outside.join("nothing-here")).unwrap();
        let problem = missing.problem.expect("refused");
        assert_eq!((problem.field.as_str(), problem.kind.as_str()), ("folder", "invalid"));

        let no_skill = f.outside.join("no-skill");
        std::fs::create_dir_all(&no_skill).unwrap();
        let problem = import_skill(&store, &no_skill).unwrap().problem.expect("refused");
        assert_eq!((problem.field.as_str(), problem.kind.as_str()), ("folder", "invalid"));
    }

    #[test]
    fn a_removal_is_refused_for_a_built_in_or_an_unknown_name() {
        let f = folders();
        let store = SkillStore::new(&f.store);
        let problem = |c: SkillChange| c.problem.expect("refused");

        let builtin = problem(remove_skill(&store, "create-skills").unwrap());
        assert_eq!((builtin.field.as_str(), builtin.kind.as_str()), ("name", "reserved"));

        let unknown = problem(remove_skill(&store, "nobody").unwrap());
        assert_eq!((unknown.field.as_str(), unknown.kind.as_str()), ("name", "not_installed"));

        create_skill(&store, "eta", "Eta skill", "Text.").unwrap();
        let removed = remove_skill(&store, "eta").unwrap();
        assert_eq!(removed.name.as_deref(), Some("eta"));
        assert!(removed.problem.is_none());
        assert!(!f.store.join("eta").exists());
    }
}
