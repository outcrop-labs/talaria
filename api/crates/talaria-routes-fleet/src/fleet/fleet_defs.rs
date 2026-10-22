// GET /api/fleet/defs. The harness registry: agent definitions (latest
// version inline) + LLM endpoints + brain routability + toolset vocabulary.
//
// Who sees what: `agents.manage` (and admins) read the whole fleet; everyone
// else reads the agents they MANAGE and nothing more, because being named a
// manager is the grant — it should not also need an org-wide permission
// (docs/PERMISSIONS.md, "Agent managers"). Someone who holds neither reads
// nothing, and `canHire` tells the roster which of the two empty states to
// render: "no agents yet" or "not available to you".
//
// A manager who is not fleet-wide gets the endpoints PARED to what the model
// picker needs — id, name, provider, class, context, models, effort ladders.
// The config surface includes infra layout (base urls, key env names, the
// price sheet), and managing one agent is not a reason to be shown the
// instance's wiring.
//
// Each def carries its `managers` and the caller's own `canManage`, so the
// roster and the manage modal gate their affordances on the same answer the
// write routes will give.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use talaria_agent_defs::{
    GOVERNED_TOOLSETS, MANDATORY_DISABLED, list_agent_defs_wire, mandatory_disabled,
};
use talaria_agent_managers::{managed_agent_ids, managers_by_agent, reads_whole_fleet};
use talaria_api_facades::gateway::registry::list_endpoints_wire;
use talaria_brain_health::fleet_brain_health;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

/// The model picker's fields, and only those.
const PICKER_FIELDS: [&str; 7] = [
    "id",
    "name",
    "provider",
    "class",
    "contextLength",
    "models",
    "modelEfforts",
];

