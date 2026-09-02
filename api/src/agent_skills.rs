// Agent skills as files — the way Hermes actually consumes them. Every agent
// mounts two skill roots read-only, both Talaria-owned:
//   /opt/skills       ← <fleet>/skills                (shared across the fleet)
//   /opt/dept-skills  ← <fleet>/agents/<slug>/skills  (the agent's own)
// Hermes reads skills per invocation, so edits here are live — no restart.
// Each skill is a directory holding a SKILL.md (plus optional support files).
//
// The write half is what the agent-hire run seeds a fresh agent with; the
// read/mutation surface (list, read, delete/rename/copy, the Summarizer
// queue) is what the skills routes serve.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Serialize;
use sha1::{Digest, Sha1};
use sqlx::PgPool;

use crate::internal_history::snapshot;
use crate::state::AppState;

/// Owner key: 'shared' or an agent slug.
pub const SHARED: &str = "shared";

/// Skill names are directory names the container mounts — lowercase, no
/// slashes, cannot look like a path segment ('..' needs a dot first char,
/// which the leading [a-z0-9] refuses).
fn name_ok(name: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z0-9][a-z0-9._-]*$").unwrap())
        .is_match(name)
}

/// The shared root every agent in the fleet mounts at /opt/skills.
pub fn shared_skill_root() -> PathBuf {
    crate::fleet::layout::fleet_dir().join("skills")
}

/// An agent's own root, mounted at /opt/dept-skills.
pub fn agent_skill_root(slug: &str) -> PathBuf {
    crate::fleet::layout::fleet_dir()
        .join("agents")
        .join(slug)
        .join("skills")
}

/// Resolve an owner to its skill root — 'shared' or any ENABLED agent's slug.
/// Disabled agents own nothing addressable (the roster does not show them,
/// and their containers are down).
pub async fn owner_root(pg: &PgPool, owner: &str) -> Result<PathBuf, String> {
    if owner == SHARED {
        return Ok(shared_skill_root());
    }
    let known: Option<(i32,)> =
        sqlx::query_as("select 1 from agent_defs where slug = $1 and enabled")
            .bind(owner)
            .fetch_optional(pg)
            .await
            .map_err(|e| e.to_string())?;
    if known.is_some() {
        return Ok(agent_skill_root(owner));
    }
    Err(format!("unknown owner \"{owner}\""))
}

/// Resolve a skill dir under its owner root, refusing path escapes. The
/// alphabet already bars '/', so the prefix check is belt-and-braces — kept
/// because it is the invariant that makes a mountable path, not the regex.
fn safe_join(root: &std::path::Path, name: &str) -> Result<PathBuf, String> {
    if !name_ok(name) {
        return Err("invalid skill name".into());
    }
    let p = root.join(name);
    if !p.starts_with(root) {
        return Err("invalid skill name".into());
    }
    Ok(p)
}

/// Write (or overwrite) a skill's SKILL.md, snapshotting the revision when
/// the content changed. Best-effort history —
/// a skill a hire cannot snapshot still lands on disk.
pub async fn write_skill(
    pg: &PgPool,
    owner: &str,
    name: &str,
    content: &str,
    author: Option<&str>,
) -> Result<(), String> {
    let root = owner_root(pg, owner).await?;
    let dir = safe_join(&root, name)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("skill dir unwritable ({}): {e}", dir.display()))?;
    let md = dir.join("SKILL.md");
    tokio::fs::write(&md, content)
        .await
        .map_err(|e| format!("SKILL.md unwritable ({}): {e}", md.display()))?;
    let _ = snapshot(pg, "skill", &format!("{owner}/{name}"), content, author).await;
    Ok(())
}

// ── The read half ────────────────────────────────────────────────────────────

/// One owner as the listing shows it.
struct OwnerInfo {
    owner: String,
    label: String,
    root: PathBuf,
    /// 'shared' | 'imported' | 'created'.
    source: String,
    /// The agent's model id (absent for the shared root) — what
    /// user_agent_access grants reference.
    model: Option<String>,
}

