// Update state — the engine's persisted truth, one settings row (key
// `"updates"`; the git-updater's `"updater"` row retires with it).
//
// READS AND WRITES GO THROUGH get_setting/set_setting RAW, deliberately not
// get_setting_hot: the hot window exists to take keys off the per-completion
// path, and this row is nowhere near it — while green's boot reconcile and
// the roll engine are RAW writers by design (a green container reconciling
// its own birth must read what blue last wrote, never a 15s-old window,
// and the settings.rs LAW says only set_setting's own callers may opt a key
// in anyway).
//
// AUTO-UPDATE IS OFF until an admin turns it on (autoUpdate's default):
// nothing here updates itself by default, ever. The toggle is the whole
// consent story for the scheduled half of the engine.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::gateway::settings::{get_setting, set_setting};

/// The settings row's key.
pub const KEY: &str = "updates";

/// How far back the run history reaches. The row is read by the panel and
/// written by every run; unbounded history is a settings row that only ever
/// grows.
const HISTORY_CAP: usize = 10;

/// One pinned image — a digest and the human-readable version beside it.
/// The digest is the identity; the version is a label for reading.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Pin {
    /// `sha256:<64 hex>` — the only ref a roll ever pulls.
    pub digest: String,
    /// The image's org.opencontainers.image.version (e.g. `sha-<sha12>`).
    pub version: String,
}

/// Where a run stands. The order is the choreography: the run walks
/// pulling → starting → cutting-over → done, and every other value is a way
/// of not reaching done.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    /// Digest resolved, `docker pull` under way.
    Pulling,
    /// The incoming slot is up and gating on health.
    Starting,
    /// Green is healthy and aliased; blue recorded this and stopped itself.
    /// GREEN's boot reconcile is the only writer that moves it to done.
    CuttingOver,
    /// Green booted, verified itself through the edge, and owns the run.
    Done,
    /// A step refused (unhealthy replacement, pull failure…). The old
    /// container kept serving; the sentence is the run's error.
    Failed,
    /// An admin rolled back to the previous slot.
    RolledBack,
}

/// What one update attempt was: from, to, who asked, and where it landed.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub state: RunState,
    pub from: Pin,
    pub to: Pin,
    /// `manual` (an admin pressed the button) or `auto` (the scheduled
    /// check applied it behind the toggle).
    pub by: RunBy,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunBy {
    Manual,
    Auto,
}

/// The last registry check: what was available when we looked. `available`
/// is None when the check itself failed (the error sentence rides along).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRecord {
    pub at: String,
    pub available: Option<Pin>,
    pub error: Option<String>,
}

/// The row. Field order is the panel's reading order; camelCase so the
/// stored jsonb and the admin route's wire shape agree without a mapping
/// layer.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateState {
    /// OFF until an admin turns it on — see the header.
    pub auto_update: bool,
    /// True once adopt() has moved this instance to updater-owned slots.
    /// The engine refuses to act on an instance it never adopted; existing
    /// installs stay untouched forever.
    pub migrated: bool,
    /// The image this instance IS (adoption pins it; every roll re-pins).
    pub pinned: Option<Pin>,
    pub last_check: Option<CheckRecord>,
    pub last_run: Option<RunRecord>,
    pub history: Vec<RunRecord>,
    /// sha256 hex of the per-instance machine key (deploy-fleet.sh's
    /// x-talaria-key). The HASH, never the key — this row serializes to
    /// the panel, and the key must not ride along.
    pub machine_key_hash: Option<String>,
    /// The host port the edge owns, recorded at adoption: the slots publish
    /// nothing, so a later roll's self-inspect has no port to read — this
    /// is where the renderer learns it back.
    pub edge_port: Option<String>,
    /// The orchestrator container adoption retired (the dokploy app). Green
    /// removes it once the cutover lands — stopped by the finish, removed by
    /// the reconcile, so nothing resurrects it. Absent on normal rolls.
    pub retired_container: Option<String>,
}

