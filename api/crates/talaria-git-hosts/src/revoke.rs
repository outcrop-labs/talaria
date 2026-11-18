// Taking an agent's git tokens back when what it may do narrows.
//
// A sandbox's token lives up to an hour, and each api process keeps the
// tokens it minted in its own cache (talaria-github). Narrowing a grant in
// the database stops the next mint, but not a token already handed out or
// one a cache would serve again. So every narrowing revokes:
//   · an admin takes a permission back from an agent on a host (the editor's
//     write) — every token of that agent on that host
//   · a repo grant goes (the grants editor) — that agent's tokens for that
//     repo
//   · the agent's Developer switch goes off, or it is retired or deleted —
//     every token of that agent, on every host
//
// HOW IT REACHES EVERY PROCESS. The process that made the change revokes what
// its own cache holds, and publishes `{host, agent, repo, why}` on the Redis
// topic `git:token-revoke`, the way logout reaches every process's streams
// (`session:revoked`, talaria-realtime): every api process subscribes at boot
// ([`listen`]) through the process's realtime hub, which resubscribes after a
// reconnect, and does the same with its own cache. A revoke is idempotent: a
// process that holds nothing for the agent does nothing, the publisher
// included when its own message comes back.
//
// WHAT A REVOKE IS. Each token is dropped from the cache, so it is never
// served again, and revoked on GitHub (`DELETE /installation/token`). Every
// process that revoked something writes `github.token_revoked`; one GitHub
// did not confirm writes `github.token_revoke_failed` and logs an ALERT (the
// admin alert reads those rows): that token lives until it expires, within
// the hour. A broadcast published while Redis was unreachable, or while a
// process was reconnecting, is not heard there: its tokens live until they
// expire, and the credential door's re-check still refuses to mint against
// the old grant.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

use talaria_audit::{AuditEntry, log_audit};
use talaria_github::Revocation;
use talaria_realtime::{RealtimeDeps, SubscribeFn};

/// The topic every api process hears revokes on.
pub const TOKEN_REVOKE_TOPIC: &str = "git:token-revoke";

/// One revoke, as it is broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Broadcast {
    /// None: every host.
    pub host: Option<String>,
    pub agent: String,
    /// None: every repo on the host.
    pub repo: Option<String>,
    /// What narrowed, for the audit row and the log.
    pub why: String,
}

/// REVOKE `agent_id`'s tokens — on `host_id` when one is named, else on every
/// host, and for `repo` alone when one is named — here and, through the
/// broadcast, in every other api process. `why` says what narrowed. Answers
/// what this process revoked.
pub async fn revoke_agent_tokens(
    pg: &PgPool,
    rt: &RealtimeDeps,
    host_id: Option<&str>,
    agent_id: &str,
    repo: Option<&str>,
    why: &str,
) -> Revocation {
    let message = Broadcast {
        host: host_id.map(str::to_string),
        agent: agent_id.to_string(),
        repo: repo.map(str::to_string),
        why: why.to_string(),
    };
    (rt.publish)(
        TOKEN_REVOKE_TOPIC,
        &serde_json::to_string(&message).expect("a broadcast serializes"),
    );
    revoke_here(pg, &message).await
}

/// One broadcast, done in this process: the cache's matching tokens revoked,
/// and what came of it recorded.
pub async fn revoke_here(pg: &PgPool, b: &Broadcast) -> Revocation {
    let out =
        talaria_github::revoke_agent_tokens(b.host.as_deref(), &b.agent, b.repo.as_deref()).await;
    if out.total() == 0 {
        return out;
    }
    let after = json!({
        "host": b.host,
        "repo": b.repo,
        "why": b.why,
        "revoked": out.revoked,
        "failed": out.failed,
        "carried": out.keys.to_json(),
    });
    if out.revoked > 0 {
        log_audit(
            pg,
            AuditEntry {
                actor: "talaria",
                action: "github.token_revoked",
                target_type: "agent",
                target_id: Some(&b.agent),
                target_label: None,
                before: None,
                after: Some(after.clone()),
            },
        )
        .await;
    }
    if out.failed > 0 {
        tracing::error!(
            "[git-hosts] ALERT: {} token(s) of agent {} ({}) were dropped but GitHub did not confirm their revoke — they live until they expire, within the hour",
            out.failed,
            b.agent,
            b.why
        );
        log_audit(
            pg,
            AuditEntry {
                actor: "talaria",
                action: "github.token_revoke_failed",
                target_type: "agent",
                target_id: Some(&b.agent),
                target_label: None,
                before: None,
                after: Some(after),
            },
        )
        .await;
    } else {
        tracing::info!(
            "[git-hosts] revoked {} token(s) of agent {} ({})",
            out.revoked,
            b.agent,
            b.why
        );
    }
    out
}

