//! Native sub-agents: profiles, their registry, the shared spawner, the
//! `Agent` tool and Messagent.

pub mod admission;
pub mod messagent;
pub mod profile;
pub mod registry;
pub mod runtime;
pub mod spawn;
pub mod tool;
pub mod workspace;

pub use messagent::{MAIN, MessagentTool};
pub use profile::{AgentProfile, Isolation, ModelPref, ProfileScope, ProfileSource, ReasoningPref};
pub use registry::{
    ProfileCatalog, ProfileDiagnostic, ProfileListing, ProfileRegistry, ProfileSources,
};
pub use runtime::{AgentRuntime, InstanceRecord, RunUsage, RuntimeHandle};
pub use spawn::{
    AgentIdentity, AgentResult, AgentSpawnRequest, AgentSpawner, BatchOptions, ChildTools, Choice,
    CommandRun, DelegationSettings, InstanceState, ParentModel, SkillLoad, SpawnError, SpawnInfo,
    Spawned, SubAgentEvent, SubAgentSink, render_result,
};
pub use tool::{AgentControlTool, AgentProfilesTool, NativeAgentTool};
pub use workspace::{ChangeSet, ChangeSetState, WorkspaceManager};
