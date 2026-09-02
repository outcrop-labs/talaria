// Member access to gateway models, plus the picker catalog this module
// judges (gateway_models below). Admins register models; this decides which
// of them NON-admins may pick (preferred model, muse drafting). Empty list
// = all models (open by default, like agent access); non-empty = exactly
// those. Admins are never restricted — they control when the expensive
// brains run.

use crate::gateway::registry::list_endpoints;
use crate::gateway::settings::get_setting;
use sqlx::PgPool;
use std::collections::HashMap;

const KEY: &str = "member_model_allowlist";

/// Trim, drop empties, dedupe — insertion order preserved, which is the
/// order the admin UI saved.
pub async fn set_member_model_allowlist(pg: &PgPool, ids: &[String]) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let cleaned: Vec<String> = ids
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .collect();
    let _ = crate::gateway::settings::set_setting(pg, KEY, &serde_json::json!(cleaned)).await;
}

/// The picker's one-row-per-target catalog — GatewayModel. Serialization
/// order (id, endpoints, qualified) is the /api/models wire order.
#[derive(Debug, Clone)]
pub struct GatewayModel {
    pub id: String,
    pub endpoints: Vec<String>,
    /// True for "<endpoint>/<model>" pins. Bare model ids may themselves
    /// contain "/" (OpenRouter, HF-style names) — only this flag tells the
    /// two apart.
    pub qualified: bool,
}

impl GatewayModel {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "endpoints": self.endpoints,
            "qualified": self.qualified,
        })
    }
}

/// EVERY MODEL, SPELLED ONE WAY: `<endpoint>/<model>`.
///
/// THE QUALIFIED FORM IS THE CANONICAL ONE because it is the one that names
/// where the model runs — which is the thing Talaria actually measures
/// capability about (`capability_key` is `endpoint:model`, never a bare
/// name). A BARE ID SURVIVES ONLY WHERE IT MEANS SOMETHING ELSE: served by
/// more than one endpoint, it is the round-robin POOL, a distinct routing
/// target no single qualified id can express. One endpoint, and the bare
/// name is not a second target — it is a second name for the first.
///
/// Order is `localeCompare`'s, via [collating_cmp] below.
pub async fn gateway_models(pg: &PgPool) -> Result<Vec<GatewayModel>, sqlx::Error> {
    let eps = list_endpoints(pg).await?;
    let mut out = Vec::new();
    let mut bare: HashMap<String, Vec<String>> = HashMap::new();
    let mut bare_order: Vec<String> = Vec::new();
    for ep in &eps {
        for m in &ep.models {
            out.push(GatewayModel {
                id: format!("{}/{}", ep.name, m),
                endpoints: vec![ep.name.clone()],
                qualified: true,
            });
            if !bare.contains_key(m) {
                bare_order.push(m.clone());
            }
            bare.entry(m.clone()).or_default().push(ep.name.clone());
        }
    }
    for id in bare_order {
        if let Some(eps) = bare.get(&id)
            && eps.len() > 1
        {
            out.push(GatewayModel {
                id,
                endpoints: eps.clone(),
                qualified: false,
            });
        }
    }
    out.sort_by(|a, b| collating_cmp(&a.id, &b.id));
    Ok(out)
}

/// A STORED SPELLING ONTO THE OFFERED ONE. The
/// qualified form is the canonical id, but everything older than the catalog
/// that offered it — role pins, an archived fitness report, a member
/// allowlist — was written when both spellings existed, so plenty of it is
/// bare. Those values keep routing; what they must not do is fail to LINE UP
/// with the canonical row, or an admin sees a role assigned to a model that
/// appears nowhere in the list and a paid run whose verdict lights up no cell.
///
/// So this maps a stored spelling onto the offered one instead of rewriting
/// the database: cheaper than a migration, correct for values written after
/// it, and it cannot corrupt anything if the catalog is momentarily wrong.
/// Unresolvable ids come back unchanged — an id nothing serves is a fact to
/// show, not one to guess at.
pub fn canonical_model_id(id: &str, catalog: &[GatewayModel]) -> String {
    if catalog.iter().any(|m| m.id == id) {
        return id.to_string();
    }
    let suffix = format!("/{id}");
    let pins: Vec<&GatewayModel> = catalog
        .iter()
        .filter(|m| m.qualified && m.id.ends_with(&suffix))
        .collect();
    // Exactly one, or it is genuinely ambiguous — two endpoints serving the
    // same model is the pooled case, and picking one of them would silently
    // reassign a role to half of what it had.
    if pins.len() == 1 {
        pins[0].id.clone()
    } else {
        id.to_string()
    }
}

