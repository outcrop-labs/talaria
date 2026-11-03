// THE HERMES IMAGES THIS INSTANCE KNOWS, AND WHICH ONE EACH AGENT RUNS.
//
// The agent image follows Hermes' `stable` channel (talaria-hermes-channel
// drives it; docs/ARCHITECTURE.md, "The fleet"). This crate is the part the
// render reads, so it sits below both: the per-digest records
// (`hermes_images`), the per-agent assignments (`hermes_agent_images`), the
// channel's own state (one `app_settings` row), and the plan the render
// stamps — each agent's image ref and the config schema version that image
// reads.
//
// AN AGENT ALWAYS RUNS A RESOLVED DIGEST. A record is written from what a
// registry or the image said, which is untrusted: a digest is kept only as
// `sha256:<64 lowercase hex>`, a version only as `vX.Y.Z`, and a config
// version only as a small integer. Anything else is dropped where it is read,
// never rendered into a compose file.
//
// THE COMPILED PIN IS THE FLOOR. With no record, no assignment and no channel
// state (a fresh install, a host upgraded from a release without the channel,
// a database read that found nothing), every agent renders the pin the
// render was built with, at the config version compiled beside it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// The `app_settings` key the channel's state lives under.
pub const STATE_KEY: &str = "hermes_channel";

/// How many events the state keeps for the panel.
const EVENTS_CAP: usize = 20;

/// Whether a digest ran well on this instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Seen on the channel, not yet rolled out (or mid-rollout).
    Candidate,
    /// Rolled out and healthy here.
    Good,
    /// Failed here; never rolled out again.
    Bad,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Candidate => "candidate",
            Status::Good => "good",
            Status::Bad => "bad",
        }
    }

    /// A status this build does not know (a later release's) reads as bad:
    /// never rolled out, never planned, until a release that knows it says.
    fn parse(s: &str) -> Status {
        match s {
            "good" => Status::Good,
            "candidate" => Status::Candidate,
            _ => Status::Bad,
        }
    }
}

/// One digest's record.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRecord {
    /// `sha256:<64 hex>` — the identity.
    pub digest: String,
    /// The repository it is pulled from (`nousresearch/hermes-agent`).
    pub repository: String,
    /// The `vX.Y.Z` tag that shares the digest, when one was found.
    pub version: Option<String>,
    /// The image's own `DEFAULT_CONFIG["_config_version"]`, read from it.
    pub config_version: Option<u64>,
    pub status: Status,
    /// Why it is bad (or what made it good, for the compiled pin).
    pub reason: Option<String>,
    /// What the inspection read from the image beyond the config version.
    #[serde(skip)]
    pub inspect: serde_json::Value,
    /// This instance pulled it (so this instance may prune it).
    pub pulled_here: bool,
    pub first_seen: i64,
    /// When it was pulled and inspected: only a prepared image is run.
    pub prepared_at: Option<i64>,
    /// Since when it has been good here.
    pub good_since: Option<i64>,
}

impl ImageRecord {
    /// The reference a compose file names: repository, the version tag for
    /// the reader, and the digest docker resolves (a tag beside a digest is
    /// not consulted).
    pub fn image_ref(&self) -> String {
        image_ref(&self.repository, self.version.as_deref(), &self.digest)
    }

    /// Whether the render may run it: pulled, inspected, and its config
    /// version known.
    pub fn runnable(&self) -> bool {
        self.prepared_at.is_some() && self.config_version.is_some()
    }

