//! The skills catalogue of the settings page: the sources a
//! session would read, with the state each candidate has there. Built on
//! `bricks_skills::SkillsService`, as the engine builds it for a session.
//!
//! Nothing here grants trust. Project sources are listed as
//! `pending_approval`: the decision belongs to a session, made by a person
//! in that session. Personal and built-in sources follow the engine's rules
//! (`bricks_skills::Trust::default()`). Each project's service is kept
//! between calls; `reload` reads its sources again.

use crate::contract::{IpcErrorCode, IpcResult, SkillSourceView, SkillsCatalog};
use crate::services::error;
use bricks_agent::control::{SkillListing, SkillsListing};
use bricks_skills::{
    CandidateState, Registry, Scope, SkillAccess, SkillRoot, SkillsService, Trust, standard_roots,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Entries per page, as the engine's listing.
const PER_PAGE: usize = 20;

/// The sources of a catalogue: the project's (when given), the personal ones
/// (when there is a home folder) and the built-in source.
pub fn catalog_roots(project: Option<&Path>, home: Option<&Path>) -> Vec<SkillRoot> {
    match project {
        Some(project) => standard_roots(project, home),
        // Without a project, the same list minus the project folders.
        None => standard_roots(Path::new(""), home)
            .into_iter()
            .filter(|r| r.scope != Scope::Project)
            .collect(),
    }
}

/// One discovery per project folder (and one for the user and built-in
/// sources), kept between calls.
pub struct SkillsCatalogStore {
    home: Option<PathBuf>,
    services: Mutex<HashMap<Option<PathBuf>, Arc<SkillsService>>>,
}

impl SkillsCatalogStore {
    /// `home`: the folder whose `.agents/skills` and `.claude/skills` are read.
    pub fn new(home: Option<PathBuf>) -> Self {
        Self {
            home,
            services: Mutex::new(HashMap::new()),
        }
    }

    /// The catalogue of `project` (or of the user and built-in sources).
    /// `reload` reads the sources again; otherwise the last reading is kept.
    pub fn catalog(
        &self,
        project_id: Option<String>,
        project: Option<&Path>,
        query: &str,
        page: u32,
        reload: bool,
    ) -> IpcResult<SkillsCatalog> {
        let key = project.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
        let service = {
            let mut services = self.services.lock();
            match services.get(&key).cloned() {
                Some(service) if reload => {
                    service.reload();
                    service
                }
                Some(service) => service,
                None => {
                    let roots = catalog_roots(key.as_deref(), self.home.as_deref());
                    let service = Arc::new(SkillsService::new(
                        roots,
                        bricks_skills::builtin::BUILTIN,
                        key.clone(),
                    ));
                    services.insert(key.clone(), Arc::clone(&service));
                    service
                }
            }
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
/// active and no project is approved.
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
            match root.scope {
                Scope::Project => "pending_approval",
                _ => "disabled",
            }
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
        diagnostics: reg.diagnostics.iter().map(diagnostic_text).chain(
            reg.truncated
                .then(|| "the scan stopped at a limit".to_string()),
        ).collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `SKILL.md` with a name and a description, as the parser reads it.
    fn write_skill(root: &Path, dir: &str, description: &str) {
        let d = root.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {dir}\ndescription: {description}\n---\nbody of {dir}\n"),
        )
        .unwrap();
    }

    struct Folders {
        _t: tempfile::TempDir,
        home: PathBuf,
        project: PathBuf,
    }

    fn folders() -> Folders {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let project = t.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        Folders {
            _t: t,
            home,
            project,
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
    fn without_a_project_the_personal_and_built_in_sources_are_listed() {
        let f = folders();
        write_skill(&f.home.join(".agents/skills"), "alpha", "Alpha skill");
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let cat = store.catalog(None, None, "", 0, false).unwrap();
        assert_eq!(cat.project_id, None);
        let alpha = entry(&cat.listing, "alpha");
        assert_eq!(alpha["state"], "available");
        assert_eq!(alpha["source"], "user:.agents");
        assert_eq!(alpha["description"], "Alpha skill");
        assert_eq!(entry(&cat.listing, "commit")["source"], "builtin");
        assert_eq!(cat.listing["project_decision"], "pending");

        let kinds: Vec<&str> = cat.sources.iter().map(|s| s.source.as_str()).collect();
        assert!(kinds.iter().all(|k| *k != "project"), "no project source: {kinds:?}");
        let user = cat.sources.iter().find(|s| s.path.ends_with(".agents/skills")).unwrap();
        assert_eq!(user.skills, 1);
        let builtin = cat.sources.iter().find(|s| s.source == "builtin").unwrap();
        assert_eq!(builtin.skills as usize, bricks_skills::builtin::BUILTIN.len());
    }

    #[test]
    fn a_project_skill_is_listed_but_never_trusted() {
        let f = folders();
        write_skill(&f.project.join(".agents/skills"), "beta", "Beta skill");
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let project = f.project.clone();
        let cat = store
            .catalog(Some("p_1".into()), Some(&project), "", 0, false)
            .unwrap();
        assert_eq!(cat.project_id.as_deref(), Some("p_1"));
        let beta = entry(&cat.listing, "beta");
        assert_eq!(beta["state"], "pending_approval", "{beta}");
        assert_eq!(beta["source"], "project:.agents");
        assert!(cat.sources.iter().any(|s| s.source == "project" && s.skills == 1));
    }

    #[test]
    fn a_personal_name_wins_over_an_unapproved_project_one() {
        let f = folders();
        write_skill(&f.project.join(".agents/skills"), "shared", "From the project");
        write_skill(&f.home.join(".agents/skills"), "shared", "From the user");
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let project = f.project.clone();
        let cat = store
            .catalog(Some("p".into()), Some(&project), "", 0, false)
            .unwrap();
        let shared: Vec<&serde_json::Value> = cat.listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "shared")
            .collect();
        assert_eq!(shared.len(), 2);
        let by_source = |source: &str| shared.iter().find(|e| e["source"] == source).unwrap();
        assert_eq!(by_source("project:.agents")["state"], "pending_approval");
        assert_eq!(by_source("user:.agents")["state"], "available");
    }

    #[test]
    fn a_second_source_with_the_same_name_is_shadowed_and_says_by_whom() {
        let f = folders();
        write_skill(&f.home.join(".agents/skills"), "twin", "First");
        write_skill(&f.home.join(".claude/skills"), "twin", "Second");
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let cat = store.catalog(None, None, "twin", 0, false).unwrap();
        assert_eq!(entry(&cat.listing, "twin")["state"], "available");
        let shadowed = cat.listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["state"] == "shadowed")
            .expect("the second one is shadowed");
        assert_eq!(shadowed["source"], "user:.claude");
        assert!(
            shadowed["details"][0]
                .as_str()
                .unwrap()
                .contains("user:.agents"),
            "{shadowed}"
        );
    }

    #[test]
    fn a_broken_skill_is_listed_as_invalid_with_its_error() {
        let f = folders();
        let dir = f.home.join(".agents/skills/broken");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: [\n---\nbody\n").unwrap();
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let cat = store.catalog(None, None, "broken", 0, false).unwrap();
        let broken = entry(&cat.listing, "broken");
        assert_eq!(broken["state"], "invalid");
        assert!(!broken["details"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_query_filters_and_the_pages_are_of_twenty() {
        let f = folders();
        for i in 0..25 {
            write_skill(&f.home.join(".agents/skills"), &format!("skill-{i:02}"), "Bulk");
        }
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let first = store.catalog(None, None, "SKILL-", 0, false).unwrap();
        assert_eq!(first.listing["total"], 25);
        assert_eq!(first.listing["entries"].as_array().unwrap().len(), 20);
        let second = store.catalog(None, None, "skill-", 1, false).unwrap();
        assert_eq!(second.listing["page"], 1);
        assert_eq!(second.listing["entries"].as_array().unwrap().len(), 5);
        let none = store.catalog(None, None, "no such skill", 0, false).unwrap();
        assert_eq!(none.listing["total"], 0);
    }

    #[test]
    fn a_plain_call_keeps_the_last_reading_and_reload_reads_again() {
        let f = folders();
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let before = store.catalog(None, None, "late", 0, false).unwrap();
        assert_eq!(before.listing["total"], 0);
        let generation = before.listing["generation"].as_u64().unwrap();

        write_skill(&f.home.join(".agents/skills"), "late", "Added later");
        let kept = store.catalog(None, None, "late", 0, false).unwrap();
        assert_eq!(kept.listing["total"], 0, "not read again yet");

        let reloaded = store.catalog(None, None, "late", 0, true).unwrap();
        assert_eq!(reloaded.listing["total"], 1);
        assert_eq!(entry(&reloaded.listing, "late")["state"], "available");
        assert!(reloaded.listing["generation"].as_u64().unwrap() > generation);
    }

    #[test]
    fn the_project_is_canonical_and_one_service_serves_it() {
        let f = folders();
        write_skill(&f.project.join(".claude/skills"), "gamma", "Gamma");
        let store = SkillsCatalogStore::new(Some(f.home.clone()));
        let via_dot = f.project.join(".");
        let a = store.catalog(Some("p".into()), Some(&via_dot), "", 0, false).unwrap();
        let b = store
            .catalog(Some("p".into()), Some(&f.project), "", 0, false)
            .unwrap();
        assert_eq!(a.listing["generation"], b.listing["generation"], "the same discovery");
        assert_eq!(entry(&b.listing, "gamma")["source"], "project:.claude");
    }
}
