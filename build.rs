//! Embeds the git sha into `--version` output (sprint S5-T2). Falls back to
//! "unknown" outside a git checkout (e.g. crates.io builds) so the version
//! string format stays stable either way.

fn main() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AGENT_PROFILE_GIT_SHA={sha}");
    // Rebuild when HEAD moves. `.git/HEAD` alone only changes on checkout —
    // a same-branch commit rewrites the resolved ref file instead, so track
    // that too (or packed-refs when the loose ref file doesn't exist), else
    // `--version` embeds a stale sha after committing on the same branch.
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD")
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        if std::path::Path::new(".git").join(reference).exists() {
            println!("cargo:rerun-if-changed=.git/{reference}");
        } else {
            // Ref is packed (fresh clone / post-`git pack-refs`).
            println!("cargo:rerun-if-changed=.git/packed-refs");
        }
    }
}
