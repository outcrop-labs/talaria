// Model Roles — which model handles each CLASS of activity. Port of
// model-roles.ts. Resolution contract: an assignment only wins while it still
// ROUTES on the gateway; otherwise callers fall back to their own heuristics
// (env default → pl-main → first routable, sonar preference scan), so a
// deleted model can never silently break a subsystem. Unset = auto.
//
// AUDIT 1.6: routable is not the same as FIT. Each role DECLARES the
// capabilities its work needs, and `role_assignment_issues` reports the
// assignments a model is known not to be able to serve. It reports; it does
// not drop. Silently ignoring an admin's explicit pick would trade one
// invisible failure for another, and the admin may well know something the
// probe suite does not.

use crate::capability::{capability_key, missing_capabilities};
use crate::gateway::registry::{resolve_route, routing_for};
use crate::gateway::settings::{get_setting, set_setting};
use futures_util::future::join_all;
use sqlx::PgPool;

pub struct RoleSpec {
    pub role: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    /// False = the slot is reserved for a surface that hasn't landed yet.
    pub wired: bool,
    /// What this role's WORK needs from a model (audit 1.6). Empty means the
    /// role genuinely runs on anything, which is a claim, not a shrug — see
    /// `utility`. Declared for reserved slots too, so the check is already
    /// right on the day the surface lands.
    pub requires: &'static [&'static str],
}

/// The role catalog, in the admin panel's row order. Labels and hints are the
/// product copy — verbatim from the TS table.
pub const MODEL_ROLES: [RoleSpec; 11] = [
    RoleSpec {
        role: "research-recon",
        label: "Research · Recon",
        hint: "Search stage for quick Recon passes. Needs a web-search-capable model. Auto: sonar.",
        wired: true,
        // The research pipeline's search stages are the whole point of these
        // three roles: a model without live search answers them from memory,
        // in the same confident shape, and the citations come out invented.
        requires: &["search"],
    },
    RoleSpec {
        role: "research-brief",
        label: "Research · Brief",
        hint: "Search stages behind Brief documents. Auto: sonar-pro.",
        wired: true,
        requires: &["search"],
    },
    RoleSpec {
        role: "research-expedition",
        label: "Research · Expedition",
        hint: "Search stages for deep Expedition runs. Auto: sonar-pro. Assigning a deep-research-class model (e.g. sonar-deep-research) makes each stage a full sweep: the engine runs fewer, bigger queries.",
        wired: true,
        requires: &["search"],
    },
    RoleSpec {
        role: "utility",
        label: "Utility",
        hint: "Background chores: catalog blurbs, chat distills, summaries, Muse fallback. A fast, cheap model is ideal. Auto: env default → pl-main → first routable.",
        wired: true,
        // Deliberately empty. Utility is the LAST link in nearly every
        // fallback chain in the codebase, so a requirement here would strand
        // titles, blurbs and distills on a small self-host. Per-harness floors
        // already refuse the handful of utility jobs that need more, which is
        // the right altitude: one harness declines, the rest keep working.
        requires: &[],
    },
    RoleSpec {
        role: "code-light",
        label: "Workbench · Light effort",
        hint: "Coding-harness runs for quick fixes and mechanical changes: agents pick the effort, this picks the model. A fast, cheap coder is ideal. Auto: utility chain.",
        wired: true,
        // `tools` is the load-bearing one and it is not a quality bar: the
        // model named here drives a CLI coding harness that reads and edits
        // files through tool calls, so without tool calling the run does not
        // degrade, it does nothing while reporting that it worked. `code` is
        // the quality half. Both apply at every effort — light effort is a
        // smaller job, not a lesser harness.
        requires: &["code", "tools"],
    },
    RoleSpec {
        role: "code-standard",
        label: "Workbench · Standard effort",
        hint: "The default coding-harness model for regular feature work. Auto: the light model, else the utility chain.",
        wired: true,
        requires: &["code", "tools"],
    },
    RoleSpec {
        role: "code-heavy",
        label: "Workbench · Heavy effort",
        hint: "The strongest coder for hard, cross-cutting work, used sparingly by design. Auto: the standard model.",
        wired: true,
        requires: &["code", "tools"],
    },
    RoleSpec {
        role: "vision",
        label: "Image understanding",
        hint: "Reserved: image inference for surfaces that analyze uploads without an agent persona.",
        wired: false,
        requires: &["vision"],
    },
    RoleSpec {
        role: "image-generation",
        label: "Image generation",
        hint: "Reserved: native image generation when a creative surface lands.",
        wired: false,
        // Empty because it is HONEST, not because nothing is needed: the
        // capability union has no member for image OUTPUT (`vision` is image
        // understanding, the opposite direction), and borrowing it would warn
        // about the wrong thing on every assignment.
        requires: &[],
    },
    RoleSpec {
        role: "embedding",
        label: "Embeddings",
        hint: "RAG embeddings run NATIVE on the self-hosted TEI service (TALARIA_EMBED_MODEL in compose; swap = reindex). This slot takes over if gateway-served embedding models ever land.",
        wired: false,
        requires: &[],
    },
    RoleSpec {
        role: "reranker",
        label: "Reranker",
        hint: "Rerank providers (self-hosted TEI, Voyage, Together, NVIDIA, Pinecone, ) are configured in Admin → Retrieval: provider APIs, not gateway models.",
        wired: false,
        requires: &[],
    },
];

