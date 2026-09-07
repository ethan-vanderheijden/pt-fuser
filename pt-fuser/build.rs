use std::{env, path::Path, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The commit CI already knows, so no runner has to ask Git for it.
fn commit_from_env() -> Option<String> {
    let sha = env::var("PT_FUSER_COMMIT").ok()?;
    let sha = sha.trim();
    // GitHub Actions exposes the full SHA; shorten it the way Git would.
    (!sha.is_empty()).then(|| sha.chars().take(7).collect())
}

fn commit_from_git() -> Option<String> {
    // Resolve Git paths so normal checkouts and worktrees both rebuild when
    // HEAD moves, including when refs have been packed.
    for name in ["HEAD", "refs", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo::rerun-if-changed={path}");
        }
    }

    git(&["rev-parse", "--short=7", "HEAD"])
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=PT_FUSER_COMMIT");

    // The commit is optional: builds without it report only the version.
    let commit = commit_from_env()
        .or_else(commit_from_git)
        .map(|commit| format!(" ({commit})"))
        .unwrap_or_default();

    println!(
        "cargo::rustc-env=PT_FUSER_VERSION={}{commit}",
        env!("CARGO_PKG_VERSION")
    );
}
