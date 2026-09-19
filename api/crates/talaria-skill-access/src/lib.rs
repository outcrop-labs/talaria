// Who may tailor which skills. The Studio principle: you can shape the
// agents you've been explicitly granted, admins and agents.manage shape the
// fleet. "May use by default" (a member with NO user_agent_access rows) does
// NOT imply "may rewrite how it works" — tailoring rights are explicit.

use sqlx::PgPool;

use talaria_agent_skills::{SHARED, owner_model, platform_skill_names};
use talaria_fleet_agents::allowed_agents;
use talaria_permissions::has_perm;

use futures_util::future::BoxFuture;
use std::sync::{Arc, OnceLock};

/// Wired from the api binary so this crate does not depend on personal_agent.rs.
pub static OWNS_AGENT: OnceLock<
    Arc<dyn Fn(sqlx::PgPool, String, String) -> BoxFuture<'static, bool> + Send + Sync>,
> = OnceLock::new();

pub async fn can_edit_skills(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    owner: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    if has_perm(pg, user_id, role, "agents.manage").await? {
        return Ok(true);
    }
    if owner == SHARED {
        return Ok(false); // fleet-wide flow changes need agents.manage
    }
    if let Some(f) = OWNS_AGENT.get()
        && f(pg.clone(), user_id.to_string(), owner.to_string()).await
    {
        return Ok(true); // your personal assistant
    }
    let Some(model) = owner_model(pg, owner).await else {
        return Ok(false);
    };
    let access = allowed_agents(pg, user_id, role).await?;
    // Only an EXPLICIT grant confers tailoring — 'all' is the unrestricted-use
    // default, not a statement of trust over every agent's behavior.
    Ok(match access {
        talaria_fleet_agents::AgentAccess::All => false,
        talaria_fleet_agents::AgentAccess::List(list) => list.contains(&model),
    })
}

/// Per-skill check: PLATFORM skills (the canonical seeded set in the shared
/// root — talaria-toolkit and friends) are essential plumbing and stay
/// admin-only no matter what grants a member holds.
pub async fn can_edit_skill(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    owner: &str,
    name: &str,
) -> Result<bool, sqlx::Error> {
    if owner == SHARED
        && role != "admin"
        && platform_skill_names()
            .unwrap_or_default()
            .iter()
            .any(|n| n == name)
    {
        return Ok(false);
    }
    can_edit_skills(pg, user_id, role, owner).await
}
