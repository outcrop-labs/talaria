// The container side of a rewind: what runs by docker exec inside the agent's
// managed container, as the agent's own user, following `lost_turns`.
//
// Hermes' HTTP fork copies a whole session and cannot cut it, so the cut goes
// through its session store in-process, the way its own /retry and /undo do.
// That is an internal, not a public API: the canary carries out a fork and a
// cut on every release before it reaches an agent, `probe` checks the symbols
// before a rewind touches anything, and a Hermes that renamed them fails the
// rewind instead of corrupting a transcript. The checkout is restored with git
// alone.

use serde_json::Value;
use sqlx::PgPool;

use crate::{docker_exec_as, managed_container};

const AGENT_USER: &str = "hermes";
const PYTHON: &str = "/opt/hermes/.venv/bin/python";
const EXEC_TIMEOUT_MS: u64 = 60_000;

/// Prints `ok`, or `missing: ` and what the scripts below read that this Hermes
/// lacks. `replace_messages` must archive the dropped suffix: a plain delete
/// would leave nothing to recover and evict the rows from the search index.
const PROBE_PY: &str = r#"
import inspect, sys
sys.path.insert(0, "/opt/hermes")
try:
    from hermes_state import SessionDB
    from hermes_state_mutation_retirement import RETIRED_PREFIX
except ImportError as e:
    print("missing: %s" % e.name)
    sys.exit(0)
missing = [n for n in ("get_messages", "get_session", "delete_session", "_execute_write", "_read_ctx", "_session_turn_lease_key") if not callable(getattr(SessionDB, n, None))]
replace = getattr(SessionDB, "replace_messages", None)
if replace is None or "archive_dropped" not in inspect.signature(replace).parameters:
    missing.append("replace_messages(archive_dropped)")
if "_read_ctx" not in missing:
    with SessionDB()._read_ctx() as conn:
        columns = {row[1] for row in conn.execute("PRAGMA table_info(session_turn_leases)")}
    if not {"conversation_id", "expires_at"} <= columns:
        missing.append("session_turn_leases(conversation_id, expires_at)")
print("missing: " + ", ".join(missing) if missing else "ok")
"#;

/// argv: session id. Hermes holds a `session_turn_leases` row for the whole
/// of a running turn, keyed by the conversation's lineage root; a session
/// with no unexpired row has no turn running. Prints `idle`, or `busy` and the
/// seconds the lease has left.
const IDLE_PY: &str = r#"
import sys, time
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
db = SessionDB()
key = db._session_turn_lease_key(sys.argv[1])
with db._read_ctx() as conn:
    row = conn.execute("SELECT expires_at FROM session_turn_leases WHERE conversation_id = ?", (key,)).fetchone()
left = float(row[0]) - time.time() if row is not None else 0
print("busy %d" % (left + 1) if left > 0 else "idle")
"#;

/// argv: session id, tool call id. Keeps every row through the result row of
/// that call and archives the rest. Prints one json line.
const CUT_PY: &str = r#"
import json, sys
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
sid, call = sys.argv[1], sys.argv[2]
db = SessionDB()
rows = db.get_messages(sid)
at = next((i for i, m in enumerate(rows) if m.get("role") == "tool" and m.get("tool_call_id") == call), None)
if at is None:
    print(json.dumps({"ok": False, "why": "the session has no result row for tool call %s" % call, "rows": len(rows)}))
    sys.exit(0)
keep = rows[: at + 1]
if len(keep) < len(rows):
    db.replace_messages(sid, keep, archive_dropped=True)
now = db.get_messages(sid)
print(json.dumps({"ok": True, "was": len(rows), "now": len(now), "last": (now[-1].get("tool_call_id") if now else None)}))
"#;

/// argv: session id, parent session id. Removes a fork a failed rewind left,
/// and only that: `gone` when the id is free after, `foreign` when the session
/// is not a fork of that parent. Hermes fences a deleted id for good, which
/// would spend the slot, and no turn ever ran on this one for the fence to
/// guard, so it is lifted.
const DISCARD_PY: &str = r#"
import sys
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
from hermes_state_mutation_retirement import RETIRED_PREFIX
sid, parent = sys.argv[1], sys.argv[2]
db = SessionDB()
row = db.get_session(sid)
if row is not None and row.get("parent_session_id") != parent:
    print("foreign")
    sys.exit(0)