const KEY: &str = "model_roles";

/// Raw assignments, unvalidated — the row rides to the wire exactly as stored
/// (TS returns getSetting's value with no filtering).
pub async fn get_model_roles(pg: &PgPool) -> serde_json::Value {
    get_setting(pg, KEY, serde_json::json!({})).await
}

/// Set or clear one role's assignment. Falsy model deletes the slot.
pub async fn set_model_role(
    pg: &PgPool,
    role: &str,
    model: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut cur = get_model_roles(pg).await;
    let obj = match cur.as_object_mut() {
        Some(o) => o,
        None => {
            // A non-object stored row is replaced wholesale, same as the TS
            // write over an equally broken value.
            cur = serde_json::json!({});
            cur.as_object_mut().unwrap()
        }
    };
    match model.filter(|m| !m.is_empty()) {
        Some(m) => {
            obj.insert(role.to_string(), serde_json::Value::String(m.to_string()));
        }
        None => {
            obj.remove(role);
        }
    }
    set_setting(pg, KEY, &cur).await
}

/// The explicitly assigned model for a role — but only while it still routes.
/// None means "auto": the caller applies its own fallback heuristic. A
/// database failure propagates (TS throws to the same 500).
///
/// AUDIT 1.6 asked whether this should also drop an assignment the model is
/// unfit for. It must NOT. An admin's explicit pick disappearing into the auto
/// chain is a second invisible failure, not a cure for the first, and probe
/// facts are evidence rather than truth. Fitness surfaces two other ways
/// instead, both of them visible: `role_assignment_issues` warns the admin at
/// the moment of assignment, and each harness's own floor refuses at run time
/// where a wrong answer would actually move a ticket.
pub async fn resolve_role_model(pg: &PgPool, role: &str) -> Result<Option<String>, sqlx::Error> {
    let Some(assigned) = get_model_roles(pg)
        .await
        .get(role)
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

// ── Fitness (audit 1.6) ──────────────────────────────────────────────────────

/// Plain words for what the admin loses, one clause per capability, written to
/// slot after the model id: "gpt-4o-mini has no web search, so …".
///
/// Partial on purpose. A capability no role requires needs no sentence, and
/// the fallback below stays truthful for one added later — a stale,
/// confidently wrong sentence would be worse than a plain one.
pub fn consequence_of(cap: &str) -> String {
    match cap {
        "search" => "has no web search, so research runs will answer from memory and the citations will be invented",
        "tools" => "cannot call tools, so a coding run cannot read or edit a single file",
        "code" => "is not a coder, so its patches will need more repair than they save",
        "vision" => "cannot read images, so anything sent to this slot comes back described from nothing",
        other => return format!("is known not to support {other}"),
    }
    .to_string()
}

/// Capabilities the assigned model is KNOWN to lack for this role. Empty when
/// the role requires nothing, when nothing routes the model, or — the
/// important one — when nobody has measured it. UNKNOWN IS NOT A LACK;
/// capability.rs owns that rule and it holds here for the same reason: a
/// fresh self-host has probed nothing, and an admin page that warned about
/// every model would teach people to ignore it.
///
/// Answered UNANIMOUSLY over the routing pool, exactly as the runner does it.
/// A bare model name can be served by several endpoints and capability is a
/// property of the endpoint, so a capability counts as missing only when
/// EVERY member says missing. `routing_for` rather than `resolve_route`,
/// because asking a question must not advance the round-robin cursor that
/// live traffic reads. Advisory, never load-bearing: a failed read means we
/// know nothing, and knowing nothing is not evidence.
pub async fn role_model_gaps(pg: &PgPool, role: &str, model: &str) -> Vec<String> {
    let Some(spec) = MODEL_ROLES.iter().find(|r| r.role == role) else {
        return Vec::new();
    };
    if spec.requires.is_empty() {
        return Vec::new();
    }
    let Ok(routing) = routing_for(pg, model).await else {
        return Vec::new();
    };
    if routing.endpoints.is_empty() {
        return Vec::new(); // unroutable: `resolve_role_model` already declines it
    }
    let mut missing: Vec<&str> = spec.requires.to_vec();
    for ep in &routing.endpoints {
        let here = missing_capabilities(
            pg,
            &capability_key(&ep.name, &routing.upstream_model),
            spec.requires,
        )
        .await;
        missing.retain(|cap| here.iter().any(|c| c == cap));
        if missing.is_empty() {
            break;
        }
    }
    missing.into_iter().map(String::from).collect()
}

/// One row of the admin panel's warnings, in the wire's key order.
pub struct RoleAssignmentIssue {
    pub role: String,
    /// The assigned model id, spelled as the admin picked it.
    pub model: String,
    /// Never empty — a role with no known gap produces no issue at all.
    pub missing: Vec<String>,
    /// One sentence for the admin UI, naming the capability in plain words.
    pub note: String,
}

impl RoleAssignmentIssue {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "role": self.role,
            "model": self.model,
            "missing": self.missing,
            "note": self.note,
        })
    }
}

