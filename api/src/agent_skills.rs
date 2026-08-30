// Agent skills as files — the way Hermes actually consumes them. Every agent
// mounts two skill roots read-only, both Talaria-owned:
//   /opt/skills       ← <fleet>/skills                (shared across the fleet)
//   /opt/dept-skills  ← <fleet>/agents/<slug>/skills  (the agent's own)
// Hermes reads skills per invocation, so edits here are live — no restart.
// Each skill is a directory holding a SKILL.md (plus optional support files).
//
// Port of the WRITE half of ui/src/server/agent-skills.ts: ownerRoot,
// safeJoin, and writeSkill — the pieces the agent-hire run seeds a fresh
// agent's skills with. The read/mutation surface (listAllSkills, readSkill,
// delete/rename/copy, the Summarizer queue) serves from TS still and crosses
// with the skills routes that own it.

use std::path::PathBuf;
use std::sync::OnceLock;

use sqlx::PgPool;

use crate::fleet_layout;
use crate::internal_history::snapshot;

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
    fleet_layout::fleet_dir().join("skills")
}

/// An agent's own root, mounted at /opt/dept-skills.
pub fn agent_skill_root(slug: &str) -> PathBuf {
    fleet_layout::fleet_dir()
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
/// the content changed. Best-effort history, like TS's `.catch(() => {})` —
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
        let fleet = fleet_layout::fleet_dir();
        assert_eq!(shared_skill_root(), fleet.join("skills"));
        assert_eq!(
            agent_skill_root("analyst"),
            fleet.join("agents").join("analyst").join("skills")
        );
    }
}