/// Read the row. Absent/corrupt → the default (never migrated, never
/// auto-updating) — a corrupt row must not brick the panel, and the default
/// is the safe side of every gate.
pub async fn load(pg: &PgPool) -> UpdateState {
    match get_setting(pg, KEY, serde_json::Value::Null).await {
        v if v.is_null() => UpdateState::default(),
        v => serde_json::from_value(v).unwrap_or_default(),
    }
}

/// Read → patch → write, returning what landed. Single-writer discipline
/// comes from the roll lock (redis), not from this function — patches made
/// OUTSIDE a roll are check/toggle/bookkeeping writes, each idempotent
/// enough that a racing reader sees one or the other whole.
pub async fn patch<F>(pg: &PgPool, f: F) -> Result<UpdateState, sqlx::Error>
where
    F: FnOnce(UpdateState) -> UpdateState,
{
    let next = f(load(pg).await);
    set_setting(
        pg,
        KEY,
        &serde_json::to_value(&next).expect("update state serializes"),
    )
    .await?;
    Ok(next)
}

/// The history append every state transition funnels through: the newest
/// run at the head, the tail capped, and last_run kept in step so the panel
/// and the engine can never disagree about which run is current.
pub fn record_run(state: &mut UpdateState, run: RunRecord) {
    state.last_run = Some(run.clone());
    // Same run re-recorded (a state moving pulling → starting) replaces
    // its own head rather than stacking; distinct runs prepend.
    state.history.retain(|r| r.started_at != run.started_at);
    state.history.insert(0, run);
    state.history.truncate(HISTORY_CAP);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(v: &str) -> Pin {
        Pin {
            digest: format!("sha256:{:0>64}", v),
            version: v.into(),
        }
    }

    fn run(state: RunState, at: &str) -> RunRecord {
        RunRecord {
            state,
            from: pin("old"),
            to: pin("new"),
            by: RunBy::Manual,
            started_at: at.into(),
            finished_at: None,
            error: None,
        }
    }

    #[test]
    fn an_absent_row_is_the_safe_default() {
        let s: UpdateState = serde_json::from_value(serde_json::Value::Null).unwrap_or_default();
        assert!(
            !s.auto_update,
            "auto-update is off until an admin turns it on"
        );
        assert!(!s.migrated, "never adopted until adoption says so");
        assert!(s.pinned.is_none() && s.history.is_empty());
    }

    #[test]
    fn a_corrupt_row_falls_back_to_the_default() {
        let s: UpdateState = serde_json::from_value(serde_json::json!({
            "autoUpdate": true,
            "migrated": "not a boolean",
        }))
        .unwrap_or_default();
        assert_eq!(s, UpdateState::default(), "corrupt parses whole-row-safe");
    }

    #[test]
    fn record_run_replaces_its_own_head_and_caps_the_tail() {
        let mut s = UpdateState::default();
        record_run(&mut s, run(RunState::Pulling, "2026-09-03T10:00:00Z"));
        record_run(
            &mut s,
            RunRecord {
                state: RunState::Starting,
                ..run(RunState::Pulling, "2026-09-03T10:00:00Z")
            },
        );
        assert_eq!(s.history.len(), 1, "a state transition is not a new run");
        assert_eq!(s.history[0].state, RunState::Starting);
        assert_eq!(s.last_run.as_ref().unwrap().state, RunState::Starting);

        for i in 0..15 {
            record_run(
                &mut s,
                run(RunState::Done, &format!("2026-09-03T10:{:02}:00Z", i)),
            );
        }
        assert_eq!(s.history.len(), HISTORY_CAP);
        assert_eq!(
            s.history[0].started_at, "2026-09-03T10:14:00Z",
            "newest at the head"
        );
    }

    #[test]
    fn the_row_serializes_camel_case_for_the_wire() {
        let mut s = UpdateState::default();
        record_run(&mut s, run(RunState::CuttingOver, "2026-09-03T10:00:00Z"));
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("autoUpdate").is_some(), "camelCase on the wire");
        assert!(v.get("lastRun").is_some());
        assert_eq!(v["lastRun"]["state"], "cutting-over", "kebab-case states");
        assert_eq!(v["lastRun"]["by"], "manual");
        // Round-trip: the stored shape reads back as the same state.
        assert_eq!(serde_json::from_value::<UpdateState>(v).unwrap(), s);
    }
}