    /// The env names the image's Hermes strips from every command an agent
    /// runs, as the inspection read them.
    pub fn stripped_env(&self) -> Vec<String> {
        self.inspect
            .get("strippedEnv")
            .and_then(|v| v.as_array())
            .map(|names| {
                names
                    .iter()
                    .filter_map(|n| n.as_str())
                    .filter(|n| is_env_name(n))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The version as a person reads it: the tag, or the digest's head.
    pub fn label(&self) -> String {
        label(self.version.as_deref(), &self.digest)
    }

    /// When this image passed the canary here, if it has.
    pub fn canary_ok_at(&self) -> Option<i64> {
        self.inspect.get("canaryOkAt").and_then(|v| v.as_i64())
    }

    /// What the contract checks found changed in the Hermes internals
    /// Talaria relies on (the inspection's, then the canary's): the image is
    /// held, not banned, until an admin accepts it.
    pub fn contract_problems(&self) -> Vec<String> {
        self.inspect
            .pointer("/contract/problems")
            .and_then(|v| v.as_array())
            .map(|ps| {
                ps.iter()
                    .filter_map(|p| p.as_str())
                    .map(|p| p.chars().take(600).collect())
                    .take(20)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Who accepted the contract problems, and when.
    pub fn contract_accepted(&self) -> Option<(String, i64)> {
        let a = self.inspect.get("contractAccepted")?;
        Some((
            a.get("by")?.as_str()?.chars().take(200).collect(),
            a.get("at")?.as_i64()?,
        ))
    }

    /// Held for an admin: a contract problem nobody accepted.
    pub fn held(&self) -> bool {
        !self.contract_problems().is_empty() && self.contract_accepted().is_none()
    }

    /// Whether an admin may pin it: it ran well here, or it passed the
    /// inspection and the canary here (a Pin rollout of an image that never
    /// ran here still canaries it first). A bad image is never pinnable —
    /// "Allow this version again" first.
    pub fn pinnable(&self) -> bool {
        match self.status {
            Status::Good => true,
            Status::Bad => false,
            Status::Candidate => self.prepared_at.is_some(),
        }
    }
}

/// `nousresearch/hermes-agent:v0.22.6@sha256:…`, or without the tag when no
/// valid one is known.
pub fn image_ref(repository: &str, version: Option<&str>, digest: &str) -> String {
    match version.filter(|v| is_version_tag(v)) {
        Some(v) => format!("{repository}:{v}@{digest}"),
        None => format!("{repository}@{digest}"),
    }
}

/// `v0.22.6`, or `sha256:d27689bc6e85` for a digest with no known tag.
pub fn label(version: Option<&str>, digest: &str) -> String {
    match version.filter(|v| is_version_tag(v)) {
        Some(v) => v.to_string(),
        None => digest.chars().take(19).collect(),
    }
}

/// A content digest as registries spell it: `sha256:` and 64 lowercase hex.
pub fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// A release tag: `v` and three short numbers (`v0.22.6`). Hermes' `main`
/// nightlies, `rc.N-vX.Y.Z` and `-desktop` variants are not.
pub fn is_version_tag(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('v') else {
        return false;
    };
    let parts: Vec<&str> = rest.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 4 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A version tag's numbers, for ordering.
pub fn version_key(tag: &str) -> Option<(u32, u32, u32)> {
    if !is_version_tag(tag) {
        return None;
    }
    let mut it = tag[1..].split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// An env name as Hermes and docker spell one.
pub fn is_env_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// A config schema version an image may declare: Hermes counts up from 1 and
/// sits at 51 for v0.22.4, so anything outside this range is not one.
pub fn is_config_version(n: i64) -> bool {
    (1..=10_000).contains(&n)
}

// ── Records ──────────────────────────────────────────────────────────────────

/// The record's columns, as a literal (sqlx takes only static SQL).
macro_rules! columns {
    () => {
        "digest, repository, version, config_version, status, reason, inspect, \
         pulled_here, (extract(epoch from first_seen) * 1000)::bigint, \
         (extract(epoch from prepared_at) * 1000)::bigint, \
         (extract(epoch from good_since) * 1000)::bigint"
    };
}

type Row = (
    String,
    String,
    Option<String>,
    Option<i32>,
    String,
    Option<String>,
    serde_json::Value,
    bool,
    i64,
    Option<i64>,
    Option<i64>,
);

fn from_row(r: Row) -> ImageRecord {
    ImageRecord {
        digest: r.0,
        repository: r.1,
        version: r.2.filter(|v| is_version_tag(v)),
        config_version: r
            .3
            .map(i64::from)
            .filter(|n| is_config_version(*n))
            .map(|n| n as u64),
        status: Status::parse(&r.4),
        reason: r.5,
        inspect: r.6,
        pulled_here: r.7,
        first_seen: r.8,
        prepared_at: r.9,
        good_since: r.10,
    }
}

/// Every record, newest first.
pub async fn records(pg: &PgPool) -> Result<Vec<ImageRecord>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "select ",
        columns!(),
        " from hermes_images order by first_seen desc, digest"
    ))
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(from_row).collect())
}

/// One digest's record.
pub async fn record(pg: &PgPool, digest: &str) -> Result<Option<ImageRecord>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "select ",
        columns!(),
        " from hermes_images where digest = $1"
    ))
    .bind(digest)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(from_row))
}

/// Note a digest the channel named. Answers whether it was new. A digest
/// seen before keeps its status; its repository and version follow the
/// latest sighting.
pub async fn note_seen(
    pg: &PgPool,
    digest: &str,
    repository: &str,
    version: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let new: (bool,) = sqlx::query_as(
        "insert into hermes_images (digest, repository, version) values ($1, $2, $3) \
         on conflict (digest) do update set repository = excluded.repository, \
           version = coalesce(excluded.version, hermes_images.version), updated_at = now() \
         returning (xmax = 0)",
    )
    .bind(digest)
    .bind(repository)
    .bind(version.filter(|v| is_version_tag(v)))
    .fetch_one(pg)
    .await?;
    Ok(new.0)
}

/// Record an image known good without the channel's say-so: the compiled
/// pin, which every release was checked against by hand. A record that
/// exists is left as it is (a pin this instance later found bad stays bad).
pub async fn seed_known_good(
    pg: &PgPool,
    digest: &str,
    repository: &str,
    version: &str,
    config_version: u64,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into hermes_images (digest, repository, version, config_version, status, \
           reason, prepared_at, good_since) \
         values ($1, $2, $3, $4, 'good', $5, now(), now()) on conflict (digest) do nothing",
    )
    .bind(digest)
    .bind(repository)
    .bind(Some(version).filter(|v| is_version_tag(v)))
    .bind(config_version as i32)
    .bind(reason)
    .execute(pg)
    .await?;
    Ok(())
}

/// Record what the inspection read: the config version, the rest of what
/// the render needs, and whether this instance pulled the image itself.
/// `own_version` is the version the image says it is (`v0.22.6`), kept as
/// the record's version only when no release tag was found to share the
/// digest.
pub async fn set_prepared(
    pg: &PgPool,
    digest: &str,
    config_version: u64,
    inspect: &serde_json::Value,
    pulled_here: bool,
    own_version: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update hermes_images set config_version = $2, inspect = $3, \
           pulled_here = pulled_here or $4, prepared_at = now(), updated_at = now(), \
           version = coalesce(version, $5) \
         where digest = $1",
    )
    .bind(digest)
    .bind(config_version as i32)
    .bind(inspect)
    .bind(pulled_here)
    .bind(own_version.filter(|v| is_version_tag(v)))
    .execute(pg)
    .await?;
    Ok(())
}

/// This instance pulled the image (so this instance may prune it).
pub async fn note_pulled(pg: &PgPool, digest: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update hermes_images set pulled_here = true, updated_at = now() where digest = $1",
    )
    .bind(digest)
    .execute(pg)
    .await?;
    Ok(())
}

/// The digest ran well here.
pub async fn mark_good(pg: &PgPool, digest: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update hermes_images set status = 'good', reason = null, \
           good_since = coalesce(good_since, now()), updated_at = now() \
         where digest = $1 and status <> 'bad'",
    )
    .bind(digest)
    .execute(pg)
    .await?;
    Ok(())
}

