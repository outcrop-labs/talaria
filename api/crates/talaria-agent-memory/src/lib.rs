// Agent memory lives INSIDE each agent's state volume (/opt/data/memories/
// in the hermes-<dept> volume) — the agent curates it itself at runtime.
// Talaria reads and writes it through the running managed container (docker
// exec), so there's no second copy to drift. Requires the container to be up.
//
// TWO FILES LIVE THERE, and they are different things. MEMORY.md is what the
// agent has learned and chosen to keep; USER.md is its profile of the person
// it works for. Hermes writes both with its own memory tool and reads both
// into every turn, so a human editor has to be able to reach either —
// otherwise the only way to fix a wrong fact about yourself is to ask the
// agent to believe you. One door, one `file` argument, because the mechanics
// (exec into the container, snapshot the result) are identical and a second
// copy of them would drift.
//
// BOTH FILES ARE §-DELIMITED LISTS. Hermes' memory tool refuses every write
// if the file it reads back is not a clean "\n§\n"-joined list of entries, so
// a writer that reflows the file silently costs the agent its ability to
// remember anything. Nothing here reflows: the content a caller hands over is
// written byte for byte, and keeping that shape is the editor's job.

use sqlx::PgPool;

use talaria_fleet_docker::{docker_exec_opt, managed_container};
use talaria_internal_history::snapshot;

const MEMORY_PATH: &str = "/opt/data/memories/MEMORY.md";
const PROFILE_PATH: &str = "/opt/data/memories/USER.md";

/// Which of the agent's two memory files a call is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryFile {
    /// MEMORY.md — what the agent has learned and kept.
    #[default]
    Memory,
    /// USER.md — the agent's profile of the person it works for.
    Profile,
}

impl MemoryFile {
    /// The `?file=` vocabulary, closed on purpose: an unknown value is a
    /// caller's bug, and the route answers it rather than guessing a file.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "memory" => Some(Self::Memory),
            "profile" => Some(Self::Profile),
            _ => None,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Memory => MEMORY_PATH,
            Self::Profile => PROFILE_PATH,
        }
    }

    /// The internal-history kind this file snapshots under. The two keep
    /// SEPARATE histories: a revision list that mixed them would offer to
    /// restore a profile over a memory.
    pub fn history_kind(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Profile => "profile",
        }
    }
}

async fn department_for(pg: &PgPool, def_id: &str) -> Result<(String, String), String> {
    let row: Option<(String, String)> = sqlx::query_as(
        "select department, display_name from agent_defs where id = $1::uuid and managed",
    )
    .bind(def_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    row.ok_or_else(|| "not a managed agent".to_string())
}

pub async fn read_memory(
    pg: &PgPool,
    def_id: &str,
    file: MemoryFile,
) -> Result<(String, String), String> {
    let (department, _) = department_for(pg, def_id).await?;
    let name = managed_container(pg, &department).await;
    let content = match docker_exec_opt(&name, &["cat", file.path()], None, 20_000).await {
        Ok((stdout, _)) => stdout,
        // A missing file reads as empty memory — the agent has simply never
        // written any. Every other failure is a failure.
        Err(e) if e.to_lowercase().contains("no such file") => String::new(),
        Err(e) => return Err(format!("cannot read memory from {name}: {e}")),
    };
    Ok((content, name))
}

/// Whole-file replace. The agent also writes this file (with a lockfile) — a
/// concurrent agent write can race a human edit; last writer wins. Every save
/// is snapshotted so any prior memory is recoverable.
pub async fn write_memory(
    pg: &PgPool,
    def_id: &str,
    content: &str,
    author: Option<&str>,
    file: MemoryFile,
) -> Result<(), String> {
    let (department, _) = department_for(pg, def_id).await?;
    let name = managed_container(pg, &department).await;
    let write = format!("cat > {}", file.path());
    docker_exec_opt(&name, &["sh", "-c", &write], Some(content), 20_000)
        .await
        .map_err(|e| format!("cannot write memory in {name}: {e}"))?;
    let _ = snapshot(pg, file.history_kind(), def_id, content, author).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocabulary is closed, and the default is the memory itself: an
    /// older client that sends no `file` at all still edits MEMORY.md.
    #[test]
    fn the_file_vocabulary_is_closed() {
        assert_eq!(MemoryFile::parse("memory"), Some(MemoryFile::Memory));
        assert_eq!(MemoryFile::parse("profile"), Some(MemoryFile::Profile));
        assert_eq!(MemoryFile::parse("MEMORY"), None);
        assert_eq!(MemoryFile::parse("user"), None);
        assert_eq!(MemoryFile::parse(""), None);
        assert_eq!(MemoryFile::default(), MemoryFile::Memory);
    }

    /// Two files, two paths, two histories — nothing shared but the door.
    #[test]
    fn each_file_has_its_own_path_and_history() {
        assert_eq!(MemoryFile::Memory.path(), "/opt/data/memories/MEMORY.md");
        assert_eq!(MemoryFile::Profile.path(), "/opt/data/memories/USER.md");
        assert_eq!(MemoryFile::Memory.history_kind(), "memory");
        assert_eq!(MemoryFile::Profile.history_kind(), "profile");
    }
}
