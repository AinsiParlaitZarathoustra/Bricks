//! bricks-skills — [Agent Skills](https://agentskills.io/specification) for
//! Bricks.
//!
//! A skill is a directory holding a `SKILL.md` (YAML frontmatter, then
//! Markdown instructions) and, optionally, other files. This crate reads and
//! validates the documents ([`document`]), discovers them in their sources
//! with priorities and collisions ([`registry`]), filters what may be used
//! by trust, builds the catalog given to the model and activates a skill
//! ([`service`]), and resolves a skill's files without leaving its directory
//! ([`resources`]).
//!
//! Loading a skill gives instructions and resources; it runs no command,
//! starts no agent and grants no permission. Actions go through the tools
//! and permissions of the agent. This crate depends on no runner, provider
//! or interface.
//!
//! ```no_run
//! use bricks_skills::{SkillAccess, SkillsService};
//! use std::sync::Arc;
//!
//! let service = SkillsService::standard("/path/to/project".as_ref(), dirs_home().as_deref());
//! let access = SkillAccess::new(Arc::new(service));
//! // Project skills stay unusable until someone approves them:
//! access.decide_project(bricks_skills::ProjectDecision::Granted);
//! if let Some(text) = access.catalog().render() {
//!     println!("{text}");
//! }
//! # fn dirs_home() -> Option<std::path::PathBuf> { None }
//! ```

pub mod builtin;
pub mod document;
pub mod ledger;
pub mod registry;
pub mod resources;
pub mod security;
pub mod service;

pub use document::{parse_document, AllowedTools, DocumentError, SkillDocument};
pub use ledger::{ActiveSkill, Ledger, Origin, StoredActivation, StoredLedger};
pub use registry::{
    standard_roots, BuiltinSkill, Candidate, CandidateState, Diagnostic, ProjectDecision, Registry,
    ResolveError, Scope, SkillRoot, Trust,
};
pub use resources::Resource;
pub use service::{
    render_activation, Activation, ActivationError, Catalog, CatalogEntry, SkillAccess,
    SkillsService,
};
