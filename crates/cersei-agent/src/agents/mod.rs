//! Native sub-agents: profiles, their registry, the shared spawner and the
//! `Agent` tool.

pub mod admission;
pub mod profile;
pub mod registry;
pub mod runtime;
pub mod spawn;
pub mod tool;
pub mod workspace;

pub use profile::{AgentProfile, Isolation, ModelPref, ProfileScope, ProfileSource, ReasoningPref};
pub use registry::{
    ProfileCatalog, ProfileDiagnostic, ProfileListing, ProfileRegistry, ProfileSources,
};
pub use runtime::{AgentRuntime, InstanceRecord, RunUsage, RuntimeHandle};
pub use spawn::{
    render_result, AgentIdentity, AgentResult, AgentSpawnRequest, AgentSpawner, BatchOptions,
    ChildTools, Choice, CommandRun, DelegationSettings, InstanceState, ParentModel, SkillLoad,
    SpawnError, SpawnInfo, Spawned, SubAgentEvent, SubAgentSink,
};
pub use tool::{AgentControlTool, AgentProfilesTool, AgentsTool, NativeAgentTool};
pub use workspace::{ChangeSet, ChangeSetState, WorkspaceManager};
