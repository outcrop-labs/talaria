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
    /// A person stopped it through Talaria (Stop) and nobody has started it
    /// since: the only stop the channel trusts. A container that is down
    /// without this record was left down by something else, and is started.
    pub stopped: bool,
}

/// What an agent's work comes to, asked before a roll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Busy {
    Idle,
    /// It is working or about to, and why.
    Working(String),
    /// Whether it is working cannot be read, and why: not restarted now —
    /// unless the rollout is an admin's Roll now or Pin.
    Unknown(String),
}

/// Why something failed, and whose fault it is: only an [`Fault::Image`]
/// failure counts toward marking an image bad (and only once it reproduces);
/// an [`Fault::Infra`] one — docker slow or refusing, the host's disk
/// saturated, a registry away, the api unable to reach a container that
/// docker calls healthy — is tried again later and never bans. A
/// [`Fault::Held`] one is neither: the image works, but a Hermes internal
/// Talaria relies on changed, so it waits for an admin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    Image(String),
    Infra(String),
    Held(Vec<String>),
}

impl Fault {
    pub fn text(&self) -> String {
        match self {
            Fault::Image(s) | Fault::Infra(s) => s.clone(),
            Fault::Held(problems) => problems.join("; "),
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
    /// When its process last started (docker's `State.StartedAt`): a
    /// restart by the daemon (a reboot, a docker restart) moves it without
    /// moving the restart count.
    pub started_at: String,
}

impl Snapshot {
    /// A person stopped it — Talaria recorded their Stop — and it is down or
    /// gone: it is assigned and starts on the image when they start it,
    /// never started by the channel.
    pub fn person_stopped(&self, agent: &ChannelAgent) -> bool {
        agent.stopped
            && (!self.exists || matches!(self.status.as_str(), "exited" | "paused" | "created"))
    }

    /// Docker says it crashed: restarting (a crash loop under its restart
    /// policy) or dead.
    pub fn crashed(&self) -> bool {
        matches!(self.status.as_str(), "restarting" | "dead")
    }

    /// Meant to run and not running, so the channel starts it: crashed, or
    /// down after the channel itself recreated it (`touched`: a roll or a
    /// put-back that did not finish) — never with a person's Stop recorded.
    /// It is recreated whatever it is doing; it cannot be doing anything.
    pub fn down_unwillingly(&self, agent: &ChannelAgent, touched: bool) -> bool {
        !self.running
            && self.status != "removing"
            && !self.person_stopped(agent)
            && (self.crashed() || touched)
    }

    /// Down, and not the channel's to start: stopped by someone — Talaria's
    /// Stop, `docker stop`, `compose stop` — removed (`compose down`), or
    /// left created. It is assigned and starts on the image when someone
    /// starts it. (Under a chassis with `restart: no`, a crash also leaves a
    /// container exited, and reads so: the stock chassis restarts a crash.)
    pub fn left_down(&self, agent: &ChannelAgent, touched: bool) -> bool {
        !self.running && self.status != "removing" && !self.down_unwillingly(agent, touched)
    }
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
    /// Pull (with `need` for the disk's room) and inspect.
    fn prepare<'a>(
        &'a self,
        record: &'a ImageRecord,
        need: prepare::Room,
    ) -> BoxFuture<'a, prepare::Prepared>;
    /// Boot the canary (its config shaped from the first agent's), soak it,
    /// take it down.
    fn canary<'a>(&'a self, record: &'a ImageRecord) -> BoxFuture<'a, Result<String, Fault>>;
    /// The agent's container.
    fn snapshot<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<Snapshot, String>>;
    /// The id an image reference resolves to on this host.
    fn image_id<'a>(&'a self, image: &'a str) -> BoxFuture<'a, Result<String, String>>;
    /// The `repository@sha256:…` an image on this host was pulled as (its
    /// first repo digest, the channel's repository first), or None for one
    /// built here and never pulled.
    fn repo_digest<'a>(&'a self, image_id: &'a str) -> BoxFuture<'a, Option<String>>;
    /// Record that the agent was stopped (a stop the channel found, made
    /// before it ran or outside Talaria's Stop), so it stays stopped.
    fn record_stop<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<(), String>>;
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
    /// are in place. Not reaching it at all is the host's fault (docker
    /// already said it is healthy); an answer that is wrong is the image's.
    fn answers<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<(), Fault>>;
    /// The config or migration fault lines in its container's log, normalized.
    fn log_faults<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Vec<String>>;
    /// What the agent's work comes to. `lenient`: it runs an image this
    /// build never inspected (an older Hermes, an operator's), so a health
    /// answer without the fields Talaria reads is judged by what it has.
    fn busy<'a>(
        &'a self,
        agent: &'a ChannelAgent,
        lenient: bool,
    ) -> BoxFuture<'a, Result<Busy, String>>;
    /// Whether the window is open now, and the zone it is read in (with
    /// where the zone came from).
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
/// the operator's workbench image (`TALARIA_WORKBENCH_IMAGE`) does not. The
/// render's own rule, not a copy of it.
pub fn on_channel(agent: &ChannelAgent) -> bool {
    talaria_fleet_render::runs_planned_image(agent.developer)
}

type AgentRow = (String, String, String, String, String, bool, bool);

/// Every managed, enabled agent that runs in its department's container
/// (the render's runner), oldest first.
pub async fn channel_agents(pg: &PgPool) -> Result<Vec<ChannelAgent>, sqlx::Error> {
    let rows: Vec<AgentRow> = sqlx::query_as(
        "select id::text, slug, department, model, display_name, developer, \
           stopped_at is not null from ( \
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
            |(id, slug, department, model, display_name, developer, stopped)| ChannelAgent {
                id,
                slug,
                department,
                model,
                display_name,
                developer,
                stopped,
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

impl DockerOps {
    /// The first agent on the channel: the one whose rendered config shapes
    /// the canary's and the inspection's.
    async fn first_agent(&self) -> Option<ChannelAgent> {
        channel_agents(&self.pg)
            .await
            .ok()?
            .into_iter()
            .find(on_channel)
    }
}

/// An agent's rendered config.yaml, as text.
async fn base_config(agent: &ChannelAgent) -> Option<String> {
    tokio::fs::read_to_string(
        talaria_fleet_layout::fleet_dir()
            .join("agents")
            .join(&agent.slug)
            .join("config.yaml"),
    )
    .await
    .ok()
}

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

    fn prepare<'a>(
        &'a self,
        record: &'a ImageRecord,
        need: prepare::Room,
    ) -> BoxFuture<'a, prepare::Prepared> {
        Box::pin(async move {
            // Every agent's rendered config, for the contract's schema check
            // — all of them, or the check is not made: an agent whose config
            // cannot be read is not checked by default.
            let agents = match channel_agents(&self.pg).await {
                Ok(a) => a,
                Err(e) => {
                    return prepare::Prepared::Failed {
                        why: format!("the agents could not be read for the contract check: {e}"),
                        definitive: false,
                    };
                }
            };
            let mut configs = Vec::new();
            for a in agents.iter().filter(|a| on_channel(a)) {
                match base_config(a).await {
                    Some(text) => configs.push((a.slug.clone(), text)),
                    None => {
                        return prepare::Prepared::Failed {
                            why: format!(
                                "{}'s rendered config could not be read for the contract check",
                                a.display_name
                            ),
                            definitive: false,
                        };
                    }
                }
            }
            prepare::prepare(&self.pg, record, &configs, need).await
        })
    }

    fn canary<'a>(&'a self, record: &'a ImageRecord) -> BoxFuture<'a, Result<String, Fault>> {
        Box::pin(async move {
            let (base, known) = match self.first_agent().await {
                Some(a) => {
                    let container =
                        talaria_fleet_docker::managed_container(&self.pg, &a.department).await;
                    (
                        base_config(&a).await,
                        probe::log_faults(&probe::logs(&container, 2000).await),
                    )
                }
                None => (None, Vec::new()),
            };
            probe::run_canary(record, base.as_deref(), &known).await
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

    fn repo_digest<'a>(&'a self, image_id: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let (_, _, repo) = talaria_fleet_render::compiled_pin_parts();
            probe::repo_digest(image_id, repo).await
        })
    }

    fn record_stop<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            sqlx::query(
                "update agent_defs set stopped_at = now() where id = $1::uuid and stopped_at is null",
            )
            .bind(&agent.id)
                .execute(&self.pg)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
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

    fn answers<'a>(&'a self, agent: &'a ChannelAgent) -> BoxFuture<'a, Result<(), Fault>> {
        Box::pin(async move {
            let entry = talaria_fleet_layout::read_manifest()
                .await
                .into_iter()
                .find(|e| e.model == agent.model)
                .ok_or_else(|| {
                    Fault::Infra(
                        "it is not in the fleet manifest, so the gateway cannot dial it".into(),
                    )
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
        lenient: bool,
    ) -> BoxFuture<'a, Result<Busy, String>> {
        Box::pin(async move {
            if talaria_fleet_reconcile::is_rolling(&agent.department) {
                return Ok(Busy::Working("another roll of it is in progress".into()));
            }
            busy::busy(&self.pg, agent, lenient)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn window_open<'a>(&'a self, window: &'a Window) -> BoxFuture<'a, (bool, String)> {
        Box::pin(async move {
            let zone = crate::window_zone(&self.pg).await;
            let m = talaria_tz::local_moment(&zone.zone, talaria_agent_auth::now_ms());
            (window.contains(m.hour * 60 + m.minute), zone.said())
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
