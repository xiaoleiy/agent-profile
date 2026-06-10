use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::error::Error;
use crate::schema::types::{API_VERSION, CapabilityBlock, Profile};
use crate::schema::validate::{Code, Finding};

/// A parsed file plus the raw source needed for provenance and the
/// same-line `# agent-profile: allow-literal` escape hatch.
#[derive(Debug, Clone)]
pub struct SourceFile<T> {
    pub value: T,
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Path relative to the workspace root, e.g. `profiles/reviewer.yaml`.
    /// Used verbatim in provenance output.
    pub rel: String,
    pub raw: String,
}

/// Handle to a `.agent-profile/` directory.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Resolve the workspace: `--dir` override, else walk up from `cwd`.
    pub fn locate(dir_override: Option<&Path>, cwd: &Path) -> Result<Self, Error> {
        match dir_override {
            Some(dir) => Self::at(dir),
            None => Self::discover(cwd),
        }
    }

    /// `--dir` override. Accepts either the `.agent-profile` directory itself
    /// or a directory containing one.
    pub fn at(dir: &Path) -> Result<Self, Error> {
        let looks_like_workspace = dir.is_dir()
            && (dir.file_name().is_some_and(|n| n == ".agent-profile")
                || dir.join("profiles").is_dir()
                || dir.join("capabilities").is_dir());
        if looks_like_workspace {
            return Ok(Self {
                root: dir.to_path_buf(),
            });
        }
        let nested = dir.join(".agent-profile");
        if nested.is_dir() {
            return Ok(Self { root: nested });
        }
        Err(Error::WorkspaceDirInvalid {
            dir: dir.to_path_buf(),
        })
    }

    /// Walk up from `start` looking for a `.agent-profile/` directory.
    pub fn discover(start: &Path) -> Result<Self, Error> {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            let candidate = dir.join(".agent-profile");
            if candidate.is_dir() {
                return Ok(Self { root: candidate });
            }
            cur = dir.parent();
        }
        Err(Error::WorkspaceNotFound {
            start: start.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profile_path(&self, name: &str) -> PathBuf {
        self.root.join("profiles").join(format!("{name}.yaml"))
    }

    pub fn capability_path(&self, name: &str) -> PathBuf {
        self.root.join("capabilities").join(format!("{name}.yaml"))
    }

    pub fn profile_names(&self) -> Result<Vec<String>, Error> {
        list_yaml_stems(&self.root.join("profiles"))
    }

    pub fn capability_names(&self) -> Result<Vec<String>, Error> {
        list_yaml_stems(&self.root.join("capabilities"))
    }

    pub fn load_profile(&self, name: &str) -> Result<SourceFile<Profile>, Error> {
        // A role is the filename stem of a file inside profiles/ (design §1.1).
        // `--role`/`include` names must be a single path component — never a
        // relative (`../`) or absolute path that could escape the profiles dir.
        if !is_safe_component(name) {
            return Err(Error::UnknownRole {
                role: name.to_string(),
                path: self.profile_path(name),
            });
        }
        let path = self.profile_path(name);
        if !path.is_file() {
            return Err(Error::UnknownRole {
                role: name.to_string(),
                path,
            });
        }
        let rel = format!("profiles/{name}.yaml");
        let src: SourceFile<Profile> = parse_source(&path, &rel)?;
        check_api_version(&src.value.api_version, &src.rel)?;
        Ok(src)
    }

    pub fn load_capability(&self, name: &str) -> Result<SourceFile<CapabilityBlock>, Error> {
        // Capability blocks are flat, one level (design §1.1/§1.3): an include
        // name is a block name, not a filesystem path. Reject any name that
        // could traverse out of capabilities/ (e.g. `../../../outside/evil`).
        if !is_safe_component(name) {
            return Err(Error::Validation(vec![Finding::new(
                Code::MissingCapability,
                format!(
                    "include `{name}` is not a valid capability block name — capability blocks \
                     are flat (one level); names cannot contain path separators or `..`"
                ),
                Some("include".to_string()),
                Some("include".to_string()),
            )]));
        }
        let path = self.capability_path(name);
        if !path.is_file() {
            return Err(Error::Validation(vec![Finding::new(
                Code::MissingCapability,
                format!(
                    "included capability block `{name}` not found at {}",
                    path.display()
                ),
                Some(format!("capabilities/{name}.yaml")),
                Some("include".to_string()),
            )]));
        }
        let rel = format!("capabilities/{name}.yaml");
        let src: SourceFile<CapabilityBlock> = parse_source(&path, &rel)?;
        check_api_version(&src.value.api_version, &src.rel)?;
        Ok(src)
    }
}

/// Is `name` a single, safe path component? Used to gate `--role` and
/// `include` names so they can never escape the profiles/capabilities dirs
/// via `..` or absolute/relative path syntax (design §1.1/§1.3).
pub fn is_safe_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

fn check_api_version(found: &str, rel: &str) -> Result<(), Error> {
    if found == API_VERSION {
        Ok(())
    } else {
        Err(Error::Validation(vec![Finding::new(
            Code::ApiVersion,
            format!("unsupported apiVersion `{found}` (only `{API_VERSION}` is accepted)"),
            Some(rel.to_string()),
            Some("apiVersion".to_string()),
        )]))
    }
}

fn parse_source<T: DeserializeOwned>(path: &Path, rel: &str) -> Result<SourceFile<T>, Error> {
    let raw = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    match serde_yaml::from_str::<T>(&raw) {
        Ok(value) => Ok(SourceFile {
            value,
            path: path.to_path_buf(),
            rel: rel.to_string(),
            raw,
        }),
        Err(err) => Err(Error::Validation(vec![Finding::new(
            Code::SchemaParse,
            err.to_string(),
            Some(rel.to_string()),
            None,
        )])),
    }
}

fn list_yaml_stems(dir: &Path) -> Result<Vec<String>, Error> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        // Discovery must agree with resolution: profiles/capabilities are
        // `<name>.yaml` only (design §1.1). A `.yml` file is not a profile, so
        // it must not appear in `list` and then fail to load in show/validate.
        let is_yaml = path.extension().is_some_and(|ext| ext == "yaml");
        if path.is_file()
            && is_yaml
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}
