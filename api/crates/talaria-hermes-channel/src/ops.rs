// WHAT THE ENGINE DOES TO THE WORLD — every docker, registry, fleet and
// notification effect behind one trait ([`FleetOps`]), so the state machine
// (engine.rs) runs the same against the real fleet ([`DockerOps`]) and
// against a scripted one in the live-DB tests.

use std::time::Duration;

use futures_util::future::BoxFuture;
use sqlx::PgPool;

use talaria_fleet_reconcile::RollOutcome;
use talaria_hermes_images::{ImageRecord, Window};
use talaria_notify::{
    ActorKind, Coalesce, NotificationInput, NotifyActor, NotifyDeps, NotifyEntity, NotifyMeta,
    add_notification_with, entity,
};
use talaria_secretbox::SecretBox;

use crate::{busy, check, prepare, probe};

/// One agent the channel moves: a department's runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAgent {
    pub id: String,
    pub slug: String,
    pub department: String,
    pub model: String,
    pub display_name: String,
    pub developer: bool,
}

/// Why something failed, and whose fault it is: only an [`Fault::Image`]
/// failure counts toward marking an image bad (and only once it reproduces);
/// an [`Fault::Infra`] one — docker slow or refusing, the host's disk
/// saturated, a registry away — is tried again later and never bans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    Image(String),
    Infra(String),
}

impl Fault {
    pub fn text(&self) -> &str {
        match self {
            Fault::Image(s) | Fault::Infra(s) => s,
        }
    }
}

/// A container as docker describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The container exists at all.
    pub exists: bool,
    pub running: bool,
    /// docker's `State.Status`: running, exited, restarting, created, dead…
    pub status: String,
    /// The health check's status, when the service has one.
    pub health: Option<String>,
    pub restart_count: i64,
    /// The container's id.
    pub container: String,
    /// The id of the image it was created from.
    pub image: String,
}

/// Every effect the engine has, in one place.
pub trait FleetOps: Send + Sync {
    /// The clock (epoch ms).
    fn now_ms(&self) -> i64;
    /// Every managed, enabled department runner.
    fn agents(&self) -> BoxFuture<'_, Result<Vec<ChannelAgent>, String>>;
    /// Why the channel stands aside (HERMES_IMAGE, the chassis), or None.
    fn off_reason(&self) -> BoxFuture<'_, Option<String>>;
    /// Resolve the channel's tag and record the digest.
    fn check(&self) -> BoxFuture<'_, Result<check::Found, String>>;
    /// Resolve a `vX.Y.Z` tag of the channel's repository (Pin a version).
    fn resolve<'a>(&'a self, version: &'a str) -> BoxFuture<'a, Result<check::Found, String>>;
    /// Pull and inspect.
    fn prepare<'a>(&'a self, record: &'a ImageRecord) -> BoxFuture<'a, prepare::Prepared>;
    /// Boot the canary (its config shaped from the first agent's), soak it,
    /// take it down.
    fn canary<'a>(&'a self, record: &'a ImageRecord) -> BoxFuture<'a, Result<String, Fault>>;
    /// The agent's container.
    fn snapshot<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<Snapshot, String>>;
    /// The id an image reference resolves to on this host.
    fn image_id<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<String, String>>;
    /// Recreate the agent on the render (its assignment already written),
    /// waiting out its health. `start_if_down` recreates one that is not
    /// running too — only for putting an agent back.
    fn roll<'a>(
        &'a self,
        agent: &'a ChannelAgent,
        start_if_down: bool,
    ) -> BoxFuture<'a, Result<(), Fault>>;
    /// Render, so a stopped agent starts on its assignment next time.
    fn render<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, ()>;
    /// Its API server answers over the gateway's dial, and its drain tools
    /// are in place.
    fn answers<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<(), String>>;
    /// The config or migration fault lines in its container's log, normalized.
    fn log_faults<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Vec<String>>;
    /// Why the agent is in use, or None.
    fn busy<'a>(&'a self, agent: &'a ChannelAgent)
    -> BoxFuture<'a, Result<Option<String>, String>>;
    /// Whether the window is open now, and the zone it is read in.
    fn window_open<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, (bool, String)>;
    /// Tell every admin. `topic` is the dedupe key's tail; `entity` the digest.
    fn notify<'a>(
        &'a self,
        kind: &'a str,
        topic: &'a str,
        title: &'a str,
        body: &'a str,
    ) -> BoxFuture<'a, ()>;
    /// Remove an image this instance pulled.
    fn remove_image<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// Whether the host's disk is saturated now (a failure then is the
    /// host's, not the image's).
    fn host_stalled(&self) -> bool;
}

// ── The real fleet ──────────────────────────────────────────────────────────

/// The fleet as it is: docker, the reconcile's roll, the registry, the
/// notification system.
pub struct DockerOps {
    pub pg: PgPool,
    pub sb: SecretBox,
    pub notify: NotifyDeps,
}