async fn owners(pg: &PgPool) -> Result<Vec<OwnerInfo>, sqlx::Error> {
    let defs: Vec<(String, String, String, String)> = sqlx::query_as(
        "select slug, model, display_name, source from agent_defs \
         where enabled order by slug",
    )
    .fetch_all(pg)
    .await?;
    let mut out = vec![OwnerInfo {
        owner: SHARED.to_string(),
        label: "Shared (all agents)".to_string(),
        root: shared_skill_root(),
        source: "shared".to_string(),
        model: None,
    }];
    for (slug, model, display_name, source) in defs {
        out.push(OwnerInfo {
            owner: slug.clone(),
            label: display_name,
            root: agent_skill_root(&slug),
            source,
            model: Some(model),
        });
    }
    Ok(out)
}

/// The owner table including roots — the richer lookup the read/mutation
/// surface needs. Unknown owner is an error.
async fn owner_info(pg: &PgPool, owner: &str) -> Result<OwnerInfo, String> {
    owners(pg)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|o| o.owner == owner)
        .ok_or_else(|| format!("unknown owner \"{owner}\""))
}

/// The agent model behind an owner slug (None for 'shared'/unknown).
pub async fn owner_model(pg: &PgPool, owner: &str) -> Option<String> {
    owners(pg)
        .await
        .ok()?
        .into_iter()
        .find(|o| o.owner == owner)?
        .model
}

/// PLATFORM skills — the canonical set Talaria seeds into the shared root
/// (scripts/skills/*). Essential plumbing like talaria-toolkit: every agent
/// depends on them, so editing/renaming/deleting is locked to admins.
/// A missing directory reads as the empty set.
pub fn platform_skill_names() -> std::io::Result<Vec<String>> {
    let dir = std::env::current_dir()?.join("../scripts/skills");
    let mut names = Vec::new();
    for e in std::fs::read_dir(&dir)? {
        let e = e?;
        if e.file_type()?.is_dir()
            && let Some(n) = e.file_name().to_str()
        {
            names.push(n.to_string());
        }
    }
    Ok(names)
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub name: String,
    /// The Summarizer's line (fallback: first prose line of SKILL.md).
    pub description: String,
    pub files: Vec<String>,
    /// Canonical seeded platform skill (shared root only) — admin-locked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OwnerSkills {
    pub owner: String,
    pub label: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub skills: Vec<SkillSummary>,
}

/// Mechanical fallback while the Summarizer hasn't produced a line yet: the
/// first prose line of SKILL.md, with a frontmatter `description:` winning.
pub fn summarize_fallback(md: &str) -> String {
    for line in md.split('\n') {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with("---") {
            continue;
        }
        // `^description:\s*(.+)$`
        let stripped = t.strip_prefix("description:").map(|rest| rest.trim_start());
        let picked = match stripped {
            // strip_prefix already guarantees the ':' — an EMPTY remainder is
            // `description:` with nothing after, which the picker's grammar
            // does not match (it needs one non-newline char), so only a
            // non-empty remainder wins.
            Some(rest) if !rest.is_empty() => Some(rest.to_string()),
            _ => None,
        };
        let line = picked.unwrap_or_else(|| t.to_string());
        return line.chars().take(160).collect();
    }
    String::new()
}