/// A broadcast's payload, read. None for anything else, which is logged and
/// ignored: a revoke nobody can read is one nobody acts on wrongly.
pub fn read_broadcast(payload: &str) -> Option<Broadcast> {
    serde_json::from_str::<Broadcast>(payload)
        .inspect_err(|e| {
            tracing::warn!("[git-hosts] a token revoke broadcast could not be read ({e})");
        })
        .ok()
        .filter(|b| !b.agent.is_empty())
}

/// EVERY API PROCESS RUNS THIS, from boot, for as long as it runs: each
/// revoke another process broadcasts is done here too. The subscription is
/// the process's realtime hub's (`RealtimeDeps::streams_only`), which holds
/// one Redis connection and resubscribes after a reconnect; a subscription
/// that ends (a lagging receiver, a shutdown) is opened again a second
/// later.
pub async fn listen(pg: PgPool, subscribe: SubscribeFn) {
    loop {
        let mut rx = subscribe(TOKEN_REVOKE_TOPIC.to_string()).await;
        while let Some(payload) = rx.recv().await {
            if let Some(b) = read_broadcast(&payload) {
                revoke_here(&pg, &b).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_broadcast_reads_back_as_itself_and_anything_else_is_ignored() {
        let b = Broadcast {
            host: Some("github".into()),
            agent: "a-1".into(),
            repo: None,
            why: "a GitHub permission was taken back".into(),
        };
        let wire = serde_json::to_string(&b).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&wire).unwrap(),
            json!({ "host": "github", "agent": "a-1", "repo": null, "why": b.why })
        );
        assert_eq!(read_broadcast(&wire), Some(b));
        assert_eq!(read_broadcast("not json"), None);
        assert_eq!(read_broadcast(r#"{"agent":"","why":"x"}"#), None);
        // A host and a repo may be left out: every host, every repo.
        assert_eq!(
            read_broadcast(r#"{"agent":"a-2","why":"retired"}"#),
            Some(Broadcast {
                host: None,
                agent: "a-2".into(),
                repo: None,
                why: "retired".into(),
            })
        );
    }

    /// A revoke publishes on the topic every process hears, whether or not
    /// this process held anything for the agent. Nothing held, nothing is
    /// written: the lazy pool is never asked.
    #[tokio::test]
    async fn a_revoke_is_broadcast_to_every_process_even_when_this_one_holds_nothing() {
        let published: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let seen = published.clone();
        let rt = RealtimeDeps {
            publish: Arc::new(move |topic: &str, payload: &str| {
                seen.lock()
                    .unwrap()
                    .push((topic.to_string(), payload.to_string()));
            }),
            subscribe: RealtimeDeps::publish_only(None).subscribe,
        };
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://revoke-test@localhost:1/none")
            .expect("a lazy pool connects to nothing");
        let out = revoke_agent_tokens(
            &pg,
            &rt,
            Some("github"),
            "a-nothing-held",
            Some("acme/widgets"),
            "the repo grant was removed",
        )
        .await;
        assert_eq!(out.total(), 0);
        let published = published.lock().unwrap();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0, TOKEN_REVOKE_TOPIC);
        assert_eq!(
            read_broadcast(&published[0].1),
            Some(Broadcast {
                host: Some("github".into()),
                agent: "a-nothing-held".into(),
                repo: Some("acme/widgets".into()),
                why: "the repo grant was removed".into(),
            })
        );
    }
}