/// The digest failed here, and why. It is never rolled out again.
pub async fn mark_bad(pg: &PgPool, digest: &str, reason: &str) -> Result<(), sqlx::Error> {
    let reason: String = reason.chars().take(2_000).collect();
    sqlx::query(
        "update hermes_images set status = 'bad', reason = $2, good_since = null, \
           updated_at = now() where digest = $1",
    )
    .bind(digest)
    .bind(reason)
    .execute(pg)
    .await?;
    Ok(())
}

/// An admin lifts a ban: the digest is a candidate again, its reason gone.
/// Answers whether it was bad.
pub async fn allow_again(pg: &PgPool, digest: &str) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "update hermes_images set status = 'candidate', reason = null, updated_at = now() \
         where digest = $1 and status = 'bad'",
    )
    .bind(digest)
    .execute(pg)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// The image passed the canary here (kept in the inspection, beside what the
/// image itself said).
pub async fn note_canary_ok(pg: &PgPool, digest: &str, at_ms: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update hermes_images set inspect = inspect || jsonb_build_object('canaryOkAt', $2::bigint), \
           updated_at = now() where digest = $1",
    )
    .bind(digest)
    .bind(at_ms)
    .execute(pg)
    .await?;
    Ok(())
}

/// The canary found contract problems of its own: added to the
/// inspection's.
pub async fn note_contract(
    pg: &PgPool,
    digest: &str,
    problems: &[String],
) -> Result<(), sqlx::Error> {
    let list = serde_json::json!(problems);
    sqlx::query(
        "update hermes_images set inspect = jsonb_set(inspect, '{contract}', \
           coalesce(inspect->'contract', '{}'::jsonb) || jsonb_build_object('problems', \
             coalesce(inspect#>'{contract,problems}', '[]'::jsonb) || $2::jsonb)), \
           updated_at = now() where digest = $1",
    )
    .bind(digest)
    .bind(list)
    .execute(pg)
    .await?;
    Ok(())
}

/// An admin accepts the contract problems: the image rolls out anyway.
/// Answers whether the image had any.
pub async fn accept_contract(
    pg: &PgPool,
    digest: &str,
    by: &str,
    at_ms: i64,
) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "update hermes_images set inspect = inspect || jsonb_build_object('contractAccepted', \
           jsonb_build_object('by', $2::text, 'at', $3::bigint)), updated_at = now() \
         where digest = $1 and jsonb_array_length(coalesce(inspect#>'{contract,problems}', '[]'::jsonb)) > 0",
    )
    .bind(digest)
    .bind(by.chars().take(200).collect::<String>())
    .bind(at_ms)
    .execute(pg)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// The image is no longer on the host (this instance removed it): a later
/// preparation pulls it again, and only then is it this instance's to prune.
pub async fn note_pruned(pg: &PgPool, digest: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update hermes_images set pulled_here = false, prepared_at = null, updated_at = now() \
         where digest = $1",
    )
    .bind(digest)
    .execute(pg)
    .await?;
    Ok(())
}

/// Every agent's assigned digest, by agent id.
pub async fn assignments(pg: &PgPool) -> Result<HashMap<String, String>, sqlx::Error> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("select agent_id::text, digest from hermes_agent_images")
            .fetch_all(pg)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Tell the render which digest an agent's container runs from here on.
pub async fn assign(pg: &PgPool, agent_id: &str, digest: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into hermes_agent_images (agent_id, digest) values ($1::uuid, $2) \
         on conflict (agent_id) do update set digest = excluded.digest, updated_at = now()",
    )
    .bind(agent_id)
    .bind(digest)
    .execute(pg)
    .await?;
    Ok(())
}

// ── The channel's state ─────────────────────────────────────────────────────

/// The maintenance window, in the instance's time zone: `HH:MM` each end.
/// An end before its start wraps past midnight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Window {
    pub start: String,
    pub end: String,
    /// Fields a later release wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Window {
    fn default() -> Self {
        Window {
            start: "02:00".into(),
            end: "05:00".into(),
            extra: Default::default(),
        }
    }
}

/// `HH:MM` as minutes past midnight.
pub fn clock_minutes(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

impl Window {
    /// Whether a local time (minutes past midnight) falls inside it. A
    /// window whose ends cannot be read is the default one.
    pub fn contains(&self, minute: u32) -> bool {
        let d = Window::default();
        let start = clock_minutes(&self.start)
            .or_else(|| clock_minutes(&d.start))
            .unwrap_or(120);
        let end = clock_minutes(&self.end)
            .or_else(|| clock_minutes(&d.end))
            .unwrap_or(300);
        if start == end {
            return false;
        }
        if start < end {
            (start..end).contains(&minute)
        } else {
            minute >= start || minute < end
        }
    }
}

/// The last look at the channel.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckRecord {
    pub at: i64,
    /// What `stable` resolved to.
    pub digest: Option<String>,
    pub version: Option<String>,
    /// Why the check failed (the registry unreachable, a refusal).
    pub error: Option<String>,
    /// Since when a successful check has named `digest` as stable: the
    /// minimum age counts from here, and restarts whenever stable moves.
    pub since: Option<i64>,
    /// Fields a later release wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Why a rollout exists.
///
/// A value this build does not know (a later release's) makes the whole row
/// unreadable to it ([`load_state_at`]), and an unreadable row is never
/// written back: the newer release's state survives a roll back to this one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RolloutKind {
    /// A new digest on the channel.
    #[default]
    Upgrade,
    /// Back to the last known good: a failure, or an admin's Roll back.
    Rollback,
    /// An admin pinned a version.
    Pin,
    /// Agents an earlier rollout left behind (busy all window, or reached
    /// by a rollout that was cancelled) moving onto the fleet's image.
    Catchup,
}

