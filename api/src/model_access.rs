// Member access to gateway models — port of model-access.ts, plus
// llm-gateway.ts's gatewayModels (the picker catalog this module judges).
// Admins register models; this decides which of them NON-admins may pick
// (preferred model, muse drafting). Empty list = all models (open by
// default, like agent access); non-empty = exactly those. Admins are never
// restricted — they control when the expensive brains run.

use crate::gateway::registry::list_endpoints;
use crate::gateway::settings::get_setting;
use sqlx::PgPool;
use std::collections::HashMap;

const KEY: &str = "member_model_allowlist";

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

/// EVERY MODEL, SPELLED ONE WAY: `<endpoint>/<model>` — port of gatewayModels.
///
/// THE QUALIFIED FORM IS THE CANONICAL ONE because it is the one that names
/// where the model runs — which is the thing Talaria actually measures
/// capability about (`capability_key` is `endpoint:model`, never a bare
/// name). A BARE ID SURVIVES ONLY WHERE IT MEANS SOMETHING ELSE: served by
/// more than one endpoint, it is the round-robin POOL, a distinct routing
/// target no single qualified id can express. One endpoint, and the bare
/// name is not a second target — it is a second name for the first.
///
/// Same recorded divergence as the /llm/v1 catalog: TS sorts `localeCompare`,
/// this is byte order — agrees on ASCII ids, which is every registered id's
/// neighborhood, and the order is not contractual.
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
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Bare model ids members may use. Empty = no restriction. Non-string
/// entries in a hand-edited row never match anything, same as TS's
/// `allow.includes(model)` on a mixed array.
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
