// The api's own git: fetching an app's code at one ref, asking a remote what
// a ref points at, and swapping two directories in one step.
//
// A CREDENTIAL NEVER RIDES IN ARGV OR A URL. The token reaches git through
// the environment of the one git process that needs it, and git asks for it
// through GIT_ASKPASS — a script that holds no secret and answers only for
// the host the fetch was pointed at. No credential helper runs
// (`credential.helper=` empties the list), so nothing can store the token,
// and GIT_TERMINAL_PROMPT=0 makes a missing credential an error instead of a
// prompt nobody will answer.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::source::GitSource;

/// How one git process authenticates.
#[derive(Clone, PartialEq, Eq)]
pub enum GitAuth {
    /// No credential: a public repository, or a local one.
    Anonymous,
    /// A login for `host` (a token is the password, with a throwaway user).
    Login {
        host: String,
        username: String,
        password: String,
    },
}

impl std::fmt::Debug for GitAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitAuth::Anonymous => f.write_str("Anonymous"),
            GitAuth::Login { host, username, .. } => f
                .debug_struct("Login")
                .field("host", host)
                .field("username", username)
                .field("password", &"<redacted>")
                .finish(),
        }
    }
}

/// The most one git command may take.
pub const GIT_DEADLINE: Duration = Duration::from_secs(120);

/// The askpass script. It holds no secret: it prints the username or the
/// password from its own environment, and only when git asks about the host
/// the fetch was pointed at — a redirect to another host gets nothing.
const ASKPASS: &str = r#"#!/bin/sh
case "$1" in
  *"://$TALARIA_GIT_HOST'"*|*"://$TALARIA_GIT_HOST:"*|*"@$TALARIA_GIT_HOST'"*|*"@$TALARIA_GIT_HOST:"*) ;;
  *) exit 1 ;;
esac
case "$1" in
  [Uu]sername*) printf '%s\n' "$TALARIA_GIT_USERNAME" ;;
  *) printf '%s\n' "$TALARIA_GIT_PASSWORD" ;;
esac
"#;

/// One askpass script on disk for the life of a fetch: created fresh under a
/// random name (never followed through a link that was there first), 0700,
/// and removed when dropped.
struct Askpass {
    dir: PathBuf,
}

impl Askpass {
    fn create() -> std::io::Result<Self> {
        let mut bytes = [0u8; 12];
        getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
        let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("talaria-askpass-{name}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir(&dir)?;
        let script = dir.join("askpass");
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o700);
            }
            let mut f = opts.open(&script)?;
            f.write_all(ASKPASS.as_bytes())?;
        }
        Ok(Self { dir })
    }

    fn script(&self) -> PathBuf {
        self.dir.join("askpass")
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The git command every call here runs: no credential helper, no prompt,
/// no stdin, and the askpass wiring when there is a login. Public so the
/// tests can hold it to the rule that the password is in no argument.
pub fn git_command(auth: &GitAuth, askpass: Option<&Path>) -> tokio::process::Command {
    let mut c = tokio::process::Command::new("git");
    c.args(["-c", "credential.helper=", "-c", "core.askPass="])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let (
        GitAuth::Login {
            host,
            username,
            password,
        },
        Some(script),
    ) = (auth, askpass)
    {
        c.env("GIT_ASKPASS", script)
            .env("TALARIA_GIT_HOST", host)
            .env("TALARIA_GIT_USERNAME", username)
            .env("TALARIA_GIT_PASSWORD", password);
    }
    c
}

/// Run git with `args`, under the deadline. stdout on success; on failure
/// git's own stderr, with any credential scrubbed out of it.
async fn run_git(auth: &GitAuth, cwd: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let askpass = match auth {
        GitAuth::Login { .. } => Some(Askpass::create().map_err(|e| format!("git askpass: {e}"))?),
        GitAuth::Anonymous => None,
    };
    let mut cmd = git_command(auth, askpass.as_ref().map(Askpass::script).as_deref());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args);
    let what = args.first().copied().unwrap_or("git");
    let out = tokio::time::timeout(GIT_DEADLINE, cmd.output())
        .await
        .map_err(|_| format!("git {what}: timed out after {}s", GIT_DEADLINE.as_secs()))?
        .map_err(|e| format!("git {what}: {e}"))?;
    drop(askpass);
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let mut err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if let GitAuth::Login { password, .. } = auth
        && !password.is_empty()
    {
        err = err.replace(password.as_str(), "<redacted>");
    }
    Err(err)
}