/// Whether the agent runs the channel's image at all: a Developer Agent on
/// the operator's workbench image (`TALARIA_WORKBENCH_IMAGE`) does not.
pub fn on_channel(agent: &ChannelAgent) -> bool {
    !(agent.developer
        && std::env::var("TALARIA_WORKBENCH_IMAGE").is_ok_and(|v| !v.trim().is_empty()))
}

type AgentRow = (String, String, String, String, String, bool);

/// Every managed, enabled agent that runs in its department's container
/// (the render's runner), oldest first.
pub async fn channel_agents(pg: &PgPool) -> Result<Vec<ChannelAgent>, sqlx::Error> {
    let rows: Vec<AgentRow> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, developer from ( \
           select d.*, row_number() over (partition by d.department order by \
             exists (select 1 from agent_versions v \
               where v.agent_id = d.id and v.version = d.current_version) desc, \
             d.created_at, d.id) as rank \
           from agent_defs d where d.managed and d.enabled \
         ) ranked where rank = 1 order by created_at, slug",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, slug, department, model, display_name, developer)| ChannelAgent {
                id,
                slug,
                department,
                model,
                display_name,
                developer,
            },
        )
        .collect())
}

/// A roll's verdict, sorted: the sentences the reconcile writes for a
/// container that came up and died, or never came up, are the image's;
/// anything else (docker would not describe it, a CLI timeout, a refusal) is
/// the host's.
pub fn classify_roll(why: &str) -> Fault {
    if why.contains("stopped running before it reported healthy")
        || why.contains("did not report healthy within")
    {
        Fault::Image(why.to_string())
    } else {
        Fault::Infra(why.to_string())
    }
}

/// The io pressure line's `avg60` for "some".
pub fn io_pressure_avg60(text: &str) -> Option<f64> {
    text.lines()
        .find(|l| l.starts_with("some"))?
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("avg60="))?
        .parse()
        .ok()
}

/// Above this share of time stalled on IO in the last minute, the host is
/// saturated: measured on these hosts, docker then takes minutes to create a
/// container and Hermes misses its own liveness probes.
const STALLED_IO_AVG60: f64 = 40.0;

impl FleetOps for DockerOps {
    fn now_ms(&self) -> i64 {
        talaria_agent_auth::now_ms()
    }