/// `listAllSkills` — every owner, every skill, one pass. Persistent
/// Summarizer one-liners are keyed to the content hash, so a changed skill
/// serves the mechanical first-line summary once while its regeneration runs
/// in the background — the listing never blocks on a model.
pub async fn list_all_skills(state: &AppState) -> Result<Vec<OwnerSkills>, sqlx::Error> {
    let stored = stored_summaries(&state.pg).await.unwrap_or_default();
    let platform = platform_skill_names().unwrap_or_default();
    let mut out = Vec::new();
    for o in owners(&state.pg).await? {
        // A missing root (an agent with no skills dir yet) reads as empty.
        let entries = match tokio::fs::read_dir(&o.root).await {
            Ok(rd) => rd,
            Err(_) => {
                out.push(OwnerSkills {
                    owner: o.owner,
                    label: o.label,
                    source: o.source,
                    model: o.model,
                    skills: Vec::new(),
                });
                continue;
            }
        };
        let mut skills = Vec::new();
        {
            let mut named: Vec<(String, bool)> = Vec::new();
            let mut it = entries;
            while let Ok(Some(e)) = it.next_entry().await {
                let is_dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                if let Some(n) = e.file_name().to_str() {
                    named.push((n.to_string(), is_dir));
                }
            }
            for (name, is_dir) in named {
                if !is_dir || !name_ok(&name) {
                    continue;
                }
                let dir = o.root.join(&name);
                let files = visible_files(&dir).await;
                let md = tokio::fs::read_to_string(dir.join("SKILL.md"))
                    .await
                    .unwrap_or_default();
                let row = stored.get(&format!("{}/{}", o.owner, name));
                let fresh = row.is_some_and(|r| !md.is_empty() && r.hash == skill_hash(&md));
                if !fresh && !md.trim().is_empty() {
                    queue_summary(state.clone(), o.owner.clone(), name.clone(), md.clone());
                }
                skills.push(SkillSummary {
                    description: if fresh {
                        row.expect("fresh checked row").summary.clone()
                    } else {
                        summarize_fallback(&md)
                    },
                    platform: if o.owner == SHARED && platform.contains(&name) {
                        Some(true)
                    } else {
                        None
                    },
                    name,
                    files,
                });
            }
        }
        // Skill names live in [a-z0-9._-], where
        // byte order and en collation agree on everything but punctuation
        // ties — equivalent for this alphabet.
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        out.push(OwnerSkills {
            owner: o.owner,
            label: o.label,
            source: o.source,
            model: o.model,
            skills,
        });
    }
    Ok(out)
}

/// A skill dir's non-dot files, SORTED — the order the wire promises.
async fn visible_files(dir: &std::path::Path) -> Vec<String> {
    match tokio::fs::read_dir(dir).await {
        Ok(mut rd) => {
            let mut out = Vec::new();
            while let Ok(Some(e)) = rd.next_entry().await {
                if let Some(n) = e.file_name().to_str()
                    && !n.starts_with('.')
                {
                    out.push(n.to_string());
                }
            }
            // SORTED — tokio's read_dir returns raw getdents order, which is
            // not sorted. Skill file names are ASCII, where byte order and
            // collation agree.
            out.sort();
            out
        }
        Err(_) => Vec::new(),
    }
}

/// The SKILL.md body and the dir's visible files. A missing
/// skill dir is an error; a missing SKILL.md is
/// empty content.
pub async fn read_skill(
    pg: &PgPool,
    owner: &str,
    name: &str,
) -> Result<(String, Vec<String>), String> {
    let o = owner_info(pg, owner).await?;
    let dir = safe_join(&o.root, name)?;
    let mut rd = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| format!("{e}"))?;
    let mut files = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        if let Some(n) = e.file_name().to_str()
            && !n.starts_with('.')
        {
            files.push(n.to_string());
        }
    }
    // sorted like visible_files — the order the wire promises.
    files.sort();
    let content = tokio::fs::read_to_string(dir.join("SKILL.md"))
        .await
        .unwrap_or_default();
    Ok((content, files))
}

/// Delete — only remove things that look like a skill dir.
pub async fn delete_skill(pg: &PgPool, owner: &str, name: &str) -> Result<(), String> {
    let o = owner_info(pg, owner).await?;
    let dir = safe_join(&o.root, name)?;
    let st = tokio::fs::metadata(&dir)
        .await
        .map_err(|e| format!("{e}"))?;
    if !st.is_dir() {
        return Err("not a skill".into());
    }
    tokio::fs::remove_dir_all(&dir)
        .await
        .map_err(|e| format!("{e}"))?;
    let _ = drop_summary(pg, owner, name).await;
    Ok(())
}