if row is not None:
    db.delete_session(sid, exclude_active_write_guards=True)
    db._execute_write(lambda conn: conn.execute("DELETE FROM state_meta WHERE key = ?", (RETIRED_PREFIX + sid,)))
print("gone")
"#;

/// argv: session id. Writes the transcript the canary forks and cuts.
const SEED_PY: &str = r#"
import sys
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
def call(id):
    return {"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function", "function": {"name": "terminal", "arguments": "{}"}}]}
SessionDB().replace_messages(sys.argv[1], [
    {"role": "user", "content": "canary"},
    call("canary_call_1"),
    {"role": "tool", "tool_call_id": "canary_call_1", "content": "one"},
    call("canary_call_2"),
    {"role": "tool", "tool_call_id": "canary_call_2", "content": "two"},
    {"role": "assistant", "content": "done"},
])
print("ok")
"#;

/// The rows [`seed_session`] writes, and the call the canary cuts at: its
/// result row is the third.
pub const SEED_ROWS: usize = 6;
pub const SEED_CUT_CALL: &str = "canary_call_1";
pub const SEED_CUT_ROWS: usize = 3;

/// $1 workdir. The checkout the agent made under a job's workdir: the workdir
/// itself, or the one directory beneath it, as the snapshot plugin finds it.
const ROOT_SH: &str = r#"
dir="$1"
if [ -d "$dir/.git" ]; then printf '%s\n' "$dir"; exit 0; fi
for kid in "$dir"/*/; do
  [ -d "$kid.git" ] && printf '%s\n' "${kid%/}"
done
"#;

/// $1 source checkout, $2 new checkout, $3 commit, $4 branch, $5 origin url.
/// An attempt starts from the step and knows nothing after it, so the new
/// repo is made empty and fetches that commit by id: a clone would bring the
/// source's later commits, branches and snapshot refs. Only the refs that stood
/// at or before the step come with it, so the base branch stays to diff
/// against. A half-made checkout from an earlier try is replaced.
const RESTORE_SH: &str = r#"
set -e
src="$1"; dst="$2"; commit="$3"; branch="$4"; origin="$5"
rm -rf -- "$dst"
mkdir -p -- "$dst"
git -C "$dst" init -q
git -C "$dst" remote add origin "$origin"
git -C "$dst" fetch -q --no-tags -- "$src" "$commit"
set --
for ref in $(git -C "$src" for-each-ref --merged "$commit" --format='%(refname)' refs/remotes/origin refs/tags); do
  [ "$ref" = refs/remotes/origin/HEAD ] || set -- "$@" "+$ref:$ref"
done
[ $# -eq 0 ] || git -C "$dst" fetch -q --no-tags -- "$src" "$@"
head=$(git -C "$src" symbolic-ref -q refs/remotes/origin/HEAD || true)
if [ -n "$head" ] && git -C "$dst" show-ref -q --verify "$head"; then
  git -C "$dst" symbolic-ref refs/remotes/origin/HEAD "$head"
fi
git -C "$dst" checkout -q -b "$branch" "$commit"
rm -f -- "$dst/.git/FETCH_HEAD"
git -C "$dst" rev-parse 'HEAD^{tree}'
"#;

/// $1 checkout, $2 branch. Prints the tree id, then the branch, then any
/// uncommitted change.
const VERIFY_SH: &str = r#"
set -e
dst="$1"
git -C "$dst" rev-parse 'HEAD^{tree}'
git -C "$dst" rev-parse --abbrev-ref HEAD
git -C "$dst" status --porcelain
"#;

async fn container_of(pg: &PgPool, agent_model: &str) -> Result<(String, String), String> {
    let department: Option<(String,)> =
        sqlx::query_as("select department from agent_defs where model = $1 and managed")
            .bind(agent_model)
            .fetch_optional(pg)
            .await
            .map_err(|e| format!("could not look up {agent_model}: {e}"))?;
    let Some((department,)) = department else {
        return Err(format!(
            "{agent_model} is not a managed agent, so nothing here can reach its container"
        ));
    };
    Ok((managed_container(pg, &department).await, department))
}

/// The managed container's department: the name every other container-side
/// call (teardown, discard) is keyed by.
pub async fn department_of(pg: &PgPool, agent_model: &str) -> Result<String, String> {
    container_of(pg, agent_model).await.map(|(_, d)| d)
}

async fn exec(pg: &PgPool, agent_model: &str, command: &[&str]) -> Result<String, String> {
    let (container, _) = container_of(pg, agent_model).await?;
    exec_in(&container, command).await
}

async fn exec_in(container: &str, command: &[&str]) -> Result<String, String> {
    let (out, _) = docker_exec_as(container, Some(AGENT_USER), command, None, EXEC_TIMEOUT_MS)
        .await
        .map_err(|e| format!("in {container}: {e}"))?;
    Ok(out)
}

/// Whether a failed exec is docker's and says nothing of the Hermes inside:
/// the daemon refused, the container is not there, or the exec never finished.
pub fn docker_failed(e: &str) -> bool {
    [
        "Error response from daemon",
        "Cannot connect to the Docker daemon",
        "docker exec ",
    ]
    .iter()
    .any(|m| e.contains(m))
}

/// Why a container cannot be asked for a rewind.
#[derive(Debug, Clone, PartialEq)]
pub enum Unready {
    /// This Hermes lacks what a rewind reads; the words name what.
    Lacks(String),
    /// Docker could not ask, so nothing is known of the Hermes.
    NoAnswer(String),
}

impl std::fmt::Display for Unready {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unready::Lacks(what) => {
                write!(f, "this agent's Hermes does not support rewind ({what})")
            }
            Unready::NoAnswer(e) => write!(f, "the agent could not be asked about rewind: {e}"),
        }
    }
}

fn probe_verdict(answer: Result<String, String>) -> Result<(), Unready> {
    match answer {
        Ok(out) if out.trim() == "ok" => Ok(()),
        Ok(out) => Err(Unready::Lacks(out.trim().to_string())),
        Err(e) if docker_failed(&e) => Err(Unready::NoAnswer(e)),
        Err(e) => Err(Unready::Lacks(
            e.lines().last().unwrap_or(&e).trim().to_string(),
        )),
    }
}

/// What a rewind reads from Hermes, and the container answering at all.
pub async fn probe(pg: &PgPool, agent_model: &str) -> Result<(), Unready> {
    let (container, _) = container_of(pg, agent_model)
        .await
        .map_err(Unready::NoAnswer)?;
    probe_container(&container).await
}

/// [`probe`] on a container by name, as the canary asks a release before it
/// reaches an agent.
pub async fn probe_container(container: &str) -> Result<(), Unready> {
    probe_verdict(exec_in(container, &[PYTHON, "-c", PROBE_PY]).await)
}

/// Write the transcript the canary forks and cuts into an existing session.
pub async fn seed_session(container: &str, session_id: &str) -> Result<(), String> {
    exec_in(container, &[PYTHON, "-c", SEED_PY, session_id])
        .await
        .map(|_| ())
}

/// The git checkout under a job's workdir, or why there is none.
pub async fn git_root(pg: &PgPool, agent_model: &str, workdir: &str) -> Result<String, String> {
    let out = exec(pg, agent_model, &["sh", "-c", ROOT_SH, "sh", workdir]).await?;
    let roots: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    match roots.as_slice() {
        [root] => Ok((*root).to_string()),
        [] => Err(format!("no git checkout under {workdir}")),
        _ => Err(format!("{} checkouts under {workdir}", roots.len())),
    }
}

/// Remove the fork of `parent` a failed rewind left under `session_id`, so the
/// next rewind can use the id.
pub async fn discard_session(
    pg: &PgPool,
    agent_model: &str,
    session_id: &str,
    parent: &str,
) -> Result<(), String> {
    let (container, _) = container_of(pg, agent_model).await?;
    discard_session_in(&container, session_id, parent).await
}

pub async fn discard_session_in(
    container: &str,
    session_id: &str,
    parent: &str,
) -> Result<(), String> {
    if !plain(session_id) || !plain(parent) {
        return Err("the session id is not one this can name".into());
    }
    let out = exec_in(container, &[PYTHON, "-c", DISCARD_PY, session_id, parent]).await?;
    match out.trim() {
        "gone" => Ok(()),
        "foreign" => Err(format!("session {session_id} is not a fork of {parent}")),
        other => Err(format!("the discard answered {other:?}")),
    }
}

/// The turn lease on a session, as the agent's store holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLease {
    Idle,
    Held { ends_in_secs: u64 },
}

/// The lease on `session_id`: the proof a fork will not tear a turn when the
/// turn itself cannot be named.
pub async fn session_lease(
    pg: &PgPool,
    agent_model: &str,
    session_id: &str,
) -> Result<SessionLease, String> {
    let (container, _) = container_of(pg, agent_model).await?;
    session_lease_in(&container, session_id).await
}

pub async fn session_lease_in(container: &str, session_id: &str) -> Result<SessionLease, String> {
    if !plain(session_id) {
        return Err("the session id is not one this can name".into());
    }
    let out = exec_in(container, &[PYTHON, "-c", IDLE_PY, session_id]).await?;
    match out.trim().split_once(' ') {
        None if out.trim() == "idle" => Ok(SessionLease::Idle),
        Some(("busy", secs)) if secs.parse::<u64>().is_ok() => Ok(SessionLease::Held {
            ends_in_secs: secs.parse().unwrap_or(0),
        }),
        _ => Err(format!("the idle probe answered {:?}", out.trim())),
    }
}

/// Whether `commit` is in the checkout at `root`.
pub async fn commit_exists(
    pg: &PgPool,
    agent_model: &str,
    root: &str,
    commit: &str,
) -> Result<bool, String> {
    let out = exec(
        pg,
        agent_model,
        &[
            "sh",
            "-c",
            r#"git -C "$1" cat-file -e "$2^{commit}" && echo yes || echo no"#,
            "sh",
            root,
            commit,
        ],
    )
    .await?;
    Ok(out.trim() == "yes")
}

/// What the cut left: how many rows the session had and has, and the last
/// row's tool call id.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    pub was: u64,
    pub now: u64,
    pub last_tool_call_id: Option<String>,
}

pub async fn cut_session(
    pg: &PgPool,
    agent_model: &str,
    session_id: &str,
    tool_call_id: &str,
) -> Result<Cut, String> {
    let (container, _) = container_of(pg, agent_model).await?;
    cut_session_in(&container, session_id, tool_call_id).await
}

pub async fn cut_session_in(
    container: &str,
    session_id: &str,
    tool_call_id: &str,
) -> Result<Cut, String> {
    if !plain(session_id) || !plain(tool_call_id) {
        return Err("the session id or tool call id is not one this can name".into());
    }
    let out = exec_in(container, &[PYTHON, "-c", CUT_PY, session_id, tool_call_id]).await?;
    let answer: Value = serde_json::from_str(out.trim()).map_err(|_| {
        format!(
            "the cut answered something that is not json: {:?}",
            out.trim()
        )
    })?;
    if answer["ok"].as_bool() != Some(true) {
        return Err(answer["why"]
            .as_str()
            .unwrap_or("the cut did not say why it failed")
            .to_string());
    }
    Ok(Cut {
        was: answer["was"].as_u64().unwrap_or(0),
        now: answer["now"].as_u64().unwrap_or(0),
        last_tool_call_id: answer["last"].as_str().map(str::to_string),
    })
}

/// Where a restore puts the files and what it checks out.
#[derive(Debug, Clone, PartialEq)]
pub struct Restore<'a> {
    pub source_root: &'a str,
    pub root: &'a str,
    pub commit: &'a str,
    pub branch: &'a str,
    pub origin_url: &'a str,
}

/// Answers the restored checkout's tree id.
pub async fn restore_checkout(
    pg: &PgPool,
    agent_model: &str,
    restore: &Restore<'_>,
) -> Result<String, String> {
    let out = exec(
        pg,
        agent_model,
        &[
            "sh",
            "-c",
            RESTORE_SH,
            "sh",
            restore.source_root,
            restore.root,
            restore.commit,
            restore.branch,
            restore.origin_url,
        ],
    )
    .await?;
    Ok(out.trim().to_string())
}

/// A restored checkout as it stands: its tree id, its branch, and whether
/// anything is uncommitted.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkout {
    pub tree: String,
    pub branch: String,
    pub clean: bool,
}

pub async fn read_checkout(pg: &PgPool, agent_model: &str, root: &str) -> Result<Checkout, String> {
    let out = exec(pg, agent_model, &["sh", "-c", VERIFY_SH, "sh", root]).await?;
    let mut lines = out.lines();
    let tree = lines.next().unwrap_or_default().trim().to_string();
    let branch = lines.next().unwrap_or_default().trim().to_string();
    let clean = lines.all(|l| l.trim().is_empty());
    if tree.is_empty() {
        return Err(format!("no checkout at {root}"));
    }
    Ok(Checkout {
        tree,
        branch,
        clean,
    })
}

/// Remove a checkout a failed rewind left behind. Nothing to remove is fine.
pub async fn remove_checkout(pg: &PgPool, agent_model: &str, workdir: &str) -> Result<(), String> {
    exec(pg, agent_model, &["rm", "-rf", "--", workdir])
        .await
        .map(|_| ())
}

fn plain(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_ids_reach_the_cut() {
        assert!(plain("f48a3937-c96c-4907-b8b7-2618a91d1a1e"));
        assert!(plain("call_1793767055076"));
        for bad in ["", "--all", "-x", "a b", "a;b", "a/b", &"x".repeat(129)] {
            assert!(!plain(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_hermes_that_lacks_a_symbol_is_told_apart_from_docker_failing() {
        assert_eq!(probe_verdict(Ok("ok\n".into())), Ok(()));
        let lacks = probe_verdict(Ok("missing: replace_messages(archive_dropped)\n".into()));
        assert_eq!(
            lacks.unwrap_err().to_string(),
            "this agent's Hermes does not support rewind (missing: replace_messages(archive_dropped))"
        );
        let crashed = "in c: Traceback (most recent call last):\n  File \"<string>\", line 1\nAttributeError: gone";
        assert_eq!(
            probe_verdict(Err(crashed.into())),
            Err(Unready::Lacks("AttributeError: gone".into()))
        );
        let moved = "in c: OCI runtime exec failed: exec failed: unable to start container process: exec: \"/opt/hermes/.venv/bin/python\": stat /opt/hermes/.venv/bin/python: no such file or directory";
        assert!(matches!(
            probe_verdict(Err(moved.into())),
            Err(Unready::Lacks(_))
        ));
        for docker in [
            "in c: Error response from daemon: container abc is not running",
            "in c: Error response from daemon: No such container: c",
            "in c: Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
            "in c: docker exec c …: timed out",
        ] {
            assert!(
                matches!(probe_verdict(Err(docker.into())), Err(Unready::NoAnswer(_))),
                "{docker}"
            );
        }
    }

    /// A repo with a step and history past it, as a job leaves one: a pushed
    /// branch and snapshot refs beyond the step, and a base branch before it.
    struct Fixture {
        dir: std::path::PathBuf,
        base: String,
        step: String,
        later: String,
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> (bool, String) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .expect("git runs");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
    }

    fn ok(dir: &std::path::Path, args: &[&str]) -> String {
        let (done, out) = git(dir, args);
        assert!(done, "git {args:?}: {out}");
        out
    }

    fn commit(src: &std::path::Path, file: &str) -> String {
        std::fs::write(src.join(file), file).unwrap();
        ok(src, &["add", "-A"]);
        ok(src, &["commit", "-qm", file]);
        ok(src, &["rev-parse", "HEAD"])
    }

    fn snapshot(src: &std::path::Path, name: &str) -> String {
        let tree = ok(src, &["rev-parse", "HEAD^{tree}"]);
        let snap = ok(src, &["commit-tree", &tree, "-p", "HEAD", "-m", name]);
        ok(
            src,
            &[
                "update-ref",
                &format!("refs/talaria/ckpt/run/{name}"),
                &snap,
            ],
        );
        snap
    }

    fn fixture(name: &str) -> Fixture {
        let dir =
            std::env::temp_dir().join(format!("talaria-rewind-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        ok(&dir, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
        ok(&dir, &["clone", "-q", "origin.git", "src"]);
        let src = dir.join("src");
        ok(&src, &["checkout", "-q", "-b", "main"]);
        let base = commit(&src, "base");
        ok(&src, &["push", "-q", "origin", "main"]);
        ok(&src, &["remote", "set-head", "origin", "main"]);
        ok(&src, &["checkout", "-q", "-b", "job"]);
        commit(&src, "step");
        let step = snapshot(&src, "at-step");
        let later = commit(&src, "later");
        snapshot(&src, "past-step");
        ok(&src, &["tag", "past-tag"]);
        ok(&src, &["push", "-q", "origin", "job"]);
        Fixture {
            dir,
            base,
            step,
            later,
        }
    }

    fn restore(f: &Fixture) -> std::path::PathBuf {
        let dst = f.dir.join("jobs/attempt/repo");
        let origin = f.dir.join("origin.git");
        let out = std::process::Command::new("sh")
            .args(["-c", RESTORE_SH, "sh"])
            .arg(f.dir.join("src"))
            .arg(&dst)
            .arg(&f.step)
            .arg("job-a2")
            .arg(&origin)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("sh runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        dst
    }

    #[test]
    fn a_restored_checkout_holds_nothing_past_its_step() {
        let f = fixture("past");
        let dst = restore(&f);
        let refs = ok(&dst, &["for-each-ref", "--format=%(refname)"]);
        let refs: Vec<&str> = refs.lines().collect();
        assert_eq!(
            refs,
            vec![
                "refs/heads/job-a2",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main"
            ],
            "only the attempt's branch and what stood at or before the step"
        );
        let all = ok(&dst, &["log", "--all", "--format=%H"]);
        assert!(all.contains(&f.step) && all.contains(&f.base), "{all}");
        assert!(!all.contains(&f.later), "git log --all reaches the future");
        assert!(
            !git(
                &dst,
                &["cat-file", "-e", &format!("{}^{{commit}}", f.later)]
            )
            .0,
            "the later commit is not in the object store at all"
        );
        assert!(!ok(&dst, &["reflog", "--all"]).contains(&f.later[..7]));
        assert!(
            !dst.join(".git/FETCH_HEAD").exists(),
            "no note of where it came from"
        );
        assert_eq!(ok(&dst, &["rev-parse", "--abbrev-ref", "HEAD"]), "job-a2");
        assert_eq!(ok(&dst, &["rev-parse", "HEAD"]), f.step);
        assert_eq!(ok(&dst, &["status", "--porcelain"]), "");
        std::fs::remove_dir_all(&f.dir).unwrap();
    }

    #[test]
    fn a_restored_checkout_still_reaches_its_remote() {
        let f = fixture("remote");
        let dst = restore(&f);
        assert_eq!(
            ok(&dst, &["remote", "get-url", "origin"]),
            f.dir.join("origin.git").display().to_string()
        );
        assert!(
            ok(&dst, &["config", "--get", "remote.origin.fetch"]).contains("refs/remotes/origin")
        );
        ok(&dst, &["fetch", "-q", "origin"]);
        assert_eq!(
            ok(&dst, &["rev-parse", "refs/remotes/origin/job"]),
            f.later,
            "the remote still holds the source's branch: an outside fact, not hidden"
        );
        std::fs::remove_dir_all(&f.dir).unwrap();
    }

    #[test]
    fn the_scripts_take_their_inputs_as_arguments() {
        assert!(CUT_PY.contains("sys.argv[1], sys.argv[2]"));
        assert!(
            RESTORE_SH.contains(r#"src="$1"; dst="$2"; commit="$3"; branch="$4"; origin="$5""#)
        );
        assert!(VERIFY_SH.contains(r#"dst="$1""#));
        assert!(ROOT_SH.contains(r#"dir="$1""#));
        assert!(DISCARD_PY.contains("sys.argv[1], sys.argv[2]"));
        assert!(PROBE_PY.contains("archive_dropped"));
        assert!(PROBE_PY.contains("_session_turn_lease_key"));
        assert!(PROBE_PY.contains("session_turn_leases"));
        assert!(SEED_PY.contains("sys.argv[1]") && SEED_PY.contains(SEED_CUT_CALL));
        assert!(IDLE_PY.contains("sys.argv[1]") && IDLE_PY.contains("session_turn_leases"));
    }
}
