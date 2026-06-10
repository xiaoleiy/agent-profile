//! Profile + capability schema (serde), workspace discovery, include
//! resolution/merge, validation, and the secret-literal scanner.

pub mod merge;
pub mod secrets;
pub mod types;
pub mod validate;
pub mod workspace;

pub use merge::{ResolvedProfile, resolve};
pub use types::{
    API_VERSION, CapabilityBlock, CapabilitySet, Effort, McpServer, McpServerType, ModelSpec,
    PermissionMode, Profile, Target, ToolsSpec,
};
pub use workspace::Workspace;
