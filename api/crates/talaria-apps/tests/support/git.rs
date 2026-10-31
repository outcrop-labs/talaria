// git as the talaria-apps tests drive it, to build the repositories they
// fetch from. One home for both kinds of test: the integration tests
// (tests/*.rs) and the lib's live lifecycle tests (src/lifecycle/live.rs)
// each include this file by path.

use std::path::Path;

/// Run git in `dir` with a fixed author; its stdout, trimmed. A failure
/// panics with git's own stderr.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
