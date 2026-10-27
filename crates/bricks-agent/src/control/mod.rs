//! The engine's command/event contract and its controller, shared by every
//! frontend (headless JSONL, terminal interface, future editors or
//! servers). Frontends never touch the agent loop, providers, memory
//! internals or shells directly: they send [`Command`]s and read
//! [`Envelope`]s. See `docs/cli.md` for the schema.

pub mod approval;
pub mod ask;
pub mod attach;
pub mod controller;
pub mod protocol;
pub mod queue;
pub mod scripted;
pub mod session;
pub mod settings;
pub mod slash;
pub mod title;

pub use approval::{ApprovalBroker, ApprovalRequest, BrokerApprover, DecidedBy, Decision};
pub use ask::{
    AskBroker, AskTimer, AskTool, QuestionAnswer, QuestionOption, QuestionOutcome, QuestionRequest,
    SkipReason,
};
pub use controller::{
    Activity, Controller, EngineConfig, EventStream, MemoryResolver, ModelCatalog, ModelChoice,
    OpenOptions, ProfileChoice, ProjectContext, ProjectLoader, RunMemory, SessionChoice,
    SkillsSetup, Snapshot, ToolFactory, ToolInfo, list_sessions, list_sessions_in,
};
pub use protocol::{
    AttachmentInfo, Command, Envelope, Event, FailureKind, MAX_PATCH_BYTES,
    MAX_SUBAGENT_OUTPUT_BYTES, MaintenanceOutcome, Prompt, PromptBlock, RunOutcome, SCHEMA_VERSION,
    SearchHit, SkillListing, SkillSourceInfo, SkillsListing, WrittenFile, cap_text,
    tool_status_name,
};
pub use session::{SessionMeta, SessionScope, SessionSummary, same_workspace};
pub use settings::AgentSettings;
