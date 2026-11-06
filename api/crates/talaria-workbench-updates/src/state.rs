// The channel's state: ONE `app_settings` row
// (`talaria_workbench_harnesses::CHANNEL_STATE_KEY`). Provisioning reads only
// its `current` and `previous` pairs (`talaria_workbench_harnesses::effective`);
// everything else is the channel's own bookkeeping and the panel's view.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use talaria_workbench_harnesses::Versions;

use crate::Tool;

/// The most recent events kept.
const EVENTS_KEPT: usize = 40;
/// The newest releases kept per harness.
const SEEN_KEPT: usize = 30;
/// The newest holds kept per harness (each carries its reason).
pub const HELD_KEPT: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    /// The pair the channel last verified on a workbench: what every new job
    /// runs. None until the first move: jobs run the bootstrap fallback.
    pub current: Option<Pair>,
    /// The pair before it: a volume that will not take `current` runs this.
    pub previous: Option<Pair>,
    /// Per harness (`bun`, `omp`).
    pub tools: BTreeMap<String, ToolState>,
    pub next_check_at: Option<i64>,
    /// Per workbench volume (department): the pair last verified there.
    pub volumes: BTreeMap<String, Volume>,
    /// Smoke tests the host could not run, in a row.
    pub host_failures: u32,
    /// No smoke test before this (the host's backoff).
    pub retry_at: Option<i64>,
    /// When each alert topic last went to the admins.
    pub alerted: BTreeMap<String, i64>,
    pub events: Vec<Event>,
    /// Alerts a patch decided are due; sent once it commits. Never stored.
    #[serde(skip)]
    pub outbox: Vec<Alert>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    pub bun: String,
    pub omp: String,
    #[serde(default)]
    pub at: i64,
}