/// Every role whose assigned model is known not to be able to do the work.
/// Empty is the normal answer, including on an install that has probed
/// nothing.
///
/// Reserved (`wired: false`) roles are included. Their surfaces do not exist
/// yet, so nothing is broken today, but telling an admin now that their pick
/// cannot see is strictly more useful than telling them the week the feature
/// ships — and the row already carries a "reserved" chip saying it is inert.
pub async fn role_assignment_issues(pg: &PgPool) -> Vec<RoleAssignmentIssue> {
    let assignments = get_model_roles(pg).await;
    // Concurrent because there are eleven roles and each gap check is two
    // small reads: serially that is an admin page load waiting out
    // twenty-two round trips for an answer that is almost always "no issues".
    // join_all preserves order, so the panel's rows stay in MODEL_ROLES order.
    let checked = join_all(MODEL_ROLES.iter().map(|spec| {
        // Borrow, don't move: the closure runs once per role, and an `async
        // move` block would consume `assignments` on the first call.
        let assignments = &assignments;
        async move {
        // Unset = auto, and the auto chains already reason about fitness (the
        // sonar preference scan, the utility fall-down). Nothing to warn about.
        let model = assignments.get(spec.role).and_then(|v| v.as_str())?;
        let missing = role_model_gaps(pg, spec.role, model).await;
        if missing.is_empty() {
            return None;
        }
        let note = format!(
            "{} {}. The assignment stands; set the role back to Auto if that is not what you meant.",
            model,
            missing
                .iter()
                .map(|c| consequence_of(c))
                .collect::<Vec<_>>()
                .join(", and ")
        );
        Some(RoleAssignmentIssue {
            role: spec.role.to_string(),
            model: model.to_string(),
            missing,
            note,
        })
        }
    }))
    .await;
    checked.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_role_catalog_is_the_product_copy() {
        assert_eq!(MODEL_ROLES.len(), 11);
        assert_eq!(MODEL_ROLES[0].role, "research-recon");
        assert_eq!(MODEL_ROLES[3].label, "Utility");
        assert!(MODEL_ROLES[3].requires.is_empty());
        assert_eq!(MODEL_ROLES[4].requires, &["code", "tools"]);
        assert!(!MODEL_ROLES[7].wired);
        assert_eq!(MODEL_ROLES[10].role, "reranker");
    }

    #[test]
    fn consequences_name_the_capability_as_plain_words() {
        assert_eq!(
            consequence_of("search"),
            "has no web search, so research runs will answer from memory and the citations will be invented"
        );
        // A capability added to the union later gets a truthful plain sentence.
        assert_eq!(
            consequence_of("long-context"),
            "is known not to support long-context"
        );
        let note = |missing: &[&str]| {
            format!(
                "gpt-4o-mini {}. The assignment stands; set the role back to Auto if that is not what you meant.",
                missing
                    .iter()
                    .map(|c| consequence_of(c))
                    .collect::<Vec<_>>()
                    .join(", and ")
            )
        };
        assert_eq!(
            note(&["search"]),
            "gpt-4o-mini has no web search, so research runs will answer from memory and the citations will be invented. The assignment stands; set the role back to Auto if that is not what you meant."
        );
        assert_eq!(
            note(&["code", "tools"]),
            "gpt-4o-mini is not a coder, so its patches will need more repair than they save, and cannot call tools, so a coding run cannot read or edit a single file. The assignment stands; set the role back to Auto if that is not what you meant."
        );
    }
}