fn pare_endpoint(e: &Value) -> Value {
    let mut out = Map::new();
    for key in PICKER_FIELDS {
        if let Some(v) = e.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    Value::Object(out)
}

/// The toolset vocabulary, straight from the crates that own it: `governed`
/// is every Hermes toolset an operator may name ([`GOVERNED_TOOLSETS`]) and
/// `mandatoryDisabled` is the WHOLE floor the renderer merges ahead of any
/// def — [`MANDATORY_DISABLED`] plus whatever the org floor closes
/// (`talaria_agent_defs::mandatory_disabled`), which the caller reads once
/// and hands in. `orgDisabled` is the org part on its own, so the editor can
/// say WHY a locked row is locked: Talaria's own tool for the pair, an admin
/// for the rest.
///
/// It rides on the roster because the editor needs it the moment the roster
/// renders, and it is READ from the constants rather than spelled again here:
/// the PATCH refuses a name outside that list, so a hand-kept copy would be a
/// picker offering toolsets the write door rejects the day Hermes grows one.
///
/// `governed` is sorted on the way out. The constant is kept in `toolsets.py`'s
/// order — alphabetical today, and not something this route gets to require of
/// it — while the wire promise to the editor is a list it can paint top to
/// bottom without sorting it again.
fn toolsets_block(floor: &[String]) -> Value {
    let mut governed: Vec<&str> = GOVERNED_TOOLSETS.to_vec();
    governed.sort_unstable();
    let org: Vec<&str> = floor
        .iter()
        .map(String::as_str)
        .filter(|n| !MANDATORY_DISABLED.contains(n))
        .collect();
    json!({
        "governed": governed,
        "mandatoryDisabled": floor,
        "orgDisabled": org,
    })
}

// doc: The roster also carries `toolsets`, the agent editor's vocabulary:
// doc: `governed`, every Hermes toolset an operator may disable on an agent
// doc: (22 names, alphabetical); `mandatoryDisabled`, the whole floor the
// doc: renderer closes on every agent whatever its def says — `todo` and
// doc: `clarify`, because Talaria's own MCP versions are the ones a person
// doc: sees, plus the org-wide floor an admin sets on `PUT
// doc: /api/admin/settings` (`mandatoryDisabledToolsets`) or pins with
// doc: `TALARIA_MANDATORY_DISABLED_TOOLSETS`; and `orgDisabled`, that org
// doc: part alone, so the editor can say an admin closed a row rather than
// doc: Talaria. `governed` is the constant the `PATCH /api/fleet/defs/{id}`
// doc: validator reads, so the picker cannot offer a name the write door
// doc: would refuse.
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let fleet_wide = match reads_whole_fleet(&state.pg, &user.id, &user.role).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[fleet] permission read failed", e)),
    };
    let mine = match managed_agent_ids(&state.pg, &user.id).await {
        Ok(ids) => ids,
        Err(e) => return Ok(internal("[fleet] managed_agent_ids failed", e)),
    };
    let defs = match list_agent_defs_wire(&state.pg).await {
        Ok(d) => d,
        Err(e) => return Ok(internal("[fleet] list_agent_defs_wire failed", e)),
    };
    let managers = match managers_by_agent(&state.pg).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[fleet] managers_by_agent failed", e)),
    };
    let mut visible_models: std::collections::HashSet<String> = std::collections::HashSet::new();
    let defs: Vec<Value> = defs
        .into_iter()
        .filter_map(|mut d| {
            let id = d
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let can_manage = user.role == "admin" || mine.contains(&id);
            if !fleet_wide && !can_manage {
                return None;
            }
            if let Some(m) = d.get("model").and_then(Value::as_str) {
                visible_models.insert(m.to_string());
            }
            let rows = managers.get(&id).cloned().unwrap_or_default();
            if let Some(obj) = d.as_object_mut() {
                obj.insert(
                    "managers".into(),
                    serde_json::to_value(rows).unwrap_or(Value::Null),
                );
                obj.insert("canManage".into(), Value::Bool(can_manage));
            }
            Some(d)
        })
        .collect();
    let endpoints = match list_endpoints_wire(&state.pg).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[fleet] list_endpoints_wire failed", e)),
    };
    let endpoints: Vec<Value> = if fleet_wide {
        endpoints
    } else if defs.is_empty() {
        // Nothing to configure, nothing to configure it with. A member who
        // manages no agent has no business reading the instance's model
        // list, pared or not.
        Vec::new()
    } else {
        endpoints.iter().map(pare_endpoint).collect()
    };
    let brains = match fleet_brain_health(&state.pg).await {
        Ok(b) => b,
        Err(e) => return Ok(internal("[fleet] fleet_brain_health failed", e)),
    };
    let brains = if fleet_wide {
        brains
    } else {
        brains
            .into_iter()
            .filter(|b| visible_models.contains(&b.agent))
            .collect()
    };
    // The floor is read here, once per roster, so the editor locks the same
    // rows the next render will close.
    let floor = mandatory_disabled(&state.pg).await;
    // The endpoints' prices ride as numbers — a whole $/MTok price prints the
    // JS way (`3`, never `3.0`).
    let mut body = json!({
        "defs": defs,
        "endpoints": endpoints,
        "brains": brains,
        "canHire": fleet_wide,
        "toolsets": toolsets_block(&floor),
    });
    talaria_body::js_numberify(&mut body);
    Ok(Json(body).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &Value) -> Vec<String> {
        v.as_array()
            .expect("a list of toolset names")
            .iter()
            .map(|n| n.as_str().expect("a toolset name").to_string())
            .collect()
    }

    fn wire(floor: &[&str]) -> Value {
        let floor: Vec<String> = floor.iter().map(|n| n.to_string()).collect();
        let mut body = json!({ "toolsets": toolsets_block(&floor) });
        talaria_body::js_numberify(&mut body);
        serde_json::from_str(&serde_json::to_string(&body).expect("the block serialises"))
            .expect("the block round-trips")
    }

    // The vocabulary reaches the wire whole, alphabetical, with the floor
    // nameable inside it — the three things the editor assumes when it paints
    // a row per governed toolset and locks two of them. Serialised and read
    // back, because what the editor gets is JSON, not a &[&str].
    #[test]
    fn the_serialised_roster_carries_the_governed_vocabulary_and_the_floor() {
        // A fresh install's floor: the pair, and no org part.
        let wire = wire(&["todo", "clarify"]);
        let governed = names(&wire["toolsets"]["governed"]);
        assert_eq!(governed.len(), 28, "28 governed toolsets, got {governed:?}");
        let mut alphabetical = governed.clone();
        alphabetical.sort();
        assert_eq!(governed, alphabetical, "governed ships alphabetical");

        let mandatory = names(&wire["toolsets"]["mandatoryDisabled"]);
        assert_eq!(mandatory, ["todo", "clarify"]);
        for name in &mandatory {
            assert!(governed.contains(name), "{name} is not governed");
        }
        assert_eq!(
            names(&wire["toolsets"]["orgDisabled"]),
            Vec::<String>::new()
        );
    }

    // An org floor rides INSIDE `mandatoryDisabled` — the editor locks every
    // name there without knowing where each came from — and `orgDisabled`
    // carries the org part alone, for the row's reason.
    #[test]
    fn an_org_floor_is_part_of_the_floor_and_named_on_its_own() {
        let wire = wire(&["todo", "clarify", "terminal", "browser"]);
        assert_eq!(
            names(&wire["toolsets"]["mandatoryDisabled"]),
            ["todo", "clarify", "terminal", "browser"]
        );
        assert_eq!(
            names(&wire["toolsets"]["orgDisabled"]),
            ["terminal", "browser"]
        );
    }
}
