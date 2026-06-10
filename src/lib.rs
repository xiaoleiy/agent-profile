//! agent-profile — spawn-time role-profile resolver for coding-agent runtimes.
//!
//! All business logic lives in this library; `main.rs` is a thin clap shell.
//! Design contract: `docs/product/design.md`.

pub mod adapters;
pub mod cli;
pub mod doctor;
pub mod error;
pub mod schema;
pub mod state;

pub use error::{Error, ExitCode};

/// Version of every `--json` output envelope (design §2).
pub const JSON_SCHEMA_VERSION: u32 = 1;
