// The container side of a rewind: what runs by docker exec inside the agent's
// managed container, as the agent's own user, following `lost_turns`.
//
// Hermes' HTTP fork copies a whole session and cannot cut it, so the cut goes
// through its session store in-process, the way its own /retry and /undo do.
// That is an internal, not a public API: `probe` checks the symbols before a
// rewind touches anything, and a Hermes that renamed them fails the rewind
// instead of corrupting a transcript. The checkout is restored with git alone.

use serde_json::Value;
use sqlx::PgPool;

use crate::{docker_exec_as, managed_container};

const AGENT_USER: &str = "hermes";
const PYTHON: &str = "/opt/hermes/.venv/bin/python";
const EXEC_TIMEOUT_MS: u64 = 60_000;

/// Fails the import when `replace_messages` cannot archive the dropped suffix:
/// a plain delete would leave nothing to recover and evict the rows from the
/// search index.
const PROBE_PY: &str = r#"
import inspect, sys
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
assert "archive_dropped" in inspect.signature(SessionDB.replace_messages).parameters
assert callable(getattr(SessionDB, "_session_turn_lease_key", None))
print("ok")
"#;

/// argv: session id. Hermes holds a `session_turn_leases` row for the whole
/// of a running turn, keyed by the conversation's lineage root; a session
/// with no unexpired row has no turn running. Prints `idle` or `busy`.
const IDLE_PY: &str = r#"
import sys, time
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
db = SessionDB()
key = db._session_turn_lease_key(sys.argv[1])
with db._read_ctx() as conn:
    row = conn.execute("SELECT expires_at FROM session_turn_leases WHERE conversation_id = ?", (key,)).fetchone()
print("busy" if row is not None and float(row[0]) > time.time() else "idle")
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

/// argv: session id. Archives every row, so a session a failed rewind left
/// under an attempt's id reads as empty to whatever lands on it next.
const CLEAR_PY: &str = r#"
import sys
sys.path.insert(0, "/opt/hermes")
from hermes_state import SessionDB
db = SessionDB()
sid = sys.argv[1]
if db.get_session(sid) and db.get_messages(sid):
    db.replace_messages(sid, [], archive_dropped=True)
print("ok")
"#;

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
/// The snapshot commits hang off `refs/talaria/*`, so that namespace is
/// fetched whole and the row's commit checked out by id: a step whose tree
/// matched the one before it has no ref of its own. A half-made checkout from
/// an earlier try is replaced.
const RESTORE_SH: &str = r#"
set -e
src="$1"; dst="$2"; commit="$3"; branch="$4"; origin="$5"
rm -rf -- "$dst"
mkdir -p -- "$(dirname -- "$dst")"
git clone -q --no-checkout -- "$src" "$dst"
git -C "$dst" fetch -q -- "$src" '+refs/talaria/*:refs/talaria/*'
git -C "$dst" checkout -q -b "$branch" "$commit"
git -C "$dst" remote set-url origin "$origin"
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
    let (out, _) = docker_exec_as(&container, Some(AGENT_USER), command, None, EXEC_TIMEOUT_MS)
        .await
        .map_err(|e| format!("in {container}: {e}"))?;
    Ok(out)
}

/// The symbols the cut needs, and the container answering at all.
pub async fn probe(pg: &PgPool, agent_model: &str) -> Result<(), String> {
    let out = exec(pg, agent_model, &[PYTHON, "-c", PROBE_PY]).await?;
    if out.trim() != "ok" {
        return Err(format!("the rewind probe answered {:?}", out.trim()));
    }
    Ok(())
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

/// Empty the session a failed rewind forked under an attempt's id.
pub async fn clear_session(pg: &PgPool, agent_model: &str, session_id: &str) -> Result<(), String> {
    if !plain(session_id) {
        return Err("the session id is not one this can name".into());
    }
    exec(pg, agent_model, &[PYTHON, "-c", CLEAR_PY, session_id])
        .await
        .map(|_| ())
}

/// Whether the agent's store holds no running turn on `session_id`: the proof
/// a fork will not tear a turn when the turn itself cannot be named.
pub async fn session_idle(
    pg: &PgPool,
    agent_model: &str,
    session_id: &str,
) -> Result<bool, String> {
    if !plain(session_id) {
        return Err("the session id is not one this can name".into());
    }
    let out = exec(pg, agent_model, &[PYTHON, "-c", IDLE_PY, session_id]).await?;
    match out.trim() {
        "idle" => Ok(true),
        "busy" => Ok(false),
        other => Err(format!("the idle probe answered {other:?}")),
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
    if !plain(session_id) || !plain(tool_call_id) {
        return Err("the session id or tool call id is not one this can name".into());
    }
    let out = exec(
        pg,
        agent_model,
        &[PYTHON, "-c", CUT_PY, session_id, tool_call_id],
    )
    .await?;
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
    fn the_scripts_take_their_inputs_as_arguments() {
        assert!(CUT_PY.contains("sys.argv[1], sys.argv[2]"));
        assert!(
            RESTORE_SH.contains(r#"src="$1"; dst="$2"; commit="$3"; branch="$4"; origin="$5""#)
        );
        assert!(VERIFY_SH.contains(r#"dst="$1""#));
        assert!(ROOT_SH.contains(r#"dir="$1""#));
        assert!(CLEAR_PY.contains("sys.argv[1]"));
        assert!(PROBE_PY.contains("archive_dropped"));
        assert!(PROBE_PY.contains("_session_turn_lease_key"));
        assert!(IDLE_PY.contains("sys.argv[1]") && IDLE_PY.contains("session_turn_leases"));
    }
}
