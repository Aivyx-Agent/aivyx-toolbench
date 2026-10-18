//! Embeds the git state toolbench was built from as TOOLBENCH_COMMIT:
//! `git describe --always --dirty --abbrev=7` (a `-dirty` suffix marks
//! uncommitted changes), or `unknown` outside a git checkout.

use std::process::Command;

fn main() {
    let commit = Command::new("git")
        .args(["describe", "--always", "--dirty", "--abbrev=7"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=TOOLBENCH_COMMIT={commit}");
    for path in ["HEAD", "refs/heads", "index", "packed-refs"] {
        println!("cargo:rerun-if-changed=../../.git/{path}");
    }
}
