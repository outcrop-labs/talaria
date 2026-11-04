// Acknowledging the turns a Hermes session lost across a non-graceful agent
// restart (an OOM, `docker kill`, a host crash). v0.22 marks such a turn
// `unknown` on the next boot and holds every later turn of that session
// behind it until someone says what became of it. The session arm notices
// (talaria-gateway `fleet_lost_turns`) and calls this through its
// RESOLVE_LOST_TURNS seam.
//
// It runs `hermes sessions discard <session> --yes` inside the agent's own
// container, as the agent's own user. That is the non-interactive form of
// Hermes' `/discard` and its Desktop's Discard button: the same fenced
// `prompt.resolve_unknown`, against the gateway that owns the session. The
// lost input is not replayed. Hermes closes the turn with a boundary row
// telling the agent it did not complete. The HTTP door Hermes offers for the
// same thing (`POST /v1/runs/{id}/resolve-unknown`) cannot be used here: it
// answers 404 for a session-stream turn once the container has restarted.

use sqlx::PgPool;

use crate::{docker_exec_as, managed_container};

/// The image's runtime user, which owns the state volume. The CLI reaches the
/// gateway that owns the session through files under that volume.
const AGENT_USER: &str = "hermes";

/// How long the discard may take. Measured at about 4.5 s on v0.22.4 (a CLI
/// start plus one gateway round trip); the bound is for a container that has
/// stopped answering.
const DISCARD_TIMEOUT_MS: u64 = 30_000;

/// Acknowledge every lost (`unknown`) turn of one session in the managed
/// agent that serves `model`. On success, the answer is what Hermes printed.
pub async fn discard_lost_turns(
    pg: &PgPool,
    model: &str,
    session_id: &str,
) -> Result<String, String> {
    if !plain_session_id(session_id) {
        return Err(format!("{session_id:?} is not a session id this can name"));
    }
    let department: Option<(String,)> =
        sqlx::query_as("select department from agent_defs where model = $1 and managed")
            .bind(model)
            .fetch_optional(pg)
            .await
            .map_err(|e| format!("could not look up {model}: {e}"))?;
    let Some((department,)) = department else {
        return Err(format!(
            "{model} is not a managed agent, so nothing here can reach its container"
        ));
    };
    let container = managed_container(pg, &department).await;
    let (out, _) = docker_exec_as(
        &container,
        Some(AGENT_USER),
        &discard_command(session_id),
        None,
        DISCARD_TIMEOUT_MS,
    )
    .await
    .map_err(|e| format!("hermes sessions discard in {container} failed: {e}"))?;
    Ok(out.trim().to_string())
}

/// The argv, one element per word: no shell ever sees the id.
fn discard_command(session_id: &str) -> [&str; 5] {
    ["hermes", "sessions", "discard", session_id, "--yes"]
}

/// Session ids Talaria keys sessions by — a conversation's uuid, a work
/// session's run id. Plain characters only, and never something the CLI
/// would read as a flag.
fn plain_session_id(id: &str) -> bool {
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
    fn the_discard_names_the_session_and_never_asks() {
        assert_eq!(
            discard_command("f48a3937-c96c-4907-b8b7-2618a91d1a1e"),
            [
                "hermes",
                "sessions",
                "discard",
                "f48a3937-c96c-4907-b8b7-2618a91d1a1e",
                "--yes"
            ]
        );
        assert_eq!(AGENT_USER, "hermes");
    }

    #[test]
    fn only_plain_session_ids_reach_the_command() {
        assert!(plain_session_id("f48a3937-c96c-4907-b8b7-2618a91d1a1e"));
        assert!(plain_session_id("run_2f8c74166c6e469297219bdf64bfb1b5"));
        for bad in ["", "--all", "-x", "a b", "a;b", "a/b", &"x".repeat(129)] {
            assert!(!plain_session_id(bad), "{bad:?}");
        }
    }
}
