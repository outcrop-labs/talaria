// Who may tailor which skills. The Studio principle: you can shape the
// agents you manage, and fleet-wide flow changes need `agents.manage`.
//
// This used to answer "has agents.manage, or holds an explicit
// user_agent_access grant for that agent" — both of which say what a person
// may USE, not what is theirs to change. An agent's skills are part of the
// agent, so the question is the same one every other per-agent surface asks:
// are you one of its managers (docs/PERMISSIONS.md, "Agent managers")?
//
// It also carried an `OWNS_AGENT` OnceLock, meant to let a personal
// assistant's human edit its skills. Nothing ever set it, so that arm never
// once answered true; `manages_agent_slug` reads the owner column directly
// and needs no wiring.

use sqlx::PgPool;

use talaria_agent_managers::manages_agent_slug;
use talaria_agent_skills::{SHARED, platform_skill_names};
use talaria_permissions::has_perm;

pub async fn can_edit_skills(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    owner: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    if owner == SHARED {
        // The shared root is the fleet's own flow, not any one agent's.
        return has_perm(pg, user_id, role, "agents.manage").await;
    }
    manages_agent_slug(pg, user_id, role, owner).await
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
