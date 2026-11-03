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
use talaria_gateway::settings::{get_setting, set_setting};

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

    fn parse(s: &str) -> Status {
        match s {
            "good" => Status::Good,
            "bad" => Status::Bad,
            _ => Status::Candidate,
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
}

impl Default for Window {
    fn default() -> Self {
        Window {
            start: "02:00".into(),
            end: "05:00".into(),
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
}

/// Why a rollout exists.
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
}

/// Where a rollout stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Pulling and inspecting the image.
    #[default]
    Preparing,
    /// Ready, waiting for the maintenance window (or Roll now).
    Waiting,
    /// The throwaway probe is booting the image.
    Canary,
    /// Agents are moving, one at a time.
    Rolling,
    /// Every agent runs it.
    Done,
    /// It stopped: the canary or an agent failed.
    Failed,
}

/// An agent the rollout passed over for now.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Skipped {
    pub slug: String,
    pub why: String,
    pub at: i64,
}

/// A rollout in flight, or the last one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rollout {
    /// The digest the fleet is moving to.
    pub digest: String,
    /// The fleet's known-good digest when it started.
    pub from: Option<String>,
    pub kind: RolloutKind,
    pub phase: Phase,
    pub started_at: i64,
    pub updated_at: i64,
    /// Roll now: run it outside the maintenance window.
    pub forced: bool,
    /// When the canary passed.
    pub canary_ok_at: Option<i64>,
    /// The agent rolling now.
    pub agent: Option<String>,
    /// The agents moved so far.
    pub moved: Vec<String>,
    /// The agents passed over, and why (each agent's latest).
    pub skipped: Vec<Skipped>,
    /// What happened, in a sentence: the failure, or the last step.
    pub detail: Option<String>,
}

impl Rollout {
    /// Whether it still has work to do.
    pub fn open(&self) -> bool {
        !matches!(self.phase, Phase::Done | Phase::Failed)
    }

    pub fn note_skip(&mut self, slug: &str, why: &str, at: i64) {
        self.skipped.retain(|s| s.slug != slug);
        self.skipped.push(Skipped {
            slug: slug.into(),
            why: why.into(),
            at,
        });
    }
}

/// One line of the channel's history.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelEvent {
    pub at: i64,
    pub text: String,
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
    /// When the next scheduled check is due (epoch ms).
    pub next_check_at: Option<i64>,
    pub window: Window,
    pub events: Vec<ChannelEvent>,
}

impl ChannelState {
    pub fn note(&mut self, at: i64, text: impl Into<String>) {
        self.events.insert(
            0,
            ChannelEvent {
                at,
                text: text.into(),
            },
        );
        self.events.truncate(EVENTS_CAP);
    }
}

/// Read the state. Absent or unreadable is the default: following stable,
/// nothing moved yet, the compiled pin current.
pub async fn load_state(pg: &PgPool) -> ChannelState {
    match get_setting(pg, STATE_KEY, serde_json::Value::Null).await {
        v if v.is_null() => ChannelState::default(),
        v => serde_json::from_value(v).unwrap_or_default(),
    }
}

/// Read, patch, write; answers what landed. One writer at a time is the
/// channel engine's own lock; the admin actions patch single fields.
pub async fn patch_state<F>(pg: &PgPool, f: F) -> Result<ChannelState, sqlx::Error>
where
    F: FnOnce(&mut ChannelState),
{
    let mut state = load_state(pg).await;
    f(&mut state);
    set_setting(
        pg,
        STATE_KEY,
        &serde_json::to_value(&state).expect("channel state serializes"),
    )
    .await?;
    Ok(state)
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
/// prepared, its config version unknown) is never planned: the agent stays
/// on the default instead of rendering an image nobody inspected.
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
            .find(|r| r.digest == digest && r.runnable())
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

/// Read the plan. A read that fails is the render's failure, never a
/// silent fall back to the compiled pin: that would move every agent's
/// image on a database hiccup, and the next apply would roll them all.
pub async fn image_plan(pg: &PgPool, compiled: Planned) -> Result<ImagePlan, sqlx::Error> {
    let records = records(pg).await?;
    let assigned = assignments(pg).await?;
    let state = load_state(pg).await;
    Ok(plan_from(
        compiled,
        state.current.as_deref(),
        &records,
        &assigned,
    ))
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
    fn the_window_wraps_past_midnight_and_reads_its_default_when_garbled() {
        let w = Window::default();
        assert!(w.contains(2 * 60) && w.contains(4 * 60 + 59));
        assert!(!w.contains(5 * 60) && !w.contains(60 + 59));
        let night = Window {
            start: "23:30".into(),
            end: "01:00".into(),
        };
        assert!(night.contains(23 * 60 + 45) && night.contains(30));
        assert!(!night.contains(60) && !night.contains(12 * 60));
        let garbled = Window {
            start: "25:00".into(),
            end: "nope".into(),
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