/// Where a rollout stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Pulling and inspecting the image.
    #[default]
    Preparing,
    /// Ready, waiting for its minimum time on stable, the maintenance
    /// window (or Roll now), or a retry.
    Waiting,
    /// The throwaway probe is booting the image and soaking.
    Canary,
    /// Agents are moving, one at a time, each watched before the next.
    Rolling,
    /// Every agent it moved is being watched before the image is called good.
    Soaking,
    /// Every agent runs it.
    Done,
    /// It stopped: the canary or an agent failed.
    Failed,
    /// It gave way before it moved anyone: stable moved on (or back), so the
    /// release it waited for is not rolled out.
    Cancelled,
}

/// An agent the rollout passed over for now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Skipped {
    pub slug: String,
    pub why: String,
    pub at: i64,
    /// Fields a later release wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// An agent the rollout moved, as its container was once it came up: the
/// soak compares the container against this.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Watched {
    pub slug: String,
    /// The container's id.
    pub container: String,
    /// Docker's restart count for it when it came up.
    pub restart_count: i64,
    /// When it was moved (epoch ms).
    pub moved_at: i64,
    /// The config or migration fault lines its log already carried on the
    /// image it left, normalized, so the soak counts only new ones.
    #[serde(default)]
    pub known_faults: Vec<String>,
    /// Fields a later release wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A rollout in flight, or the last one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rollout {
    /// This rollout's identity: every write the engine makes into it names
    /// it, and is refused once the row holds another rollout (an admin's
    /// Roll back or Pin replaced it mid-step).
    pub id: String,
    /// The digest the fleet is moving to.
    pub digest: String,
    /// The fleet's known-good digest when it started.
    pub from: Option<String>,
    pub kind: RolloutKind,
    pub phase: Phase,
    pub started_at: i64,
    pub updated_at: i64,
    /// Roll now: run it outside the maintenance window and before its
    /// minimum time on stable.
    pub forced: bool,
    /// When the canary passed.
    pub canary_ok_at: Option<i64>,
    /// The agent rolling now (set before its roll, in the same write as its
    /// assignment; cleared after): a step that finds it set after a restart
    /// re-verifies that agent.
    pub agent: Option<String>,
    /// The fault lines `agent`'s log carried on the image it left, so a
    /// resumed check counts only new ones.
    pub agent_faults: Vec<String>,
    /// The agents moved so far (for the record: what is still to move is
    /// read from the assignments, never from this list).
    pub moved: Vec<String>,
    /// Each agent this rollout recreated on an image not yet known good,
    /// as its container came up: the soak compares the container against
    /// this. An agent put back leaves it in the same write.
    pub watch: Vec<Watched>,
    /// The agents passed over, and why (each agent's latest).
    pub skipped: Vec<Skipped>,
    /// The agents busy all window: left on the image they run, for a
    /// catch-up rollout in a later window.
    pub deferred: Vec<String>,
    /// For a catch-up: each agent's digest before it moved, to put it back.
    pub prev: std::collections::BTreeMap<String, String>,
    /// 1 after a first image failure of `strike_on`: the retry confirms it,
    /// a second failure of the same thing marks the image bad. Cleared when
    /// that thing passes.
    pub strikes: u32,
    /// What the strike is for: `canary`, `prepare`, or an agent's slug.
    pub strike_on: Option<String>,
    /// Host failures in a row (each retry waits twice as long as the last,
    /// to six hours; the third tells a person). Cleared by progress.
    pub infra_failures: u32,
    /// Image failures read as the host's because its disk was saturated
    /// (at most two; the next is judged as the image's).
    pub masked: u32,
    /// Not before this (epoch ms): a retry after a failure.
    pub retry_at: Option<i64>,
    /// When agents started moving.
    pub rolling_since: Option<i64>,
    /// The whole rollout is watched until then before the image is good.
    pub soak_until: Option<i64>,
    /// What happened, in a sentence: the failure, or the last step.
    pub detail: Option<String>,
    /// Fields a later release wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Rollout {
    /// Whether it still has work to do.
    pub fn open(&self) -> bool {
        !matches!(self.phase, Phase::Done | Phase::Failed | Phase::Cancelled)
    }

    pub fn note_skip(&mut self, slug: &str, why: &str, at: i64) {
        self.skipped.retain(|s| s.slug != slug);
        self.skipped.push(Skipped {
            slug: slug.into(),
            why: why.into(),
            at,
            extra: Default::default(),
        });
    }
}

/// A fresh rollout id.
pub fn new_rollout_id(now_ms: i64) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i64(now_ms);
    format!("r{now_ms:x}{:08x}", h.finish() as u32)
}

/// One line of the channel's history.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelEvent {
    pub at: i64,
    pub text: String,
    /// Fields a later release wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The channel's persisted state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelState {
    /// Pinned by an admin to this digest: the channel moves nothing else.
    pub pinned: Option<String>,
    /// The fleet's known-good digest: what an agent with no assignment runs
    /// (none: the compiled pin).
    pub current: Option<String>,
    /// The known-good digest before `current`, kept on the host until
    /// `current` has been good for a while.
    pub previous: Option<String>,
    pub rollout: Option<Rollout>,
    pub last_check: Option<CheckRecord>,
    /// When the last check that reached the registry ran (epoch ms).
    pub last_good_check_at: Option<i64>,
    /// When the next scheduled check is due (epoch ms).
    pub next_check_at: Option<i64>,
    pub window: Window,
    pub events: Vec<ChannelEvent>,
    /// When each stuck-state alert last went out, by its key.
    pub alerted: std::collections::BTreeMap<String, i64>,
    /// How many windows in a row each agent (by slug) was left behind busy.
    pub deferrals: std::collections::BTreeMap<String, u32>,
    /// Fields a later release wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChannelState {
    pub fn note(&mut self, at: i64, text: impl Into<String>) {
        self.events.insert(
            0,
            ChannelEvent {
                at,
                text: text.into(),
                extra: Default::default(),
            },
        );
        self.events.truncate(EVENTS_CAP);
    }

    /// The open rollout, if it is the one named.
    pub fn rollout_if(&mut self, id: &str) -> Option<&mut Rollout> {
        self.rollout.as_mut().filter(|r| r.id == id && r.open())
    }
}

