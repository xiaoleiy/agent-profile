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
    println!("cargo:rerun-if-changed=.git/HEAD");
}