    fn agents(&self) -> BoxFuture<'_, Result<Vec<ChannelAgent>, String>> {
        Box::pin(async move {
            channel_agents(&self.pg)
                .await
                .map_err(|e| format!("the agents could not be read: {e}"))
        })
    }

    fn off_reason(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move { crate::off_reason().await })
    }

    fn check(&self) -> BoxFuture<'_, Result<check::Found, String>> {
        Box::pin(async move { check::check(&self.pg).await })
    }

    fn resolve<'a>(&'a self, version: &'a str) -> BoxFuture<'a, Result<check::Found, String>> {
        Box::pin(async move { check::resolve_version(&self.pg, version).await })
    }

    fn prepare<'a>(&'a self, record: &'a ImageRecord) -> BoxFuture<'a, prepare::Prepared> {
        Box::pin(async move { prepare::prepare(&self.pg, record).await })
    }

    fn canary<'a>(&'a self, record: &'a ImageRecord) -> BoxFuture<'a, Result<String, Fault>> {
        Box::pin(async move {
            let base = match channel_agents(&self.pg).await {
                Ok(agents) => match agents.iter().find(|a| on_channel(a)) {
                    Some(a) => tokio::fs::read_to_string(
                        talaria_fleet_layout::fleet_dir()
                            .join("agents")
                            .join(&a.slug)
                            .join("config.yaml"),
                    )
                    .await
                    .ok(),
                    None => None,
                },
                Err(_) => None,
            };
            probe::run_canary(record, base.as_deref()).await
        })
    }

    fn snapshot<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<Snapshot, String>> {
        Box::pin(async move {
            let container =
                talaria_fleet_docker::managed_container(&self.pg, &agent.department).await;
            probe::snapshot(&container).await
        })
    }

    fn image_id<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move { probe::image_id(image).await })
    }

    fn roll<'a>(
        &'a self,
        agent: &'a ChannelAgent,
        start_if_down: bool,
    ) -> BoxFuture<'a, Result<(), Fault>> {
        Box::pin(async move {
            let outcome = if start_if_down {
                talaria_fleet_reconcile::roll_agent_back_outcome(
                    &self.pg,
                    &self.sb,
                    &agent.department,
                )
                .await
            } else {
                talaria_fleet_reconcile::roll_running_agent_outcome(
                    &self.pg,
                    &self.sb,
                    &agent.department,
                )
                .await
            };
            match outcome {
                Ok(RollOutcome::Rolled | RollOutcome::LeftAlone) => Ok(()),
                Ok(RollOutcome::StillStarting { late, .. }) => match late.await {
                    Ok(None) => Ok(()),
                    Ok(Some(why)) => Err(classify_roll(&why)),
                    Err(e) => Err(Fault::Infra(format!("its late health wait was lost: {e}"))),
                },
                Ok(RollOutcome::Soft(s)) => Err(classify_roll(&s)),
                Err(e) => Err(Fault::Infra(e)),
            }
        })
    }

    fn render<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Err(e) =
                talaria_fleet_reconcile::apply_agent_change(&self.pg, &self.sb, &agent.department)
                    .await
            {
                tracing::warn!(
                    "[hermes] {}: the render after its move failed: {e}",
                    agent.slug
                );
            }
        })
    }

    fn answers<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let entry = talaria_fleet_layout::read_manifest()
                .await
                .into_iter()
                .find(|e| e.model == agent.model)
                .ok_or_else(|| {
                    "it is not in the fleet manifest, so the gateway cannot dial it".to_string()
                })?;
            probe::api_answers(&entry.url, entry.key.as_deref()).await?;
            let container =
                talaria_fleet_docker::managed_container(&self.pg, &agent.department).await;
            probe::drain_ready(&container).await
        })
    }

    fn log_faults<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            let container =
                talaria_fleet_docker::managed_container(&self.pg, &agent.department).await;
            probe::log_faults(&probe::logs(&container, 2000).await)
        })
    }

    fn busy<'a>(
        &'a self,
        agent: &'a ChannelAgent,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        Box::pin(async move {
            if talaria_fleet_reconcile::is_rolling(&agent.department) {
                return Ok(Some("another roll of it is in progress".into()));
            }
            busy::busy(&self.pg, agent).await.map_err(|e| e.to_string())
        })
    }

    fn window_open<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, (bool, String)> {
        Box::pin(async move {
            let zone = talaria_notify::prefs::workspace_zone(&self.pg).await;
            let m = talaria_tz::local_moment(&zone, talaria_agent_auth::now_ms());
            (window.contains(m.hour * 60 + m.minute), m.zone)
        })
    }

    fn notify<'a>(
        &'a self,
        kind: &'a str,
        topic: &'a str,
        title: &'a str,
        body: &'a str,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let admins: Vec<(String,)> =
                match sqlx::query_as("select id::text from users where role = 'admin'")
                    .fetch_all(&self.pg)
                    .await
                {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!("[hermes] the admins could not be read to notify: {e}");
                        return;
                    }
                };
            let meta = NotifyMeta {
                group_key: Some(talaria_notify::group_key::hermes(topic)),
                coalesce: Coalesce::Realert,
                entity: Some(NotifyEntity::new(entity::HERMES_IMAGE, topic)),
                actor: Some(NotifyActor {
                    kind: ActorKind::System,
                    id: "hermes-channel".into(),
                    label: "Agent image updates".into(),
                }),
                ..Default::default()
            };
            for (admin,) in admins {
                if let Err(e) = add_notification_with(
                    &self.notify,
                    &admin,
                    &NotificationInput {
                        kind,
                        title,
                        body: Some(body),
                        href: Some(crate::HREF),
                    },
                    &meta,
                )
                .await
                {
                    tracing::warn!("[hermes] the notification to {admin} failed: {e}");
                }
            }
        })
    }

    fn remove_image<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            talaria_fleet_docker::docker(&["image", "rm", image], Duration::from_secs(120))
                .await
                .map(|_| ())
        })
    }

    fn host_stalled(&self) -> bool {
        std::fs::read_to_string("/proc/pressure/io")
            .ok()
            .and_then(|t| io_pressure_avg60(&t))
            .is_some_and(|a| a > STALLED_IO_AVG60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_roll_that_came_up_and_died_is_the_images_and_anything_else_the_hosts() {
        assert!(matches!(
            classify_roll(
                "Rel Bot: the recreated container stopped running before it reported healthy — see its logs"
            ),
            Fault::Image(_)
        ));
        assert!(matches!(
            classify_roll(
                "department \"x\": the recreated container did not report healthy within 900s of its roll — see its logs"
            ),
            Fault::Image(_)
        ));
        for host in [
            "Rel Bot: docker did not describe the recreated container in the 180s after its roll, so whether it came up is unknown",
            "docker compose … timed out after 120s",
            "department \"x\" is already mid-roll — wait for it to finish",
            "could not pull the image for agent-x: …",
        ] {
            assert!(matches!(classify_roll(host), Fault::Infra(_)), "{host}");
        }
    }

    #[test]
    fn the_hosts_io_pressure_is_read_from_its_avg60() {
        let t = "some avg10=79.43 avg60=67.54 avg300=61.40 total=55407754642\nfull avg10=1 avg60=2 avg300=3 total=4\n";
        assert_eq!(io_pressure_avg60(t), Some(67.54));
        assert_eq!(io_pressure_avg60("garbage"), None);
    }
}