/// Why the state could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// The database did not answer.
    Store(String),
    /// The row is there and this build cannot read it (corrupt, or written
    /// by a later release): nothing reads it as a default, nothing writes it.
    Unreadable(String),
    /// The engine's write named a rollout the row no longer holds.
    Superseded,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Store(e) => write!(f, "the agent image state could not be read: {e}"),
            StateError::Unreadable(e) => write!(
                f,
                "the agent image state (app_settings '{STATE_KEY}') is unreadable, so the channel moves nothing until it is fixed: {e}"
            ),
            StateError::Superseded => {
                write!(f, "the rollout was replaced while this step worked on it")
            }
        }
    }
}

/// Parse a stored row. Absent is the default (nothing has happened yet);
/// anything stored that does not parse is an error, never a default.
pub fn parse_state(stored: Option<serde_json::Value>) -> Result<ChannelState, StateError> {
    match stored {
        None | Some(serde_json::Value::Null) => Ok(ChannelState::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| StateError::Unreadable(e.to_string())),
    }
}

/// Read the state under `key` (the channel's is [`STATE_KEY`]).
pub async fn load_state_at(pg: &PgPool, key: &str) -> Result<ChannelState, StateError> {
    let stored = talaria_gateway::settings::try_get_setting(pg, key)
        .await
        .map_err(|e| StateError::Store(e.to_string()))?;
    parse_state(stored)
}

/// Read the channel's state.
pub async fn load_state(pg: &PgPool) -> Result<ChannelState, StateError> {
    load_state_at(pg, STATE_KEY).await
}

/// Writes to the records and the assignments that belong with a state
/// change: applied in the state's own transaction, so they happen exactly
/// when it commits — never after a write that was refused (a step whose
/// rollout an admin replaced), never before one that fails.
#[derive(Debug, Default)]
pub struct Effects {
    assign: Vec<(String, String)>,
    good: Vec<String>,
    bad: Vec<(String, String)>,
    allow: Vec<String>,
}

impl Effects {
    /// The agent (by `agent_defs.id`) runs `digest` from its next render.
    pub fn assign(&mut self, agent_id: &str, digest: &str) {
        self.assign.push((agent_id.to_string(), digest.to_string()));
    }
    /// The digest ran well here.
    pub fn mark_good(&mut self, digest: &str) {
        self.good.push(digest.to_string());
    }
    /// The digest failed here, and why.
    pub fn mark_bad(&mut self, digest: &str, reason: &str) {
        self.bad
            .push((digest.to_string(), reason.chars().take(2_000).collect()));
    }
    /// An admin lifts the digest's ban.
    pub fn allow_again(&mut self, digest: &str) {
        self.allow.push(digest.to_string());
    }
}

/// Read, change and write the state under `key` in one transaction, the row
/// locked: two writers (the engine's step and an admin's action, on any api
/// instance) take turns, and neither writes over what the other read. The
/// closure may refuse (an `Err`), and then nothing is written. A row that
/// cannot be read is never written.
pub async fn patch_state_at<T, F>(
    pg: &PgPool,
    key: &str,
    f: F,
) -> Result<(ChannelState, T), StateError>
where
    F: FnOnce(&mut ChannelState) -> Result<T, StateError>,
{
    patch_state_fx_at(pg, key, |s, _| f(s)).await
}

/// [`patch_state_at`], with the [`Effects`] the closure asks for applied in
/// the same transaction.
pub async fn patch_state_fx_at<T, F>(
    pg: &PgPool,
    key: &str,
    f: F,
) -> Result<(ChannelState, T), StateError>
where
    F: FnOnce(&mut ChannelState, &mut Effects) -> Result<T, StateError>,
{
    let store = |e: sqlx::Error| StateError::Store(e.to_string());
    let mut tx = pg.begin().await.map_err(store)?;
    sqlx::query(
        "insert into app_settings (key, value) values ($1, 'null'::jsonb) on conflict (key) do nothing",
    )
    .bind(key)
    .execute(&mut *tx)
    .await
    .map_err(store)?;
    let stored: Option<serde_json::Value> =
        sqlx::query_scalar("select value from app_settings where key = $1 for update")
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store)?;
    let mut state = parse_state(stored)?;
    let mut fx = Effects::default();
    let out = f(&mut state, &mut fx)?;
    let value = serde_json::to_value(&state)
        .map_err(|e| StateError::Unreadable(format!("the state would not serialize: {e}")))?;
    sqlx::query("update app_settings set value = $2, updated_at = now() where key = $1")
        .bind(key)
        .bind(&value)
        .execute(&mut *tx)
        .await
        .map_err(store)?;
    for (agent, digest) in &fx.assign {
        sqlx::query(
            "insert into hermes_agent_images (agent_id, digest) values ($1::uuid, $2) \
             on conflict (agent_id) do update set digest = excluded.digest, updated_at = now()",
        )
        .bind(agent)
        .bind(digest)
        .execute(&mut *tx)
        .await
        .map_err(store)?;
    }
    for digest in &fx.good {
        sqlx::query(
            "update hermes_images set status = 'good', reason = null, \
               good_since = coalesce(good_since, now()), updated_at = now() \
             where digest = $1 and status <> 'bad'",
        )
        .bind(digest)
        .execute(&mut *tx)
        .await
        .map_err(store)?;
    }
    for (digest, reason) in &fx.bad {
        sqlx::query(
            "update hermes_images set status = 'bad', reason = $2, good_since = null, \
               updated_at = now() where digest = $1",
        )
        .bind(digest)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(store)?;
    }
    for digest in &fx.allow {
        sqlx::query(
            "update hermes_images set status = 'candidate', reason = null, updated_at = now() \
             where digest = $1 and status = 'bad'",
        )
        .bind(digest)
        .execute(&mut *tx)
        .await
        .map_err(store)?;
    }
    tx.commit().await.map_err(store)?;
    Ok((state, out))
}

