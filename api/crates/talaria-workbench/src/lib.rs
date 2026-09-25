// The workbench engine: the Developer Agent switch and the sandbox it turns on.
//
// The per-agent control is ONE boolean on agent_defs:
//   developer: true | false
// On means everything a coding agent needs, with nothing left to wire by hand:
// the dev sandbox overlay (below), Oh My Pi as the harness
// (talaria-workbench-harnesses), and the Workbench MCP tools. The registry
// grants the `workbench` server from this flag and from nothing else, so the
// switch and the tools cannot drift apart. Which repos the agent may touch
// stays an explicit grant (workbench_repos); the switch never picks for you.

use serde_json::{Map, Value, json};
use sqlx::PgPool;

/// The sandbox's name on the wire and in the container env.
pub const SANDBOX: &str = "dev";

/// Extra container env the sandbox carries: the marker the fleet skills and
/// the credential helper key on.
pub fn sandbox_env() -> Map<String, Value> {
    let mut env = Map::new();
    env.insert("TALARIA_WORKBENCH".into(), json!(SANDBOX));
    env.insert("TALARIA_WORKBENCH_PROFILE".into(), json!(SANDBOX));
    env
}

/// The container image Developer Agents run instead of the chassis image,
/// when the operator sets `TALARIA_WORKBENCH_IMAGE` (e.g. the pre-baked
/// `talaria-workbench:latest` from scripts/build-workbench-image.sh). Unset or
/// blank keeps the chassis image, and omp installs through `npx` on first use.
pub fn sandbox_image() -> Option<String> {
    std::env::var("TALARIA_WORKBENCH_IMAGE")
        .ok()
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty())
}

/// Flip the switch (fleet.defs.$id PATCH). Returns whether the stored value
/// changed, so the caller rolls the agent only when there is something to
/// apply.
pub async fn set_developer(pg: &PgPool, id: &str, on: bool) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "update agent_defs set developer = $1, updated_at = now() \
         where id = $2::uuid and developer is distinct from $1",
    )
    .bind(on)
    .bind(id)
    .execute(pg)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_env_marks_the_dev_workbench() {
        let env = sandbox_env();
        assert_eq!(env["TALARIA_WORKBENCH"], json!("dev"));
        assert_eq!(env["TALARIA_WORKBENCH_PROFILE"], json!("dev"));
    }
}
