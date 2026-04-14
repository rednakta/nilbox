use std::process::Command;

fn main() {
    // Capture git short hash at compile time
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if !sha.is_empty() {
        println!("cargo:rustc-env=NIL_GIT_SHA={}", sha);
    }

    // Rebuild when git HEAD changes
    println!("cargo:rerun-if-changed=../.git/HEAD");
}