/// Set an unreadable state aside and start a fresh one that keeps what the
/// fleet runs: the old row is archived under `<key>:archived:<ms>` exactly as
/// it was, and the new one carries over `current`, `previous` and `pinned`
/// where they read as digests (and nothing else — no rollout). Refused when
/// the row reads: there is nothing to reset. Answers the archive's key.
pub async fn reset_state_at(
    pg: &PgPool,
    key: &str,
    now_ms: i64,
    by: &str,
) -> Result<String, StateError> {
    let store = |e: sqlx::Error| StateError::Store(e.to_string());
    let mut tx = pg.begin().await.map_err(store)?;
    let stored: Option<serde_json::Value> =
        sqlx::query_scalar("select value from app_settings where key = $1 for update")
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store)?;
    if parse_state(stored.clone()).is_ok() {
        return Err(StateError::Store(
            "the agent image state reads fine: there is nothing to reset".into(),
        ));
    }
    let stored = stored.unwrap_or(serde_json::Value::Null);
    let archive = format!("{key}:archived:{now_ms}");
    sqlx::query("insert into app_settings (key, value) values ($1, $2)")
        .bind(&archive)
        .bind(&stored)
        .execute(&mut *tx)
        .await
        .map_err(store)?;
    let digest_at = |field: &str| {
        stored
            .get(field)
            .and_then(|v| v.as_str())
            .filter(|d| is_digest(d))
            .map(str::to_string)
    };
    let mut fresh = ChannelState {
        current: digest_at("current"),
        previous: digest_at("previous"),
        pinned: digest_at("pinned"),
        ..Default::default()
    };
    fresh.note(
        now_ms,
        format!("{by} reset the unreadable state (kept as app_settings '{archive}'); the fleet stays on its image"),
    );
    let value = serde_json::to_value(&fresh)
        .map_err(|e| StateError::Unreadable(format!("the state would not serialize: {e}")))?;
    sqlx::query("update app_settings set value = $2, updated_at = now() where key = $1")
        .bind(key)
        .bind(&value)
        .execute(&mut *tx)
        .await
        .map_err(store)?;
    tx.commit().await.map_err(store)?;
    Ok(archive)
}

/// [`patch_state_at`] on the channel's own row.
pub async fn patch_state<T, F>(pg: &PgPool, f: F) -> Result<(ChannelState, T), StateError>
where
    F: FnOnce(&mut ChannelState) -> Result<T, StateError>,
{
    patch_state_at(pg, STATE_KEY, f).await
}

// ── What the render stamps ──────────────────────────────────────────────────

/// One agent's image and the config schema version that image reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub image: String,
    pub config_version: u64,
}

/// What every agent runs, from the records: an agent's own assignment, or
/// the channel's current image, or the compiled pin.
#[derive(Clone, Debug)]
pub struct ImagePlan {
    pub default: Planned,
    per_agent: HashMap<String, Planned>,
}

impl ImagePlan {
    /// The plan with no records at all: everything on the compiled pin.
    pub fn compiled(compiled: Planned) -> ImagePlan {
        ImagePlan {
            default: compiled,
            per_agent: HashMap::new(),
        }
    }

    /// What the agent (by `agent_defs.id`) runs.
    pub fn for_agent(&self, agent_id: &str) -> &Planned {
        self.per_agent.get(agent_id).unwrap_or(&self.default)
    }
}

/// Build the plan from what was read. A digest with no runnable record (not
/// prepared, its config version unknown), or one marked bad, is never
/// planned: the agent stays on the default (the fleet's current image, else
/// the compiled pin) instead of rendering an image nobody inspected or one
/// that failed.
pub fn plan_from(
    compiled: Planned,
    current: Option<&str>,
    records: &[ImageRecord],
    assigned: &HashMap<String, String>,
) -> ImagePlan {
    let fallback_version = compiled.config_version;
    let planned = |digest: &str| {
        records
            .iter()
            .find(|r| r.digest == digest && r.runnable() && r.status != Status::Bad)
            .map(|r| Planned {
                image: r.image_ref(),
                config_version: r.config_version.unwrap_or(fallback_version),
            })
    };
    let default = current.and_then(planned).unwrap_or(compiled);
    let per_agent = assigned
        .iter()
        .filter_map(|(agent, digest)| planned(digest).map(|p| (agent.clone(), p)))
        .collect();
    ImagePlan { default, per_agent }
}

/// Read the plan. A failed read of the records or the assignments is the
/// render's failure, never a silent fall back to the compiled pin: that would
/// move every agent's image on a database hiccup, and the next apply would
/// roll them all.
///
/// The channel's state row is read for its `current` alone, and leniently: a
/// row this build cannot read (written by a later release, or corrupt) stops
/// the channel (engine.rs), never a hire, a start or an edit. `current` that
/// is not a digest reads as none — every agent keeps its own assignment, and
/// one without falls to the compiled pin until an admin resets the row.
pub async fn image_plan(pg: &PgPool, compiled: Planned) -> Result<ImagePlan, String> {
    image_plan_at(pg, STATE_KEY, compiled).await
}

/// [`image_plan`] over the state under `key`.
pub async fn image_plan_at(pg: &PgPool, key: &str, compiled: Planned) -> Result<ImagePlan, String> {
    let records = records(pg).await.map_err(|e| e.to_string())?;
    let assigned = assignments(pg).await.map_err(|e| e.to_string())?;
    let stored = talaria_gateway::settings::try_get_setting(pg, key)
        .await
        .map_err(|e| e.to_string())?;
    Ok(plan_from(
        compiled,
        current_of(stored.as_ref()).as_deref(),
        &records,
        &assigned,
    ))
}