/// `String.prototype.localeCompare` — default locale, ICU root collation —
/// for the id alphabet Talaria actually registers (ASCII letters, digits,
/// `/ . - _`). Byte order is NOT it: case is a tertiary difference there, so
/// `Z.ai/glm-5.3` sorts at its lowercased position, after every `o...` id,
/// while a byte sort hoists the uppercase Z to the front. Fold case first;
/// where folding ties, lowercase wins (ICU's default case-first), and a full
/// tie falls back to bytes for a total order.
pub fn collating_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let fold = a
        .chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase));
    if fold != Ordering::Equal {
        return fold;
    }
    let case = a
        .chars()
        .map(|c| c.is_uppercase() as u8)
        .cmp(b.chars().map(|c| c.is_uppercase() as u8));
    if case != Ordering::Equal {
        return case;
    }
    a.cmp(b)
}

/// Bare model ids members may use. Empty = no restriction. Non-string
/// entries in a hand-edited row never match anything — only string entries
/// are read.
pub async fn member_model_allowlist(pg: &PgPool) -> Vec<String> {
    get_setting(pg, KEY, serde_json::Value::Array(vec![]))
        .await
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// May this role use this model id? Endpoint-qualified ids are judged by the
/// model they pin (allowing "m" allows "ep/m" on any endpoint).
pub fn model_allowed_for(
    role: &str,
    model: &str,
    allow: &[String],
    catalog: &[GatewayModel],
) -> bool {
    if role == "admin" || allow.is_empty() {
        return true;
    }
    if allow.iter().any(|a| a == model) {
        return true;
    }
    if let Some(entry) = catalog.iter().find(|m| m.id == model)
        && entry.qualified
    {
        // "ep/rest": allowed iff the pinned bare model is allowed.
        let rest = &model[model.find('/').map_or(0, |i| i + 1)..];
        return allow.iter().any(|a| a == rest);
    }
    false
}

/// The gateway catalog as this role may see it.
pub async fn gateway_models_for(pg: &PgPool, role: &str) -> Result<Vec<GatewayModel>, sqlx::Error> {
    let all = gateway_models(pg).await?;
    if role == "admin" {
        return Ok(all);
    }
    let allow = member_model_allowlist(pg).await;
    if allow.is_empty() {
        return Ok(all);
    }
    let full = all.clone();
    Ok(all
        .into_iter()
        .filter(|m| model_allowed_for(role, &m.id, &allow, &full))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gm(id: &str, qualified: bool) -> GatewayModel {
        GatewayModel {
            id: id.into(),
            endpoints: vec!["ep".into()],
            qualified,
        }
    }

    #[test]
    fn the_allowlist_judges_pins_by_their_bare_model() {
        let catalog = vec![
            gm("openrouter/m", true),
            gm("local/m", true),
            gm("m", false),
            gm("x", false),
        ];
        let allow = vec!["m".to_string()];
        // Admin or empty allow: everything.
        assert!(model_allowed_for("admin", "anything", &allow, &catalog));
        assert!(model_allowed_for("member", "anything", &[], &catalog));
        // A listed bare id, and any qualified pin of it.
        assert!(model_allowed_for("member", "m", &allow, &catalog));
        assert!(model_allowed_for(
            "member",
            "openrouter/m",
            &allow,
            &catalog
        ));
        assert!(model_allowed_for("member", "local/m", &allow, &catalog));
        // An unlisted bare id — even one present in the catalog — and an
        // unlisted id the catalog doesn't know at all.
        assert!(!model_allowed_for("member", "x", &allow, &catalog));
        assert!(!model_allowed_for(
            "member",
            "never-registered",
            &allow,
            &catalog
        ));
        // A qualified id whose REST half is not allowed.
        assert!(!model_allowed_for(
            "member",
            "openrouter/x",
            &allow,
            &catalog
        ));
    }
}
