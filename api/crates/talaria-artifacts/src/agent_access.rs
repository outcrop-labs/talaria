// WHAT AN AGENT MAY DO WITH ONE ARTIFACT — read it, rewrite its body — said
// once, for every door that asks.
//
// EVERY DOOR THAT ASKS IT. The artifact GET answers an agent's read and the
// LIST filters by it (`agent_readable`); the PUT and `/{id}/edit` gate an
// agent's write (`body_editor`); the Office and Google exports gate an agent's
// export on the read, and the Google export records the new Drive file only
// for an agent that may edit; and the agent door that opens a file in a
// person's Work pane tells the agent whether it can read and edit what it just
// opened. Each used to state the rule inline, and the pane door's copy would
// have lived in another route crate — which is how two planes come to disagree
// about one document. The rule lives here, below every route crate, and the
// routes keep only their own error wording.
//
// The predicates themselves are talaria-kb-perms' pure checks; what this adds is
// the reads they need — the personal assistant's owner, the agent's teams,
// whether an admin elevated it — so no caller can forget one.

use std::collections::HashSet;

use sqlx::PgPool;
use talaria_agent_auth::{AgentCaller, AgentSubject};
use talaria_kb::perms::{
    EditorGrant, Guarded, ITEM_ARTIFACT, can_edit_agent, can_read_agent, granted_item_ids_for_agent,
};

use crate::ArtifactHead;

/// May this agent read the artifact? Org or public visibility, a grant on a
/// private one (to the agent or a team it is on), or — for a personal
/// assistant — its owner's own artifact (`can_read_agent`'s owner arm). A
/// legacy shared-key caller is never an assistant here: `assistant_owner_for`
/// demands a proven subject.
pub async fn agent_reads_artifact(
    pg: &PgPool,
    agent: &AgentCaller,
    artifact: &Guarded,
    editors: &[EditorGrant],
) -> Result<bool, sqlx::Error> {
    let owner =
        talaria_users::assistant_owner_for(pg, &AgentSubject::Caller(agent.clone())).await?;
    let team_ids = talaria_teams::team_ids_for_agent(pg, &agent.model).await?;
    Ok(can_read_agent(
        artifact,
        &agent.model,
        owner.as_deref(),
        editors,
        &team_ids,
    ))
}

/// `agent_reads_artifact` for a whole list at once: the ids among `heads` this
/// agent may read. The grant leg is one read for every artifact
/// (`granted_item_ids_for_agent` expands team grants the way `team_agents`
/// does), so a session's hundred files cost three queries, not two hundred —
/// and it is `can_read_agent` that decides, as for one.
pub async fn agent_readable(
    pg: &PgPool,
    agent: &AgentCaller,
    heads: &[ArtifactHead],
) -> Result<HashSet<String>, sqlx::Error> {
    if heads.is_empty() {
        return Ok(HashSet::new());
    }
    let owner =
        talaria_users::assistant_owner_for(pg, &AgentSubject::Caller(agent.clone())).await?;
    let granted = if heads.iter().any(|h| h.guarded.visibility == "private") {
        granted_item_ids_for_agent(pg, ITEM_ARTIFACT, &agent.model).await?
    } else {
        HashSet::new()
    };
    Ok(heads
        .iter()
        .filter(|h| {
            granted.contains(&h.id)
                || can_read_agent(&h.guarded, &agent.model, owner.as_deref(), &[], &[])
        })
        .map(|h| h.id.clone())
        .collect())
}

/// May this agent rewrite the artifact's body? Only with an editor grant (to
/// the agent or a team it is on) — agents never get implicit edit rights — or
/// as an admin-elevated assistant on an artifact that is not private.
pub async fn agent_edits_artifact(
    pg: &PgPool,
    agent: &AgentCaller,
    artifact: &Guarded,
    editors: &[EditorGrant],
) -> Result<bool, sqlx::Error> {
    let team_ids = talaria_teams::team_ids_for_agent(pg, &agent.model).await?;
    if can_edit_agent(&agent.model, editors, &team_ids) {
        return Ok(true);
    }
    if artifact.visibility == "private" {
        return Ok(false);
    }
    talaria_users::is_elevated_assistant(pg, &AgentSubject::Caller(agent.clone())).await
}