/// A stored state's `current`, read on its own: a digest, or none.
pub fn current_of(stored: Option<&serde_json::Value>) -> Option<String> {
    stored?
        .get("current")?
        .as_str()
        .filter(|d| is_digest(d))
        .map(str::to_string)
}

/// The env names the GOOD images on record strip, for the render to adopt:
/// an image's names count only once it has run well here, and stop counting
/// if it goes bad. Read from the records, so every api process adopts the
/// same set.
pub async fn stripped_env_of_good(pg: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    Ok(records(pg)
        .await?
        .into_iter()
        .filter(|r| r.status == Status::Good)
        .flat_map(|r| r.stripped_env())
        .collect())
}

// ── An image the operator chose ─────────────────────────────────────────────

/// The image an operator set with `HERMES_IMAGE`: in the api's own
/// environment (compose reads its caller's env first) or in `fleet/.env`.
/// Either overrides every rendered default, so the channel stands aside.
pub async fn operator_image() -> Option<String> {
    if let Ok(v) = std::env::var("HERMES_IMAGE")
        && !v.trim().is_empty()
    {
        return Some(v.trim().to_string());
    }
    let env = tokio::fs::read_to_string(talaria_fleet_layout::fleet_env())
        .await
        .unwrap_or_default();
    env_file_image(&env)
}

