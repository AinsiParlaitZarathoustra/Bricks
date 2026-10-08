//! The engine's command/event contract and its controller, shared by every
//! frontend (headless JSONL, terminal interface, future editors or
//! servers). Frontends never touch the agent loop, providers, memory
//! internals or shells directly: they send [`Command`]s and read
//! [`Envelope`]s. See `docs/cli.md` for the schema.

pub mod approval;
pub mod attach;
pub mod controller;
pub mod protocol;
pub mod queue;
pub mod scripted;
pub mod session;
pub mod settings;

pub use approval::{ApprovalBroker, ApprovalGate, ApprovalRequest, DecidedBy, Decision};
pub use controller::{
    list_sessions, list_sessions_in, Activity, Controller, EngineConfig, EventStream, ModelCatalog,
    ModelChoice, OpenOptions, ProfileChoice, ProjectContext, ProjectLoader, SessionChoice,
    Snapshot, ToolFactory, ToolInfo,
};
pub use protocol::{
    AttachmentInfo, Command, Envelope, Event, FailureKind, MaintenanceOutcome, Prompt, PromptBlock,
    RunOutcome, SearchHit, WrittenFile, SCHEMA_VERSION,
};
pub use session::{same_workspace, SessionMeta, SessionScope, SessionSummary};
pub use settings::{Action, AgentSettings, ApprovalRules};
