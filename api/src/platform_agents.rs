// Platform sub-agents — Talaria's OWN workers, separate from the Hermes
// fleet. Which model powers each agent is configured GRANULARLY on
// Models → Platform; unset = the job's auto chain keeps working untouched.
// Port of the resolution half of platform-agents.ts. What was deferred with
// the admin panel's route is the panel's OWN row data (jobs, skills, the
// "Auto" prose); the join keys the harness registry cross-checks against —
// id, label, assignable — landed here with it, because a cross-check against
// a table that does not exist yet is a cross-check that cannot be written.
//
// Resolution contract mirrors model roles: an assignment only wins while it
// still ROUTES on the gateway — a deleted model can never silently break a
// subsystem, the job just falls back to its own chain.

use crate::gateway::registry::resolve_route;
use crate::gateway::settings::get_setting;
use sqlx::PgPool;

const KEY: &str = "platform_agent_models";

// ── The catalog (the join keys; the panel's rows cross with its route) ───────

/// One platform agent as the registry knows it. `assignable: false` means the
/// model is fixed by design — the briefer is the owner's own personal
/// assistant, and its persona and privacy are the feature — so no harness may
/// declare a pin on it and the Models → Platform page offers no slot.
pub struct PlatformAgent {
    pub id: &'static str,
    pub label: &'static str,
    pub assignable: bool,
}

pub const PLATFORM_AGENTS: &[PlatformAgent] = &[
    PlatformAgent {
        id: "muse",
        label: "Muse",
        assignable: true,
    },
    PlatformAgent {
        id: "distiller",
        label: "Distiller",
        assignable: true,
    },
    PlatformAgent {
        id: "concluder",
        label: "Concluder",
        assignable: true,
    },
    PlatformAgent {
        id: "blurb-writer",
        label: "Catalog writer",
        assignable: true,
    },
    PlatformAgent {
        id: "titler",
        label: "Titler",
        assignable: true,
    },
    PlatformAgent {
        id: "summarizer",
        label: "Summarizer",
        assignable: true,
    },
    PlatformAgent {
        id: "librarian",
        label: "Librarian",
        assignable: true,
    },
    PlatformAgent {
        id: "judge",
        label: "Judge",
        assignable: true,
    },
    PlatformAgent {
        id: "briefer",
        label: "Briefer",
        assignable: false,
    },
];

/// Current raw assignments (id → model), unvalidated.
pub async fn get_platform_agent_models(pg: &PgPool) -> serde_json::Value {
    get_setting(pg, KEY, serde_json::json!({})).await
}

/// The admin-assigned model for a platform agent — but only while it still
/// routes on the gateway. None = unassigned or stale: use the job's auto
/// chain. A database failure propagates (TS throws to the same 500).
///
/// NOTE: this resolves (and so ADVANCES the round-robin cursor), exactly as
/// the TS `resolveRoute(assigned)` does — intentional parity with live
/// traffic's routing behavior.
pub async fn platform_agent_model(pg: &PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
    let Some(assigned) = get_platform_agent_models(pg)
        .await
        .get(id)
        .and_then(|v| v.as_str())
        .map(String::from)
    else {
        return Ok(None);
    };
    if resolve_route(pg, &assigned).await?.is_none() {
        return Ok(None);
    }
    Ok(Some(assigned))
}
