//! bricks-skills — [Agent Skills](https://agentskills.io/specification) for
//! Bricks.
//!
//! A skill is a directory holding a `SKILL.md` (YAML frontmatter, then
//! Markdown instructions) and, optionally, other files. This crate reads and
//! validates the documents ([`document`]), discovers them in their sources
//! with priorities and collisions ([`registry`]), installs and removes the
//! copies kept in Bricks' store ([`store`]), filters what may be used
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
//! // The skills installed in the store, and the built-in one.
//! let service = SkillsService::standard("/path/to/project".as_ref(), Some("/bricks/skills".as_ref()));
//! let access = SkillAccess::new(Arc::new(service));
//! if let Some(text) = access.catalog().render() {
//!     println!("{text}");
//! }
//! ```

pub mod builtin;
pub mod document;
pub mod ledger;
pub mod registry;
pub mod resources;
pub mod security;
pub mod service;
pub mod store;

pub use document::{AllowedTools, DocumentError, SkillDocument, parse_document};
pub use ledger::{ActiveSkill, Ledger, Origin, StoredActivation, StoredLedger};
pub use registry::{
    BuiltinSkill, Candidate, CandidateState, Diagnostic, ProjectDecision, Registry, ResolveError,
    Scope, SkillRoot, Trust, bricks_roots, standard_roots,
};
pub use resources::Resource;
pub use service::{
    Activation, ActivationError, Catalog, CatalogEntry, SkillAccess, SkillsService,
    render_activation,
};
pub use store::{Checked, Field, InstallOrigin, Installed, SkillStore, StoreError};
