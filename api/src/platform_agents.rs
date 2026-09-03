// Platform sub-agents — Talaria's OWN workers, separate from the Hermes
// fleet. Which model powers each agent is configured GRANULARLY on
// Models → Platform; unset = the job's auto chain keeps working untouched.
//
// Resolution contract mirrors model roles: an assignment only wins while it
// still ROUTES on the gateway — a deleted model can never silently break a
// subsystem, the job just falls back to its own chain.

use crate::gateway::registry::resolve_route;
use crate::gateway::settings::{get_setting, set_setting};
use serde_json::Value;
use sqlx::PgPool;

const KEY: &str = "platform_agent_models";

// ── The catalog (the join keys; the panel reads these rows) ──────────────────

/// One platform agent as the registry knows it. `assignable: false` means the
/// model is fixed by design — the briefer is the owner's own personal
/// assistant, and its persona and privacy are the feature — so no harness may
/// declare a pin on it and the Models → Platform page offers no slot.
pub struct PlatformAgent {
    pub id: &'static str,
    pub label: &'static str,
    pub job: &'static str,
    /// What its harness brings to the job — shown as chips in the panel.
    pub skills: &'static [&'static str],
    /// What "Auto" resolves to, in words.
    pub auto: &'static str,
    /// False = the model is fixed by design (e.g. the user's own assistant).
    pub assignable: bool,
}

impl PlatformAgent {
    /// The catalog row exactly as the admin panel reads it.
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "job": self.job,
            "skills": self.skills,
            "auto": self.auto,
            "assignable": self.assignable,
        })
    }
}

pub const PLATFORM_AGENTS: &[PlatformAgent] = &[
    PlatformAgent {
        id: "muse",
        label: "Muse",
        job: "The writing partner behind prompt-editing everywhere: documents, agent souls, and templates.",
        skills: &[
            "org voice",
            "register-aware drafting",
            "template skeleton harness",
        ],
        auto: "the requesting user's preferred model, else the Utility role chain",
        assignable: true,
    },
    PlatformAgent {
        id: "distiller",
        label: "Distiller",
        job: "Condenses idle agent chats into their durable substance before they archive \u{2014} what feeds each user\u{2019}s private brain.",
        skills: &[
            "transcript distillation",
            "decision & preference extraction",
        ],
        auto: "the chat owner's muse (their preference, else the Utility role chain)",
        assignable: true,
    },
    PlatformAgent {
        id: "concluder",
        label: "Concluder",
        job: "Writes the closing summary when a relay concludes: decisions, deliverables, follow-ups.",
        skills: &["multi-party synthesis", "action-item extraction"],
        auto: "the concluding user's muse (their preference, else the Utility role chain)",
        assignable: true,
    },
    PlatformAgent {
        id: "blurb-writer",
        label: "Catalog writer",
        job: "Keeps the model catalog human: one-line plain-language blurbs for every registered model.",
        skills: &["plain-language descriptions", "org-profile awareness"],
        auto: "the Utility role chain",
        assignable: true,
    },
    PlatformAgent {
        id: "titler",
        label: "Titler",
        job: "Names things as they take shape: chats and plans after their first exchange, research runs from their question.",
        skills: &["concise naming", "never clobbers user-chosen names"],
        auto: "the Utility role chain (a fast, cheap model is ideal)",
        assignable: true,
    },
    PlatformAgent {
        id: "summarizer",
        label: "Summarizer",
        job: "Keeps the Studio readable: one plain line per skill saying what it teaches, regenerated only when the skill changes.",
        skills: &["one-line gist extraction", "content-hash change detection"],
        auto: "the Utility role chain (a fast, cheap model is ideal)",
        assignable: true,
    },
    PlatformAgent {
        id: "librarian",
        label: "Librarian",
        job: "Maintains each knowledge space\u{2019}s OKF digest: summaries and links of the promoted documents, regenerated as promotions change.",
        skills: &[
            "knowledge digestion",
            "summaries with links",
            "autonomous upkeep",
        ],
        auto: "the Utility role chain",
        assignable: true,
    },
    PlatformAgent {
        id: "judge",
        label: "Judge",
        job: "Reviews agents\u{2019} reported ticket outcomes against the ask: verdicts and findings on boards with judging on.",
        skills: &["outcome verification", "structured verdicts"],
        auto: "pl-main when judging is enabled without a pick",
        assignable: true,
    },
    PlatformAgent {
        id: "ticket-relevance",
        label: "Ticket gate",
        job: "Reads each message on a ticket\u{2019}s discussion and decides whether it concerns the assigned agent\u{2019}s work — the agent replies only to what is its business.",
        skills: &["cheap relevance calls", "fail-open gating"],
        auto: "the Utility role chain (a fast, cheap model is ideal)",
        assignable: true,
    },
    PlatformAgent {
        id: "briefer",
        label: "Briefer",
        job: "Writes your daily brief every morning and follows it through the day as it moves.",
        skills: &[
            "scope-aware summarizing",
            "the append-only document contract",
        ],
        auto: "always the user\u{2019}s personal assistant; its persona and privacy are the point",
        assignable: false,
    },
];

/// Current raw assignments (id → model), unvalidated.
pub async fn get_platform_agent_models(pg: &PgPool) -> serde_json::Value {
    get_setting(pg, KEY, serde_json::json!({})).await
}

/// Assign or clear one platform agent's model.
pub async fn set_platform_agent_model(
    pg: &PgPool,
    id: &str,
    model: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut cur = get_platform_agent_models(pg).await;
    let obj = match &mut cur {
        Value::Object(o) => o,
        _ => return Ok(()), // a corrupt setting is never writable-shape here
    };
    match model.filter(|m| !m.is_empty()) {
        Some(m) => {
            obj.insert(id.to_string(), Value::String(m.to_string()));
        }
        None => {
            obj.remove(id);
        }
    }
    set_setting(pg, KEY, &cur).await
}

/// The admin-assigned model for a platform agent — but only while it still
/// routes on the gateway. None = unassigned or stale: use the job's auto
/// chain. A database failure propagates.
///
/// NOTE: this resolves (and so ADVANCES the round-robin cursor) — the same
/// routing behavior live traffic sees.
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
