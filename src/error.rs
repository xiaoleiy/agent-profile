use std::path::PathBuf;

use crate::schema::validate::Finding;

/// Process exit codes, uniform across commands (design §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    /// Success / no findings / no diff.
    Success = 0,
    /// Internal or I/O error.
    Internal = 1,
    /// Validation failure (schema, includes, secret literal, unknown role/target).
    Validation = 2,
    /// Drift detected.
    Drift = 3,
    /// Doctor found errors.
    Doctor = 4,
    /// Session/state error.
    Session = 5,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// Library error type. The CLI shell maps each variant to an [`ExitCode`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("no .agent-profile/ directory found walking up from {start}")]
    WorkspaceNotFound { start: PathBuf },

    #[error("{dir} is not a directory")]
    WorkspaceDirInvalid { dir: PathBuf },

    #[error("unknown role profile `{role}` (no {path} file)")]
    UnknownRole { role: String, path: PathBuf },

    #[error("validation failed:\n{}", format_findings(.0))]
    Validation(Vec<Finding>),

    #[error("command not implemented yet (planned for a later sprint)")]
    NotImplemented,
}

fn format_findings(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| format!("  {f}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Error {
    /// Central exit-code mapping (design §2): the single source of truth used
    /// by `main.rs`.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Error::Io { .. } | Error::NotImplemented => ExitCode::Internal,
            Error::WorkspaceNotFound { .. }
            | Error::WorkspaceDirInvalid { .. }
            | Error::UnknownRole { .. }
            | Error::Validation(_) => ExitCode::Validation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_design_table() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::Internal.code(), 1);
        assert_eq!(ExitCode::Validation.code(), 2);
        assert_eq!(ExitCode::Drift.code(), 3);
        assert_eq!(ExitCode::Doctor.code(), 4);
        assert_eq!(ExitCode::Session.code(), 5);
    }

    #[test]
    fn error_variants_map_to_exit_codes() {
        let io = Error::Io {
            path: PathBuf::from("/x"),
            source: std::io::Error::other("boom"),
        };
        assert_eq!(io.exit_code(), ExitCode::Internal);
        assert_eq!(Error::NotImplemented.exit_code(), ExitCode::Internal);
        assert_eq!(
            Error::WorkspaceNotFound {
                start: PathBuf::from("/x")
            }
            .exit_code(),
            ExitCode::Validation
        );
        assert_eq!(
            Error::UnknownRole {
                role: "x".into(),
                path: PathBuf::from("/x")
            }
            .exit_code(),
            ExitCode::Validation
        );
        assert_eq!(Error::Validation(vec![]).exit_code(), ExitCode::Validation);
    }
}
