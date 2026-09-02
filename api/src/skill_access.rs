// Who may tailor which skills. The Studio principle: you can shape the
// agents you've been explicitly granted, admins and agents.manage shape the
// fleet. "May use by default" (a member with NO user_agent_access rows) does
// NOT imply "may rewrite how it works" — tailoring rights are explicit.

use sqlx::PgPool;

use crate::agent_skills::{SHARED, owner_model, platform_skill_names};
use crate::fleet::allowed_agents;
use crate::permissions::has_perm;
use crate::personal_agent::owns_agent;

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
    if owns_agent(pg, user_id, Some(owner), None).await {
        return Ok(true); // your personal assistant
    }
    let Some(model) = owner_model(pg, owner).await else {
        return Ok(false);
    };
    let access = allowed_agents(pg, user_id, role).await?;
    // Only an EXPLICIT grant confers tailoring — 'all' is the unrestricted-use
    // default, not a statement of trust over every agent's behavior.
    Ok(match access {
        crate::fleet::AgentAccess::All => false,
        crate::fleet::AgentAccess::List(list) => list.contains(&model),
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