impl Pair {
    pub fn of(v: &Versions, at: i64) -> Pair {
        Pair {
            bun: v.bun.clone(),
            omp: v.omp.clone(),
            at,
        }
    }
    pub fn versions(&self) -> Versions {
        Versions {
            bun: self.bun.clone(),
            omp: self.omp.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolState {
    /// An admin's Pin: this version and no other, until Unpin.
    pub pinned: Option<String>,
    /// An admin's Update now: adopt this version without its soak.
    pub forced: Option<String>,
    /// Releases the checks have seen, by version.
    pub seen: BTreeMap<String, Seen>,
    /// Releases that failed to install, start or pass the preflight on a
    /// workbench, by version: never adopted until an admin allows them again.
    pub held: BTreeMap<String, Held>,
    pub last_check: Option<LastCheck>,
    /// Why the newest release is not what jobs run yet (the panel's line).
    pub detail: Option<String>,
    /// Releases whose smoke test timed out, by version: held on the second
    /// in a row, never on the first (a slow or stalled host times out too).
    pub strikes: BTreeMap<String, Strike>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Seen {
    /// The first check that saw it: its soak counts from here.
    pub first_seen_at: i64,
    /// Oh My Pi's `engines.bun`.
    pub requires_bun: Option<String>,
    /// npm's deprecation message, or that upstream no longer has it.
    pub deprecated: Option<String>,
    /// npm's `dist.integrity` for the tarball.
    pub integrity: Option<String>,
    /// Upstream answered 404 for its document this many times in a row: two
    /// mark it withdrawn, one is a blip.
    pub missing: u32,
}

/// A release's smoke tests that ran past a bound, in a row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Strike {
    pub count: u32,
    /// The workbenches it timed out on (the next try prefers another).
    pub departments: Vec<String>,
    pub at: i64,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Held {
    pub reason: String,
    pub at: i64,
    /// The workbench the smoke test ran on.
    pub department: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LastCheck {
    pub at: i64,
    pub latest: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Volume {
    /// The pair last verified on this volume.
    pub bun: Option<String>,
    pub omp: Option<String>,
    pub at: i64,
    pub error: Option<String>,
    /// The pair this volume would not take (the release's own failure
    /// there): its provisions go straight to the fallback until the channel's
    /// next try here succeeds (`talaria_workbench_harnesses::attempts_from`).
    pub refused: Option<Pair>,
    /// Prewarms here the host failed, in a row, and no prewarm before
    /// `retry_at`: this volume's own backoff, apart from the channel's.
    pub host_failures: u32,
    pub retry_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub at: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub topic: String,
    pub title: String,
    pub body: String,
}

/// An alert topic goes to the admins at most this often.
pub const ALERT_EVERY_MS: i64 = 24 * 60 * 60 * 1000;

/// Read a stored row: absent or `null` is a fresh state; anything else must
/// parse, or nothing acts on it (provisioning still runs the bootstrap).
pub fn parse_state(stored: Option<serde_json::Value>) -> Result<State, String> {
    match stored {
        None | Some(serde_json::Value::Null) => Ok(State::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| {
            format!(
                "the workbench harness state (app_settings '{}') is unreadable: {e}",
                talaria_workbench_harnesses::CHANNEL_STATE_KEY
            )
        }),
    }
}

impl State {
    /// What jobs run now: the verified pair, else the bootstrap fallback.
    pub fn running(&self) -> Versions {
        self.current
            .as_ref()
            .map(Pair::versions)
            .filter(Versions::plain)
            .unwrap_or_else(Versions::bootstrap)
    }

    pub fn tool(&self, t: Tool) -> ToolState {
        self.tools.get(t.slug()).cloned().unwrap_or_default()
    }

    pub fn tool_mut(&mut self, t: Tool) -> &mut ToolState {
        self.tools.entry(t.slug().to_string()).or_default()
    }

    pub fn event(&mut self, at: i64, text: impl Into<String>) {
        self.events.push(Event {
            at,
            text: text.into(),
        });
        let over = self.events.len().saturating_sub(EVENTS_KEPT);
        self.events.drain(..over);
    }

    /// Queue an alert unless its topic went out within [`ALERT_EVERY_MS`].
    pub fn alert(&mut self, now: i64, topic: &str, title: &str, body: &str) {
        self.alerted.retain(|_, at| now - *at < 31 * ALERT_EVERY_MS);
        if self
            .alerted
            .get(topic)
            .is_some_and(|at| now - at < ALERT_EVERY_MS)
        {
            return;
        }
        self.alerted.insert(topic.to_string(), now);
        self.outbox.push(Alert {
            topic: topic.into(),
            title: title.into(),
            body: body.into(),
        });
    }

    /// Hold `version` of `t`, keeping at most the [`HELD_KEPT`] newest holds.
    pub fn hold(&mut self, t: Tool, version: &str, held: Held) {
        let ts = self.tool_mut(t);
        ts.held.insert(version.to_string(), held);
        while ts.held.len() > HELD_KEPT {
            let Some(oldest) = ts
                .held
                .iter()
                .min_by_key(|(_, h)| h.at)
                .map(|(v, _)| v.clone())
            else {
                break;
            };
            ts.held.remove(&oldest);
        }
    }

    /// Upstream has withdrawn or deprecated `version` of `t`.
    pub fn is_bad(&self, t: Tool, version: &str) -> bool {
        self.tools
            .get(t.slug())
            .and_then(|ts| ts.seen.get(version))
            .is_some_and(|s| s.deprecated.is_some())
    }

    /// Upstream answered 404 for `version` of `t`'s document. The second in a
    /// row marks it withdrawn (never adopted, and moved off if jobs run it);
    /// one is a blip. Whether it is withdrawn now.
    pub fn missed(&mut self, t: Tool, version: &str, now: i64) -> bool {
        let ts = self.tool_mut(t);
        let e = ts.seen.entry(version.to_string()).or_insert(Seen {
            first_seen_at: now,
            ..Default::default()
        });
        e.missing = e.missing.saturating_add(1);
        if e.missing >= 2 && e.deprecated.is_none() {
            e.deprecated = Some("withdrawn: upstream no longer has it".into());
        }
        e.missing >= 2
    }

    /// Record a release a check (or a Pin) read. Its soak clock starts the
    /// first time it is seen and never restarts.
    pub fn saw(&mut self, t: Tool, rel: &crate::upstream::Release, now: i64) {
        let ts = self.tool_mut(t);
        let e = ts.seen.entry(rel.version.clone()).or_insert(Seen {
            first_seen_at: now,
            ..Default::default()
        });
        e.requires_bun = rel.requires_bun.clone();
        e.deprecated = rel.deprecated.clone();
        e.missing = 0;
        if rel.integrity.is_some() {
            e.integrity = rel.integrity.clone();
        }
        if ts.seen.len() > SEEN_KEPT {
            let keep: Vec<String> = {
                let mut vs: Vec<&String> = ts.seen.keys().collect();
                vs.sort_by(|a, b| crate::semver::cmp(b, a));
                vs.into_iter().take(SEEN_KEPT).cloned().collect()
            };
            ts.seen.retain(|v, _| keep.contains(v));
        }
    }
}