/// Fetch `src` at `git_ref` (its default branch when None) into `dir`, which
/// must not exist yet, and check that commit out. Answers the commit id. On
/// failure `dir` is removed.
pub async fn fetch_into(
    dir: &Path,
    src: &GitSource,
    git_ref: Option<&str>,
    auth: &GitAuth,
) -> Result<String, String> {
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let result = async {
        let dir_s = dir.to_string_lossy().into_owned();
        run_git(&GitAuth::Anonymous, None, &["init", "--quiet", &dir_s])
            .await
            .map_err(|e| format!("git init failed: {e}"))?;
        let want = git_ref.unwrap_or("HEAD");
        run_git(
            auth,
            Some(dir),
            &[
                "fetch",
                "--quiet",
                "--depth",
                "1",
                "--no-tags",
                "--",
                &src.url,
                want,
            ],
        )
        .await
        .map_err(|e| format!("git fetch failed: {e}"))?;
        run_git(
            &GitAuth::Anonymous,
            Some(dir),
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "--quiet",
                "--detach",
                "FETCH_HEAD",
            ],
        )
        .await
        .map_err(|e| format!("git checkout failed: {e}"))?;
        let sha = run_git(&GitAuth::Anonymous, Some(dir), &["rev-parse", "HEAD"])
            .await
            .map_err(|e| format!("git rev-parse failed: {e}"))?;
        Ok::<String, String>(sha.trim().to_string())
    }
    .await;
    if result.is_err() {
        let _ = remove_tree(dir).await;
    }
    result
}

/// What `git_ref` (the default branch when None) points at on the remote
/// now: the commit id, peeled through an annotated tag. None when the remote
/// has no such ref.
pub async fn remote_head(
    src: &GitSource,
    git_ref: Option<&str>,
    auth: &GitAuth,
) -> Result<Option<String>, String> {
    let want = git_ref.unwrap_or("HEAD");
    let out = run_git(auth, None, &["ls-remote", "--", &src.url, want])
        .await
        .map_err(|e| format!("git ls-remote failed: {e}"))?;
    Ok(pick_ref(&out, want))
}

/// The commit `want` names in `git ls-remote` output: HEAD for HEAD, else a
/// branch, else a tag (its peeled `^{}` line when annotated).
pub fn pick_ref(ls_remote: &str, want: &str) -> Option<String> {
    let lines: Vec<(&str, &str)> = ls_remote
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .collect();
    let find = |name: &str| {
        lines
            .iter()
            .find(|(_, r)| *r == name)
            .map(|(sha, _)| sha.to_string())
    };
    if want == "HEAD" {
        return find("HEAD");
    }
    find(&format!("refs/heads/{want}"))
        .or_else(|| find(&format!("refs/tags/{want}^{{}}")))
        .or_else(|| find(&format!("refs/tags/{want}")))
        .or_else(|| find(want))
}

/// Remove a directory tree. Async so a large tree never blocks a runtime
/// thread; a missing tree is not an error.
pub async fn remove_tree(dir: &Path) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(dir).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(target_os = "linux")]
fn renameat2(from: &Path, to: &Path, flags: libc::c_uint) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both paths are NUL-terminated C strings that outlive the call;
    // renameat2 reads them and touches no other memory. The raw syscall (not
    // the libc wrapper) so a musl build links too.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            flags,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Swap two directories in ONE step: after the call each name holds what the