/// `HERMES_IMAGE=<value>` in an env file's text, unquoted; empty is unset.
pub fn env_file_image(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("HERMES_IMAGE="))
        .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .rfind(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const D1: &str = "sha256:7a69201726bc64db2072a1371557137c23312d29705cda2b8ea2a0214a50a4cf";
    const D2: &str = "sha256:d27689bc6e8581ec884a7227c9101fdaebf6c853cff8859020dac14f21452b4d";

    fn rec(digest: &str, version: Option<&str>, cv: Option<u64>, prepared: bool) -> ImageRecord {
        ImageRecord {
            digest: digest.into(),
            repository: "nousresearch/hermes-agent".into(),
            version: version.map(str::to_string),
            config_version: cv,
            status: Status::Good,
            reason: None,
            inspect: serde_json::json!({}),
            pulled_here: false,
            first_seen: 0,
            prepared_at: prepared.then_some(1),
            good_since: None,
        }
    }

    fn compiled() -> Planned {
        Planned {
            image: format!("nousresearch/hermes-agent:v0.22.4@{D1}"),
            config_version: 51,
        }
    }

    #[test]
    fn untrusted_words_are_checked_before_they_are_kept() {
        assert!(is_digest(D1));
        assert!(!is_digest("sha256:ABC"));
        assert!(!is_digest(&D1.to_uppercase()));
        assert!(!is_digest("sha512:00"));
        assert!(is_version_tag("v0.22.6"));
        for not in [
            "stable",
            "latest",
            "main",
            "v0.22",
            "v0.22.6-desktop",
            "rc.1-v0.22.6",
            "0.22.6",
            "v0.22.6\nx",
            "v00000.1.1",
        ] {
            assert!(!is_version_tag(not), "{not}");
        }
        assert_eq!(version_key("v0.22.10"), Some((0, 22, 10)));
        assert!(version_key("v0.22.10") > version_key("v0.22.9"));
        assert!(is_env_name("OPENAI_API_KEY"));
        for not in ["", "lower", "A B", "A=B", "1ABC", "A-B"] {
            assert!(!is_env_name(not), "{not}");
        }
        assert!(is_config_version(51));
        assert!(!is_config_version(0) && !is_config_version(-1) && !is_config_version(10_001));
    }

    #[test]
    fn an_image_ref_carries_its_tag_only_when_the_tag_is_a_version() {
        assert_eq!(
            image_ref("nousresearch/hermes-agent", Some("v0.22.6"), D2),
            format!("nousresearch/hermes-agent:v0.22.6@{D2}")
        );
        assert_eq!(
            image_ref("nousresearch/hermes-agent", Some("stable"), D2),
            format!("nousresearch/hermes-agent@{D2}")
        );
        assert_eq!(label(None, D2), "sha256:d27689bc6e85");
    }

    #[test]
    fn the_plan_runs_assignments_then_current_then_the_compiled_pin() {
        let records = vec![
            rec(D1, Some("v0.22.4"), Some(51), true),
            rec(D2, Some("v0.22.6"), Some(52), true),
        ];
        // Nothing recorded as current, nothing assigned: the compiled pin.
        let plan = plan_from(compiled(), None, &records, &HashMap::new());
        assert_eq!(plan.for_agent("a"), &compiled());

        // Current moved: every unassigned agent follows it, at the image's
        // own config version.
        let plan = plan_from(compiled(), Some(D2), &records, &HashMap::new());
        assert_eq!(
            plan.for_agent("a"),
            &Planned {
                image: format!("nousresearch/hermes-agent:v0.22.6@{D2}"),
                config_version: 52
            }
        );

        // An agent's own assignment wins over current, both ways.
        let assigned = HashMap::from([("a".to_string(), D1.to_string())]);
        let plan = plan_from(compiled(), Some(D2), &records, &assigned);
        assert_eq!(plan.for_agent("a").config_version, 51);
        assert_eq!(plan.for_agent("b").config_version, 52);
    }

    #[test]
    fn an_image_nobody_inspected_is_never_planned() {
        // Seen but not prepared, or prepared without a config version: the
        // agent stays on the default rather than run an unread image.
        let records = vec![
            rec(D2, Some("v0.22.6"), Some(52), false),
            rec(D1, Some("v0.22.4"), None, true),
        ];
        let assigned = HashMap::from([
            ("a".to_string(), D2.to_string()),
            ("b".to_string(), D1.to_string()),
        ]);
        let plan = plan_from(compiled(), Some(D2), &records, &assigned);
        assert_eq!(plan.for_agent("a"), &compiled());
        assert_eq!(plan.for_agent("b"), &compiled());
    }

    #[test]
    fn a_status_this_build_does_not_know_reads_as_bad() {
        assert_eq!(Status::parse("good"), Status::Good);
        assert_eq!(Status::parse("candidate"), Status::Candidate);
        assert_eq!(Status::parse("revoked"), Status::Bad, "fails closed");
    }

    #[test]
    fn the_render_reads_current_alone_and_leniently() {
        let newer = serde_json::json!({ "current": D2, "rollout": { "phase": "verifying" } });
        assert_eq!(current_of(Some(&newer)).as_deref(), Some(D2));
        assert_eq!(current_of(Some(&serde_json::json!("garbled"))), None);
        assert_eq!(
            current_of(Some(&serde_json::json!({ "current": "latest" }))),
            None
        );
        assert_eq!(current_of(None), None);
    }

    #[test]
    fn a_banned_digest_is_never_planned() {
        // Bad under a rollback, or banned by an admin: an agent assigned it,
        // and the fleet if it is current, render the next image down.
        let mut bad = rec(D2, Some("v0.22.6"), Some(52), true);
        bad.status = Status::Bad;
        let records = vec![rec(D1, Some("v0.22.4"), Some(51), true), bad];
        let assigned = HashMap::from([("a".to_string(), D2.to_string())]);
        let plan = plan_from(compiled(), Some(D1), &records, &assigned);
        assert_eq!(plan.for_agent("a").config_version, 51, "assigned a bad one");
        let plan = plan_from(compiled(), Some(D2), &records, &HashMap::new());
        assert_eq!(plan.for_agent("b"), &compiled(), "current is bad");
    }

    #[test]
    fn the_window_wraps_past_midnight_and_reads_its_default_when_garbled() {
        let w = Window::default();
        assert!(w.contains(2 * 60) && w.contains(4 * 60 + 59));
        assert!(!w.contains(5 * 60) && !w.contains(60 + 59));
        let night = Window {
            start: "23:30".into(),
            end: "01:00".into(),
            ..Default::default()
        };
        assert!(night.contains(23 * 60 + 45) && night.contains(30));
        assert!(!night.contains(60) && !night.contains(12 * 60));
        let garbled = Window {
            start: "25:00".into(),
            end: "nope".into(),
            ..Default::default()
        };
        assert!(garbled.contains(3 * 60));
        assert_eq!(clock_minutes("2:05"), Some(125));
        assert_eq!(clock_minutes("24:00"), None);
    }

    #[test]
    fn hermes_image_in_the_env_file_is_the_operators() {
        assert_eq!(env_file_image("A=1\nHERMES_IMAGE=\n"), None);
        assert_eq!(
            env_file_image("HERMES_IMAGE=\"x/y:z\"\nB=2"),
            Some("x/y:z".into())
        );
        assert_eq!(env_file_image("# HERMES_IMAGE=x\n"), None);
    }

    #[test]
    fn an_unreadable_row_is_an_error_and_never_the_default() {
        // Absent: nothing has happened yet.
        assert_eq!(parse_state(None).unwrap(), ChannelState::default());
        // Corrupt, or a value this build does not know (a later release's
        // phase): refused, so nothing writes the default back over it.
        for stored in [
            serde_json::json!("not an object"),
            serde_json::json!({ "pinned": 7 }),
            serde_json::json!({ "rollout": { "id": "r", "phase": "verifying" } }),
            serde_json::json!({ "rollout": { "kind": "staged" } }),
        ] {
            assert!(
                matches!(
                    parse_state(Some(stored.clone())),
                    Err(StateError::Unreadable(_))
                ),
                "{stored}"
            );
        }
    }

    #[test]
    fn a_field_a_later_release_wrote_survives_a_round_trip() {
        let stored = serde_json::json!({
            "current": D2,
            "stagingRing": 3,
            "rollout": { "id": "r1", "digest": D2, "phase": "rolling", "ringFrom": "a" }
        });
        let state = parse_state(Some(stored)).unwrap();
        let back = serde_json::to_value(&state).unwrap();
        assert_eq!(back["stagingRing"], serde_json::json!(3));
        assert_eq!(back["rollout"]["ringFrom"], serde_json::json!("a"));
        assert_eq!(back["rollout"]["phase"], serde_json::json!("rolling"));
    }

    #[test]
    fn only_the_named_open_rollout_takes_the_engines_writes() {
        let mut s = ChannelState {
            rollout: Some(Rollout {
                id: "a".into(),
                phase: Phase::Rolling,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(s.rollout_if("a").is_some());
        assert!(s.rollout_if("b").is_none(), "another rollout replaced it");
        s.rollout.as_mut().unwrap().phase = Phase::Done;
        assert!(
            s.rollout_if("a").is_none(),
            "a closed rollout takes nothing"
        );
        assert_ne!(new_rollout_id(1), new_rollout_id(1));
    }

    #[test]
    fn a_bad_image_is_never_pinnable_and_an_inspected_one_is() {
        let mut r = rec(D2, Some("v0.22.6"), Some(52), true);
        r.status = Status::Candidate;
        assert!(r.pinnable(), "inspected here; a Pin canaries it first");
        r.prepared_at = None;
        assert!(!r.pinnable(), "nobody read it");
        r.status = Status::Bad;
        r.prepared_at = Some(1);
        assert!(!r.pinnable());
        r.status = Status::Good;
        assert!(r.pinnable());
        r.inspect = serde_json::json!({ "canaryOkAt": 5 });
        assert_eq!(r.canary_ok_at(), Some(5));
    }

    #[test]
    fn the_state_reads_its_defaults_from_an_old_or_partial_row() {
        let s: ChannelState = serde_json::from_value(serde_json::json!({
            "current": D2,
            "window": {"start": "01:00"}
        }))
        .unwrap();
        assert_eq!(s.current.as_deref(), Some(D2));
        assert_eq!(s.window.end, "05:00");
        assert!(s.rollout.is_none() && s.pinned.is_none());
    }
}