/// Rename in place (same owner). Refuses to clobber.
pub async fn rename_skill(
    pg: &PgPool,
    owner: &str,
    name: &str,
    to_name: &str,
) -> Result<(), String> {
    let o = owner_info(pg, owner).await?;
    let from = safe_join(&o.root, name)?;
    let to = safe_join(&o.root, to_name)?;
    if tokio::fs::metadata(&to).await.is_ok() {
        return Err(format!("\"{to_name}\" already exists"));
    }
    tokio::fs::rename(&from, &to)
        .await
        .map_err(|e| format!("{e}"))?;
    let _ = move_summary(pg, owner, name, owner, to_name).await;
    Ok(())
}

/// Copy — the whole dir, support files included, to another owner —
/// optionally removing the source (= move, e.g. promote dept → shared).
pub async fn copy_skill(
    pg: &PgPool,
    owner: &str,
    name: &str,
    to_owner: &str,
    to_name: Option<&str>,
    remove_source: bool,
) -> Result<(), String> {
    let src = owner_info(pg, owner).await?;
    let dst = owner_info(pg, to_owner).await?;
    let to_name = to_name.unwrap_or(name);
    let from = safe_join(&src.root, name)?;
    let to = safe_join(&dst.root, to_name)?;
    if tokio::fs::metadata(&to).await.is_ok() {
        return Err(format!("\"{to_name}\" already exists there"));
    }
    tokio::fs::create_dir_all(&dst.root)
        .await
        .map_err(|e| format!("{e}"))?;
    copy_dir_all(&from, &to).await?;
    if remove_source {
        tokio::fs::remove_dir_all(&from)
            .await
            .map_err(|e| format!("{e}"))?;
        let _ = move_summary(pg, owner, name, to_owner, to_name).await;
    }
    Ok(())
}

/// A plain recursive copy: dirs, files, and nothing clever about links.
async fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    tokio::fs::create_dir_all(dst)
        .await
        .map_err(|e| format!("{e}"))?;
    let mut rd = tokio::fs::read_dir(src).await.map_err(|e| format!("{e}"))?;
    while let Some(e) = rd.next_entry().await.map_err(|e| format!("{e}"))? {
        let from = e.path();
        let to = dst.join(e.file_name());
        if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            Box::pin(copy_dir_all(&from, &to)).await?;
        } else {
            tokio::fs::copy(&from, &to)
                .await
                .map_err(|e| format!("{e}"))?;
        }
    }
    Ok(())
}

// ── The Summarizer storage half ───────────────────────────────────────────────
//
// The prompt, the model chain and the one-line extraction live in the
// harness def; this is the content hash, the in-flight dedupe and the upsert.

pub fn skill_hash(md: &str) -> String {
    let mut h = Sha1::new();
    h.update(md.as_bytes());
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub struct StoredSummary {
    pub hash: String,
    pub summary: String,
}

/// All stored summaries in one query — the listing consults this map.
pub async fn stored_summaries(
    pg: &PgPool,
) -> Result<std::collections::HashMap<String, StoredSummary>, sqlx::Error> {
    let rows: Vec<(String, String, String, String)> =
        sqlx::query_as("select owner, name, hash, summary from skill_summaries")
            .fetch_all(pg)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(owner, name, hash, summary)| {
            (format!("{owner}/{name}"), StoredSummary { hash, summary })
        })
        .collect())
}

static IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// Fire-and-forget: regenerate one skill's summary for this content hash.
/// The dedupe set stops the same skill being summarized twice while one call
/// is still out, which happens on every listing — a list call queues a
/// regeneration for every changed skill it walks past.
pub fn queue_summary(state: AppState, owner: String, name: String, md: String) {
    let key = format!("{owner}/{name}");
    {
        let mut set = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        if set.contains(&key) || md.trim().is_empty() {
            return;
        }
        set.insert(key.clone());
    }
    tokio::spawn(async move {
        // `onFailure: 'null'` — no model on the gateway, an unusable reply, a
        // dead endpoint: all land as a null value and nothing is written, so
        // the previously stored summary survives. The runner has already
        // recorded the attempt on a harness_runs row, which is where a
        // failure belongs.
        let ctx = crate::harness::run::RunContext {
            caller: "platform:summarizer".into(),
            ..Default::default()
        };
        let input = serde_json::json!({ "md": md });
        let run = crate::harness::run::run_harness(
            &state,
            &crate::harness::defs::summarizer::summarizer_harness(),
            &input,
            ctx,
        )
        .await;
        if let Ok(result) = run
            && let Some(line) = result.value.as_ref().and_then(|v| v.as_str())
        {
            let hash = skill_hash(&md);
            let _ = sqlx::query(
                "insert into skill_summaries (owner, name, hash, summary) \
                 values ($1, $2, $3, $4) \
                 on conflict (owner, name) do update set hash = $3, summary = $4, updated_at = now()",
            )
            .bind(&owner)
            .bind(&name)
            .bind(&hash)
            .bind(line)
            .execute(&state.pg)
            .await;
        }
        IN_FLIGHT
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
    });
}

/// Housekeeping when skills move or die — the summary follows the file.
pub async fn drop_summary(pg: &PgPool, owner: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from skill_summaries where owner = $1 and name = $2")
        .bind(owner)
        .bind(name)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn move_summary(
    pg: &PgPool,
    owner: &str,
    name: &str,
    to_owner: &str,
    to_name: &str,
) -> Result<(), sqlx::Error> {
    // A unique violation on (to_owner, to_name) — the one way the update
    // fails — falls back to the delete.
    let moved = sqlx::query(
        "update skill_summaries set owner = $3, name = $4 where owner = $1 and name = $2",
    )
    .bind(owner)
    .bind(name)
    .bind(to_owner)
    .bind(to_name)
    .execute(pg)
    .await;
    if moved.is_err() {
        drop_summary(pg, owner, name).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_alphabet_bars_every_path_shape() {
        assert!(name_ok("a"));
        assert!(name_ok("talaria-toolkit"));
        assert!(name_ok("1st-skill"));
        assert!(name_ok("deep.skill.v2"));
        // no uppercase, no slash, no leading dot/dash, no empties
        assert!(!name_ok(""));
        assert!(!name_ok("A"));
        assert!(!name_ok("a/b"));
        assert!(!name_ok(".."));
        assert!(!name_ok(".hidden"));
        assert!(!name_ok("-x"));
        assert!(!name_ok("a b"));
    }

    #[test]
    fn safe_join_lands_under_the_root_and_refuses_the_rest() {
        let root = std::path::Path::new("/fleet/skills");
        assert_eq!(
            safe_join(root, "talaria-toolkit").unwrap(),
            PathBuf::from("/fleet/skills/talaria-toolkit")
        );
        // Every escape shape dies at the alphabet; the prefix check stands
        // behind it for whatever slips a future edit.
        assert_eq!(safe_join(root, "../etc").unwrap_err(), "invalid skill name");
        assert_eq!(safe_join(root, "").unwrap_err(), "invalid skill name");
        assert_eq!(safe_join(root, "a\\b").unwrap_err(), "invalid skill name");
    }

    #[test]
    fn the_two_roots_match_the_container_mounts() {
        let fleet = crate::fleet::layout::fleet_dir();
        assert_eq!(shared_skill_root(), fleet.join("skills"));
        assert_eq!(
            agent_skill_root("analyst"),
            fleet.join("agents").join("analyst").join("skills")
        );
    }
}