/// other held, and at no instant is either name missing. The app host's
/// watcher drops an app's database the moment `apps/<slug>` disappears
/// (ui/src/server/app-build/reconcile.ts), so an update can never move the
/// live tree aside and the new one in with two renames. Linux only
/// (renameat2 RENAME_EXCHANGE); elsewhere it refuses rather than leave a gap.
pub fn exchange(a: &Path, b: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        renameat2(a, b, libc::RENAME_EXCHANGE)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (a, b);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "an atomic directory swap needs Linux (renameat2)",
        ))
    }
}

/// Rename `from` to `to`, refusing when `to` exists — even an empty
/// directory, which a plain rename would replace.
pub fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        renameat2(from, to, libc::RENAME_NOREPLACE)
    }
    #[cfg(not(target_os = "linux"))]
    {
        if to.exists() {
            return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
        }
        std::fs::rename(from, to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let mut b = [0u8; 6];
        getrandom::fill(&mut b).unwrap();
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let d = std::env::temp_dir().join(format!("talaria-git-{tag}-{hex}"));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn exchange_swaps_two_trees_and_neither_name_is_ever_missing() {
        let d = scratch("xchg");
        let (a, b) = (d.join("crm"), d.join(".crm.next"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("v"), "old").unwrap();
        std::fs::write(b.join("v"), "new").unwrap();
        exchange(&b, &a).unwrap();
        assert_eq!(std::fs::read_to_string(a.join("v")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(b.join("v")).unwrap(), "old");
        // Exchanging with a name that does not exist is refused, not a move.
        assert!(exchange(&a, &d.join("absent")).is_err());
        assert!(a.exists());
        let c = d.join("c");
        std::fs::create_dir_all(&c).unwrap();
        assert!(
            rename_noreplace(&b, &c).is_err(),
            "an empty target is still a target"
        );
        rename_noreplace(&b, &d.join("free")).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ls_remote_picks_the_branch_then_the_peeled_tag() {
        let out = "aaa\tHEAD\nbbb\trefs/heads/main\nccc\trefs/tags/v1\nddd\trefs/tags/v1^{}\neee\trefs/tags/v2\n";
        assert_eq!(pick_ref(out, "HEAD").as_deref(), Some("aaa"));
        assert_eq!(pick_ref(out, "main").as_deref(), Some("bbb"));
        assert_eq!(pick_ref(out, "v1").as_deref(), Some("ddd"));
        assert_eq!(pick_ref(out, "v2").as_deref(), Some("eee"));
        assert_eq!(pick_ref(out, "nope"), None);
    }

    #[test]
    fn a_login_never_lands_in_an_argument_and_the_debug_form_hides_it() {
        let auth = GitAuth::Login {
            host: "github.com".into(),
            username: "x-access-token".into(),
            password: "ghs_SECRET".into(),
        };
        let cmd = git_command(&auth, Some(Path::new("/tmp/askpass")));
        let std = cmd.as_std();
        for a in std.get_args() {
            assert!(!a.to_string_lossy().contains("ghs_SECRET"));
        }
        let env: Vec<(String, Option<String>)> = std
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let get = |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .and_then(|(_, v)| v.clone())
        };
        assert_eq!(get("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(get("GIT_ASKPASS").as_deref(), Some("/tmp/askpass"));
        assert_eq!(get("TALARIA_GIT_PASSWORD").as_deref(), Some("ghs_SECRET"));
        assert_eq!(get("TALARIA_GIT_HOST").as_deref(), Some("github.com"));
        assert!(!format!("{auth:?}").contains("ghs_SECRET"));
        // Anonymous: no askpass at all, so a private repository is an error.
        let anon = git_command(&GitAuth::Anonymous, None);
        assert!(
            anon.as_std()
                .get_envs()
                .all(|(k, v)| k != "GIT_ASKPASS" || v.is_none())
        );
    }
}
