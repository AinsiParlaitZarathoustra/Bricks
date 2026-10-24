//! # Bricks
//!
//! A modular Rust SDK for building coding agents programmatically.
//!
//! Bricks provides a high-level `Agent` builder that combines LLM providers,
//! tools, memory, permissions, and hooks into a complete agentic system.
//! Every component is modular and replaceable.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use bricks::prelude::*;
//!
//! let output = Agent::builder()
//!     // Models come from ~/.bricks/providers.toml, selected as `provider_id/model_id`.
//!     .provider(bricks::provider_from_config(None, "my-provider/my-model")?)
//!     .tools(bricks::tools::coding())
//!     .run_with("Fix the failing tests")
//!     .await?;
//!
//! println!("{}", output.text());
//! ```

// Re-export sub-crates
pub use bricks_agent::events::{
    self as events, AgentEvent, AgentStream, CompactReason, WarningState,
};
pub use bricks_agent::reporters;
pub use bricks_agent::{Agent, AgentBuilder, AgentOutput, Reporter};
#[cfg(feature = "agentlang")]
pub use bricks_agentlang as agentlang;
#[cfg(feature = "agentrl")]
pub use bricks_agentrl as agentrl;
pub use bricks_hooks as hooks;
pub use bricks_mcp as mcp;
pub use bricks_memory as memory;
pub use bricks_provider as provider;
/// Agent Skills (`SKILL.md`): format, sources, the store, trust, activation.
pub use bricks_skills as skills;
pub use bricks_tools as tools;
pub use bricks_types as types;
#[cfg(feature = "vms")]
pub use bricks_vms as vms;
#[cfg(feature = "workflows")]
pub use bricks_workflows as workflows;

// Providers are configuration, not types: load a registry and resolve
// `provider_id/model_id` (see `docs/providers.md`).
pub use bricks_provider::{
    ConfiguredProvider, ProviderRegistry, ResolvedModel, provider_from_config,
};

/// The prelude — import this for the most common types.
pub mod prelude {
    // Core agent
    pub use crate::{Agent, AgentBuilder, AgentOutput};
    pub use crate::{AgentEvent, AgentStream, Reporter};

    // Providers
    pub use crate::provider::{CompletionRequest, Provider, ProviderOptions};
    pub use crate::{ConfiguredProvider, ProviderRegistry, provider_from_config};

    // Types
    pub use bricks_types::{
        CerseiError, ContentBlock, DocumentSource, ImageSource, MediaKind, Message, MessageContent,
        MessageMetadata, Result, Role, StopReason, StreamEvent, ToolDefinition, Usage,
    };

    // Tools
    pub use bricks_tools::permissions::{
        AllowAll, AllowReadOnly, DenyAll, InteractivePolicy, PermissionDecision, PermissionPolicy,
        PermissionRequest, RuleBased,
    };
    pub use bricks_tools::{
        CostTracker, Extensions, PermissionLevel, Tool, ToolCategory, ToolContext, ToolExecute,
        ToolResult,
    };

    // Memory
    pub use bricks_memory::Memory;

    // Hooks
    pub use bricks_hooks::{Hook, HookAction, HookContext, HookEvent};

    // Derive macro
    pub use bricks_tools_derive::Tool;

    // Workflows
    #[cfg(feature = "workflows")]
    pub use bricks_workflows::{
        FnStep, RunStatus, Step, StepContext, StepOutcome, StepRegistry, Workflow, WorkflowBuilder,
        WorkflowDef, WorkflowEvent, WorkflowResult,
    };

    // Re-export for derive macro usage
    pub use async_trait::async_trait;
    pub use schemars;
    pub use serde;
    pub use serde_json;
}
