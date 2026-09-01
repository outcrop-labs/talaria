// Agent memory lives INSIDE each agent's state volume (/opt/data/memories/
// MEMORY.md in the hermes-<dept> volume) — the agent curates it itself at
// runtime. Talaria reads and writes it through the running managed container
// (docker exec), so there's no second copy to drift. Requires the container
// to be up.
//
// Port of ui/src/server/agent-memory.ts.

use sqlx::PgPool;

use crate::fleet_docker::{docker_exec_opt, managed_container};
use crate::internal_history::snapshot;

const MEMORY_PATH: &str = "/opt/data/memories/MEMORY.md";

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

pub async fn read_memory(pg: &PgPool, def_id: &str) -> Result<(String, String), String> {
    let (department, _) = department_for(pg, def_id).await?;
    let name = managed_container(pg, &department).await;
    let content = match docker_exec_opt(&name, &["cat", MEMORY_PATH], None, 20_000).await {
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
) -> Result<(), String> {
    let (department, _) = department_for(pg, def_id).await?;
    let name = managed_container(pg, &department).await;
    let write = format!("cat > {MEMORY_PATH}");
    docker_exec_opt(&name, &["sh", "-c", &write], Some(content), 20_000)
        .await
        .map_err(|e| format!("cannot write memory in {name}: {e}"))?;
    let _ = snapshot(pg, "memory", def_id, content, author).await;
    Ok(())
}
