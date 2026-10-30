// /api/fleet/defs/{id}/edit. POST → save an edit as a NEW immutable version
// (and optionally apply it to the running managed container). Admin. This is
// "versioned agent internals": nothing shifts silently — every change is a
// version you can diff and revert.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use talaria_agent_defs::{
    AliasTarget, CompressionEdits, ConfigEdits, Knob, ModelTarget, NewVersion,
    add_version_if_changed, apply_config_edits, list_versions,
};
use talaria_api_facades::fleet::docker::{ANSWER_WITHIN, Detached, run_detached};
use talaria_api_facades::fleet::reconcile::apply_agent_change;
use talaria_api_facades::gateway::provider::catalog_models;
use talaria_api_facades::gateway::registry::{LlmEndpoint, add_endpoint_models, list_endpoints};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    NumKind, array_msg, array_too_big_msg, nullable_number_member, optional_boolean_member,
    optional_max_string_member, parse, string_member, string_msg, too_big_msg, too_small_msg,
    utf16_len, zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400, store_answer};
use talaria_session::{actor_of, require_agent_manager, secretbox_or_500};
use talaria_state::AppState;

/// One Target: `{ endpoint, model, contextLength?, effort? }`. The effort is
/// NOT validated against the model's published levels here: the editor only
/// offers real levels, and every runtime consumer re-validates against the
/// live metadata — a level that went stale between save and use is inert,
/// never a refused turn.
fn parse_target(m: &Map<String, Value>) -> Result<ModelTarget, String> {
    let endpoint = string_member(m, "endpoint", 1, 100)?;
    let model = string_member(m, "model", 1, 200)?;
    let context_length = match m.get("contextLength") {
        None => None,
        Some(Value::Null) => return Err("Invalid input: expected number, received null".into()),
        Some(_) => {
            // the bound is exclusive (>0), which the folded helper's
            // >= min cannot say.
            let n =
                nullable_number_member(m, "contextLength", NumKind::Int, f64::MIN, f64::INFINITY)?
                    .ok_or("unreachable: present non-null read above")?;
            if n <= 0.0 {
                return Err("Too small: expected number to be >0".into());
            }
            Some(n as i64)
        }
    };
    let effort = match m.get("effort") {
        None => None,
        Some(Value::Null) => None,
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            if s.chars().count() < 1 {
                return Err(too_small_msg(1));
            }
            if utf16_len(s) > 24 {
                return Err(too_big_msg(24));
            }
            Some(s.to_string())
        }
    };
    Ok(ModelTarget {
        endpoint,
        model,
        context_length,
        effort,
    })
}

/// What the answer says about a ratio Hermes will not honor as written. The
/// save is NOT refused: 0.75 is Hermes' floor for models under 512K of
/// context and nothing at all above that, so refusing the number would lie
/// about the big-context case. The edit lands and the caller is told.
const HERMES_FLOOR_NOTE: &str =
    "Hermes raises a threshold under 0.75 to 0.75 for models with less than 512K context";

/// One nullish number member as a three-way knob: absent leaves the stored
/// key alone, null drops it, a number writes it. The bounds and their messages
/// are the shared helper's, so a knob refuses in the same words every other
/// number on this door does.
fn knob_member<T>(
    m: &Map<String, Value>,
    key: &str,
    kind: NumKind,
    min: f64,
    max: f64,
    cast: impl Fn(f64) -> T,
) -> Result<Knob<T>, String> {
    match m.get(key) {
        None => Ok(Knob::Keep),
        Some(Value::Null) => Ok(Knob::Clear),
        Some(_) => {
            let n = nullable_number_member(m, key, kind, min, max)?
                .ok_or("unreachable: present non-null read above")?;
            Ok(Knob::Set(cast(n)))
        }
    }
}

/// `compression?: { threshold?: number | null, thresholdTokens?: number | null }`
/// — the two compaction knobs, plus whatever the answer should say about them.
/// An absent (or null) member says nothing about compression and the def's
/// own block rides through untouched; every other key in that block is the
/// author's either way.
fn parse_compression(
    obj: &Map<String, Value>,
    key: &str,
) -> Result<(CompressionEdits, Vec<String>), String> {
    let Some(v) = obj.get(key).filter(|v| !v.is_null()) else {
        return Ok((CompressionEdits::default(), Vec::new()));
    };
    let m = v
        .as_object()
        .ok_or_else(|| talaria_body::object_msg(zod_type_name(v)))?;
    let threshold = knob_member(m, "threshold", NumKind::Float, 0.1, 1.0, |n| n)?;
    let threshold_tokens = knob_member(
        m,
        "thresholdTokens",
        NumKind::Int,
        1_000.0,
        10_000_000.0,
        |n| n as i64,
    )?;
    let mut notes = Vec::new();
    if let Knob::Set(ratio) = threshold
        && ratio < 0.75
    {
        notes.push(HERMES_FLOOR_NOTE.to_string());
    }
    Ok((
        CompressionEdits {
            threshold,
            threshold_tokens,
        },
        notes,
    ))
}

/// A required Target member (`main: Target`) — an object, not an array.
fn target_member(obj: &Map<String, Value>, key: &str) -> Result<ModelTarget, String> {
    let v = obj
        .get(key)
        .ok_or_else(|| talaria_body::object_msg("undefined"))?;
    let m = v
        .as_object()
        .ok_or_else(|| talaria_body::object_msg(zod_type_name(v)))?;
    parse_target(m)
}

/// A required array of Targets (`aliases` adds the tier `name`). Items
/// validate in order — each member, then the array's own length bound.
fn target_array<T>(
    obj: &Map<String, Value>,
    key: &str,
    max_items: usize,
    mut item: impl FnMut(&Map<String, Value>) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let v = obj.get(key).ok_or_else(|| array_msg("undefined"))?;
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let m = el
            .as_object()
            .ok_or_else(|| talaria_body::object_msg(zod_type_name(el)))?;
        out.push(item(m)?);
    }
    if arr.len() > max_items {
        return Err(array_too_big_msg(max_items));
    }
    Ok(out)
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    // When the request began: the apply below answers within what is left of
    // the bound (`bounded_apply`).
    let started = std::time::Instant::now();
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let soul = match string_member(obj, "soul", 0, 200_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let main = match target_member(obj, "main") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let aliases = match target_array(obj, "aliases", 20, |m| {
        Ok(AliasTarget {
            name: string_member(m, "name", 1, 60)?,
            target: parse_target(m)?,
        })
    }) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let fallbacks = match target_array(obj, "fallbacks", 10, parse_target) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let note = match optional_max_string_member(obj, "note", 300) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Apply the edit to the running agent now: re-render (config and soul land in place) and
    // restart the managed container only if the edit changed something it reads only at start.
    let apply = match optional_boolean_member(obj, "apply") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    }
    .unwrap_or(false);
    // The compaction knobs, and what the answer owes the caller about them.
    // note: `compression` — `{ threshold?: number|null (0.1..1), thresholdTokens?: number|null (1000..10000000) }`; null clears a knob, absent leaves it; a threshold under 0.75 saves and the answer's `notes` says Hermes raises it for models under 512K context
    let (compression, mut notes) = match parse_compression(obj, "compression") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }

    let def: Option<(String, String, bool, String, String, bool)> = match sqlx::query_as(
        "select id::text, department, managed, display_name, slug, enabled from agent_defs where id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(row) => row,
        Err(e) => return Ok(internal("[fleet/defs/edit] def read failed", e)),
    };
    let Some((def_id, department, managed, display_name, slug, enabled)) = def else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };

    let versions = match list_versions(&state.pg, &def_id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[fleet/defs/edit] versions read failed", e)),
    };
    let Some(latest) = versions.first() else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "no base version — import first",
        ));
    };

    // A picked model must actually route: auto-register it on its endpoint
    // when the provider's LIVE catalog serves it (no maintained lists — the
    // registration follows the choice), refuse clearly when it doesn't.
    // Without this, an unregistered model renders fine and the agent then
    // dies with a gateway 404 on its first turn — a silent-freeze chat.
    let endpoints: Vec<LlmEndpoint> = match list_endpoints(&state.pg).await {
        Ok(eps) => eps,
        Err(e) => return Ok(internal("[fleet/defs/edit] endpoints read failed", e)),
    };
    let mut endpoints: HashMap<String, LlmEndpoint> =
        endpoints.into_iter().map(|e| (e.name.clone(), e)).collect();
    let targets = std::iter::once((&main.endpoint, &main.model))
        .chain(
            aliases
                .iter()
                .map(|a| (&a.target.endpoint, &a.target.model)),
        )
        .chain(fallbacks.iter().map(|f| (&f.endpoint, &f.model)));
    for (endpoint, model) in targets {
        let Some(ep) = endpoints.get(endpoint) else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!("endpoint \"{endpoint}\" does not exist"),
            ));
        };
        if ep.models.iter().any(|m| m == model) {
            continue;
        }
        let live: Option<Vec<String>> = match catalog_models(&state, ep).await {
            Ok(catalog) => Some(catalog.into_iter().map(|m| m.id).collect()),
            // an unreachable catalog is "cannot vouch", not an error.
            Err(_) => None,
        };
        if live
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|m| m == model))
        {
            let ep = endpoints.get_mut(endpoint).expect("present above");
            if let Err(e) =
                add_endpoint_models(&state.pg, &ep.name, std::slice::from_ref(model)).await
            {
                return Ok(internal(
                    "[fleet/defs/edit] endpoint model register failed",
                    e,
                ));
            }
            ep.models.push(model.to_string());
        } else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "\"{model}\" is not registered on \"{endpoint}\"{} — pick it on /models first",
                    if live.is_some() {
                        " and its live catalog does not list it"
                    } else {
                        ""
                    }
                ),
            ));
        }
    }

    // apply_config_edits' refusal ("cannot edit what has no models" and
    // friends) is the 400; its registry re-read failing (the auto-register
    // above dropped the cache) is the store's fault sentence → the house 500,
    // and so is the version write beneath it.
    let edits = ConfigEdits {
        main,
        aliases,
        fallbacks,
        compression,
    };
    let config = match apply_config_edits(&state.pg, &latest.config, &edits).await {
        Ok(c) => c,
        Err(msg) => return Ok(store_answer(StatusCode::BAD_REQUEST, &msg)),
    };
    let created_by = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "admin".into());
    let note = note.unwrap_or_else(|| "edited in Talaria".into());
    let (version, created) = match add_version_if_changed(
        &state.pg,
        &def_id,
        &NewVersion {
            soul: &soul,
            config: &config,
            note: Some(&note),
            created_by: Some(&created_by),
        },
    )
    .await
    {
        Ok(vc) => vc,
        Err(e) => return Ok(internal("[fleet/defs/edit] version write failed", e)),
    };
    if created {
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.edit",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&display_name),
                before: None,
                after: Some(json!({ "version": version })),
            },
        )
        .await;
    }
    // A Save & apply with nothing changed creates no version. Whether it has
    // anything to apply is the agent's to say (`version_in_effect`): quiet
    // when the agent runs this version (or, stopped, will on its next
    // start), and APPLIED when it does not — a version saved without an
    // apply, or a roll that never landed. It used to warn and apply nothing,
    // so the one button that says "apply" could not put the current version
    // on; the person had to find Roll. Now it renders and rolls only what
    // moved, the same apply a new version gets. Two cases still answer a
    // warning instead: a retired agent (nothing runs it, and an apply would
    // read as one that landed) and docker not saying whether it runs (the
    // apply would read the same silence and leave everything as it is).
    let mut reapply = false;
    if !created && apply && managed {
        let warning = match talaria_agent_defs::runner_instead_of(&state.pg, &def_id, &department)
            .await
        {
            Ok(Some(runner)) => Some(talaria_agent_defs::shared_department_refusal(
                "roll",
                &display_name,
                &department,
                &runner.display_name,
            )),
            Ok(None) => {
                match version_in_effect(&state, &slug, &department, &display_name, enabled, version)
                    .await
                {
                    InEffect::Yes => None,
                    InEffect::No(why) => {
                        tracing::info!(
                            "[fleet/defs/edit] a Save & apply with nothing changed: {display_name} does not run version {version} ({why}) — applying it"
                        );
                        reapply = true;
                        None
                    }
                    InEffect::Unsure(why) => Some(why),
                }
            }
            Err(e) => {
                return Ok(internal(
                    "[fleet/defs/edit] department share read failed",
                    e,
                ));
            }
        };
        if !reapply {
            let mut out = json!({
                "ok": true,
                "version": version,
                "created": created,
                "applied": false,
                "inEffect": warning.is_none(),
                "notes": notes,
            });
            if let Some(warning) = warning {
                out["warning"] = json!(warning);
            }
            return Ok(Json(out).into_response());
        }
        // This apply can recreate the agent's container, as the Roll it
        // stands in for does, and no `agent.edit` is written for it (no
        // version was made): audited here, before the act, as the control
        // route audits a roll, so the trail says who put the version on.
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: "agent.apply",
                target_type: "agent",
                target_id: Some(&def_id),
                target_label: Some(&display_name),
                before: None,
                after: Some(json!({ "version": version, "reapply": true })),
            },
        )
        .await;
    }
    if created && apply && managed {
        // ONE CONTAINER PER DEPARTMENT. The apply below reaches the container
        // by department, and where an older agent holds this one's too (a
        // pair from before a department became one agent's) that container
        // is the other agent's: the render leaves this one out, so nothing of
        // this version runs anywhere, and applying would restart the other
        // agent for an edit that is not its own. The version stands; the
        // answer says it was not applied, and why, in `warning` — the one
        // field every apply that did not land answers, and the one the Config
        // form shows.
        match talaria_agent_defs::runner_instead_of(&state.pg, &def_id, &department).await {
            Ok(Some(runner)) => {
                let warning = talaria_agent_defs::shared_department_refusal(
                    "roll",
                    &display_name,
                    &department,
                    &runner.display_name,
                );
                return Ok(Json(json!({
                    "ok": true,
                    "version": version,
                    "created": created,
                    "applied": false,
                    "warning": warning,
                    "notes": notes,
                }))
                .into_response());
            }
            Ok(None) => {}
            Err(e) => {
                return Ok(internal(
                    "[fleet/defs/edit] department share read failed",
                    e,
                ));
            }
        }
    }
    let mut applied = false;
    if (created || reapply) && apply && managed {
        // Apply, don't roll by default: the render rewrites config.yaml and
        // SOUL.md in place, and the agent restarts only when the edit moved
        // its container — a key Hermes reads only at start, an env value.
        // An edit to the soul alone reaches the agent's next new session
        // without interrupting anyone; one that does roll takes the agent
        // offline for about a minute, and a turn in flight waits for it.
        let sb = secretbox_or_500(&state, "[fleet/defs/edit] secretbox unavailable").await?;
        // On its own task, and answered within a bound (`bounded_apply`):
        // a roll's drain, stop and recreate plus its 180 s health wait can
        // outlast the app proxy's 300 s, which answered the form a 502 for a
        // save that had landed. Past the bound the answer is a 202 that says
        // the version is saved and the apply goes on.
        let verdict = match bounded_apply(
            &state,
            sb,
            &department,
            ANSWER_WITHIN.saturating_sub(started.elapsed()),
            "[fleet/defs/edit]",
        )
        .await
        {
            BoundedApply::Settled(verdict) => verdict,
            BoundedApply::StillApplying => {
                return Ok((
                    StatusCode::ACCEPTED,
                    Json(json!({
                        "ok": true,
                        "version": version,
                        "created": created,
                        "applied": false,
                        "pending": true,
                        "notes": notes,
                    })),
                )
                    .into_response());
            }
        };
        match verdict {
            Ok(None) => {
                applied = true;
                // The roll recreated the container on this version, and it
                // is still booting past the roll's health wait: applied, and
                // the roster's roll chip reads "still starting" until it is
                // up — said here too, since the form closes on an apply.
                if talaria_api_facades::fleet::reconcile::still_starting(&department) {
                    notes.push(format!(
                        "{display_name} is still starting on version {version}: its recreated container has not reported healthy yet — the roster shows when it is up"
                    ));
                }
            }
            Ok(Some(warning)) => {
                let mut out = json!({
                    "ok": true,
                    "version": version,
                    "created": created,
                    "applied": false,
                    "warning": warning,
                    "notes": notes,
                });
                if reapply {
                    out["inEffect"] = json!(false);
                }
                return Ok(Json(out).into_response());
            }
            // A step that threw (the render, the image pull, the recreate)
            // is an apply that did not land too, and the version above is
            // already written: the same answer as a soft failure, the reason
            // in `warning`. A bare 400 here said nothing of the saved
            // version, and a second Save & apply of the same draft used to
            // be a no-op, so the form closed on it. (`bounded_apply` logged
            // the error.)
            Err(e) => {
                let mut out = json!({
                    "ok": true,
                    "version": version,
                    "created": created,
                    "applied": false,
                    "warning": format!("the roll failed — {e}"),
                    "notes": notes,
                });
                if reapply {
                    out["inEffect"] = json!(false);
                }
                return Ok(Json(out).into_response());
            }
        }
    }
    // `notes` is always present and usually empty — one shape for the client,
    // and the only thing in it today is what Hermes will do to a low ratio.
    let mut out = json!({
        "ok": true,
        "version": version,
        "created": created,
        "applied": applied,
        "notes": notes,
    });
    // A Save & apply with nothing changed that applied the current version:
    // the agent runs it now.
    if reapply {
        out["inEffect"] = json!(applied);
    }
    Ok(Json(out).into_response())
}

/// What [`bounded_apply`] came to.
pub(crate) enum BoundedApply {
    /// The apply's own verdict, within the bound — `apply_agent_change`'s
    /// shape: `Ok(None)` landed, `Ok(Some(_))` a soft failure, `Err` a step
    /// that threw.
    Settled(Result<Option<String>, String>),
    /// Still going when the bound passed. It goes on to its end, and logs
    /// its own verdict.
    StillApplying,
}

/// Apply a saved change to `department`'s agent (`apply_agent_change`: a
/// render, then a roll only where the render moved the container) on a task
/// of its own, and answer within `within`.
///
/// ON ITS OWN TASK: a client that goes away mid-save drops the request's
/// future, and a recreate's compose CLI dies with the future that runs it —
/// halfway, stranding its new container. BOUNDED: a roll's drain, stop and
/// recreate (2 m 41 s measured under heavy IO) plus its 180 s health wait
/// outlast the app proxy's 300 s, which answered a save that had landed with
/// a 502. The caller passes what is left of [`ANSWER_WITHIN`] (210 s, the
/// control route's bound) after its own work, so the whole request answers
/// before the proxy gives up. The verdict is logged here, once, because past
/// the bound nobody else hears it.
pub(crate) async fn bounded_apply(
    state: &AppState,
    sb: talaria_secretbox::SecretBox,
    department: &str,
    within: std::time::Duration,
    route: &'static str,
) -> BoundedApply {
    let (pg, dept) = (state.pg.clone(), department.to_string());
    let began = std::time::Instant::now();
    let op = async move {
        let verdict = apply_agent_change(&pg, &sb, &dept).await;
        let secs = began.elapsed().as_secs();
        match &verdict {
            Ok(None) => tracing::info!("{route} apply of {dept} landed ({secs}s)"),
            Ok(Some(warning)) => {
                tracing::warn!("{route} apply of {dept} did not land ({secs}s): {warning}")
            }
            Err(e) => tracing::warn!("{route} roll of {dept} failed ({secs}s): {e}"),
        }
        verdict
    };
    match run_detached(op, within).await {
        Detached::Settled(verdict) => BoundedApply::Settled(verdict),
        Detached::Lost(why) => {
            BoundedApply::Settled(Err(format!("the apply's task failed: {why}")))
        }
        Detached::StillRunning => {
            tracing::warn!(
                "{route} apply of {department} is still running after {}s — answering 202 (saved, the apply goes on); its verdict is logged when it lands",
                began.elapsed().as_secs()
            );
            BoundedApply::StillApplying
        }
    }
}

/// Re-render the fleet on a task of its own, waiting at most `within`: a
/// change saved without an apply, rendered so the agent's next start carries
/// it, or one an older agent's share of the department keeps from applying.
/// The caller's answer does not depend on the render (the change is written,
/// and every start renders again), so past the bound the request answers and
/// the render goes on. Awaited with no bound, a render that took minutes
/// under heavy IO could pass the app proxy's 300 s, which answers a 502 for
/// a change that was written. The render's own failure is logged here,
/// because nobody else hears it.
pub(crate) async fn bounded_render(
    state: &AppState,
    sb: talaria_secretbox::SecretBox,
    within: std::time::Duration,
    route: &'static str,
) {
    let pg = state.pg.clone();
    let began = std::time::Instant::now();
    let op = async move {
        if let Err(e) = talaria_api_facades::fleet::render::render_fleet(&pg, &sb).await {
            tracing::warn!(
                "{route} render failed ({}s): {e}",
                began.elapsed().as_secs()
            );
        }
    };
    match run_detached(op, within).await {
        Detached::Settled(()) => {}
        Detached::Lost(why) => tracing::warn!("{route} the render's task failed: {why}"),
        Detached::StillRunning => tracing::warn!(
            "{route} render still running after {}s — answering; it goes on, and logs a failure if it has one",
            began.elapsed().as_secs()
        ),
    }
}

/// Whether the current version is what the agent runs ([`version_in_effect`]).
#[derive(Debug, PartialEq, Eq)]
enum InEffect {
    /// It is (or, stopped, will be on its next start).
    Yes,
    /// It is not, and why: a Save & apply applies it.
    No(String),
    /// Not known, and why — a retired agent, or docker not saying — so
    /// there is nothing an apply could do: the answer's warning.
    Unsure(String),
}

/// Whether `version` — the agent's current one, which a Save & apply with
/// nothing changed would otherwise not re-apply — is what the agent runs.
/// `Yes` when it is: the last render wrote it (`rendered_version`, the
/// config.yaml a running Hermes reads in place) and the running container
/// was created from that render (`container_fit` = Matches: its env and the
/// settings Hermes reads only at start). A stopped agent is `Yes` too — its
/// next start renders and recreates whatever moved, as an apply to a
/// stopped agent leaves it.
async fn version_in_effect(
    state: &AppState,
    slug: &str,
    department: &str,
    display_name: &str,
    enabled: bool,
    version: i32,
) -> InEffect {
    use talaria_api_facades::fleet::docker::{
        CONTAINER_FIT_PATIENT, ContainerFit, container_fit, container_fit_within,
    };
    if !enabled {
        return InEffect::Unsure(format!(
            "{display_name} is retired, so nothing runs it — re-hire it to put it on"
        ));
    }
    let mut fit = container_fit(&state.pg, department).await;
    if fit == ContainerFit::Unknown {
        fit = container_fit_within(&state.pg, department, CONTAINER_FIT_PATIENT).await;
    }
    let rendered = talaria_api_facades::fleet::render::rendered_version(slug).await;
    version_state(fit, rendered, display_name, version)
}

/// [`version_in_effect`]'s decision, from the container's fit and the
/// version the last render wrote.
fn version_state(
    fit: talaria_api_facades::fleet::docker::ContainerFit,
    rendered: Option<i32>,
    display_name: &str,
    version: i32,
) -> InEffect {
    use talaria_api_facades::fleet::docker::ContainerFit;
    match fit {
        ContainerFit::NotRunning => InEffect::Yes,
        ContainerFit::Unknown => InEffect::Unsure(format!(
            "docker did not say whether {display_name} is running it, so it may not be — roll the agent to be sure"
        )),
        ContainerFit::Matches if rendered == Some(version) => InEffect::Yes,
        ContainerFit::Matches => InEffect::No(format!(
            "it was saved and never applied — {display_name} still runs {}",
            rendered.map_or_else(
                || "an earlier render".to_string(),
                |v| format!("version {v}")
            )
        )),
        ContainerFit::Differs => InEffect::No(format!(
            "{display_name}'s running container was created from an earlier render (a setting it reads only at start, its env or its image moved, and no roll has landed since)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: Value) -> Map<String, Value> {
        json.as_object().cloned().expect("an object literal")
    }

    /// A Save & apply with nothing changed is quiet only when the agent runs
    /// the current version (or is stopped, and its next start reads the
    /// render); a version that is not in effect is applied; docker's silence
    /// is said, not acted on.
    #[test]
    fn a_no_op_apply_applies_a_version_that_is_not_in_effect() {
        use talaria_api_facades::fleet::docker::ContainerFit;
        // Running, created from the render that wrote this version.
        assert_eq!(
            version_state(ContainerFit::Matches, Some(4), "Ava", 4),
            InEffect::Yes
        );
        // Stopped or gone: its next start renders and recreates what moved.
        assert_eq!(
            version_state(ContainerFit::NotRunning, Some(3), "Ava", 4),
            InEffect::Yes
        );
        assert_eq!(
            version_state(ContainerFit::NotRunning, None, "Ava", 4),
            InEffect::Yes
        );
        // Saved without an apply: the render still holds version 3 — apply.
        let InEffect::No(never) = version_state(ContainerFit::Matches, Some(3), "Ava", 4) else {
            panic!("a version the render never wrote is not in effect");
        };
        assert!(
            never.starts_with("it was saved and never applied"),
            "{never}"
        );
        assert!(never.contains("Ava still runs version 3"), "{never}");
        // Nothing rendered for it at all: an earlier render, apply.
        let InEffect::No(none) = version_state(ContainerFit::Matches, None, "Ava", 4) else {
            panic!("no rendered version is not this one");
        };
        assert!(none.contains("an earlier render"), "{none}");
        // Rendered, but the container is from an earlier render (a roll that
        // did not land) — apply.
        let InEffect::No(moved) = version_state(ContainerFit::Differs, Some(4), "Ava", 4) else {
            panic!("a container from an earlier render is not in effect");
        };
        assert!(moved.contains("created from an earlier render"), "{moved}");
        // Docker would not say: not claimed either way, and not applied (the
        // apply would read the same silence).
        let InEffect::Unsure(unknown) = version_state(ContainerFit::Unknown, Some(4), "Ava", 4)
        else {
            panic!("docker's silence is not a verdict");
        };
        assert!(unknown.contains("docker did not say"), "{unknown}");
    }

    /// Absent is not null: one leaves the def's stored block alone, the other
    /// is an instruction to drop a key. A door that collapsed them could not
    /// express "stop overriding the default".
    fn parse_compression_t(
        m: &Map<String, Value>,
    ) -> Result<(CompressionEdits, Vec<String>), String> {
        parse_compression(m, "compression")
    }

    #[test]
    fn an_absent_member_keeps_and_a_null_member_clears() {
        let (edits, notes) = parse_compression_t(&body(json!({}))).unwrap();
        assert_eq!(edits, CompressionEdits::default());
        assert!(notes.is_empty());

        // the member present but empty: still nothing said about either knob
        let (edits, _) = parse_compression_t(&body(json!({ "compression": {} }))).unwrap();
        assert_eq!(edits, CompressionEdits::default());

        let (edits, _) = parse_compression_t(&body(
            json!({ "compression": { "threshold": null, "thresholdTokens": null } }),
        ))
        .unwrap();
        assert_eq!(
            edits,
            CompressionEdits {
                threshold: Knob::Clear,
                threshold_tokens: Knob::Clear,
            }
        );
    }

    #[test]
    fn numbers_write_the_knobs_they_name() {
        let (edits, notes) = parse_compression_t(&body(
            json!({ "compression": { "threshold": 0.9, "thresholdTokens": 400000 } }),
        ))
        .unwrap();
        assert_eq!(
            edits,
            CompressionEdits {
                threshold: Knob::Set(0.9),
                threshold_tokens: Knob::Set(400_000),
            }
        );
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// A ratio under Hermes' floor SAVES, and the answer says what Hermes
    /// will do with it — the floor only applies under 512K of context, so
    /// refusing the number would be wrong for the models above that.
    #[test]
    fn a_low_ratio_saves_and_the_answer_carries_the_note() {
        let (edits, notes) =
            parse_compression_t(&body(json!({ "compression": { "threshold": 0.5 } }))).unwrap();
        assert_eq!(edits.threshold, Knob::Set(0.5));
        assert_eq!(edits.threshold_tokens, Knob::Keep);
        assert_eq!(notes, vec![HERMES_FLOOR_NOTE.to_string()]);

        // exactly the floor is nothing to warn about
        let (_, notes) =
            parse_compression_t(&body(json!({ "compression": { "threshold": 0.75 } }))).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// The bounds refuse in the shared helper's words — the same sentences
    /// every other number on this door answers with.
    #[test]
    fn the_bounds_and_the_types_refuse_the_way_the_rest_of_the_door_does() {
        let err = |b: Value| parse_compression_t(&body(b)).unwrap_err();
        assert_eq!(
            err(json!({ "compression": { "threshold": 0.05 } })),
            "Too small: expected number to be >=0.1"
        );
        assert_eq!(
            err(json!({ "compression": { "threshold": 1.5 } })),
            "Too big: expected number to be <=1"
        );
        assert_eq!(
            err(json!({ "compression": { "thresholdTokens": 999 } })),
            "Too small: expected number to be >=1000"
        );
        assert_eq!(
            err(json!({ "compression": { "thresholdTokens": 10_000_001 } })),
            "Too big: expected number to be <=10000000"
        );
        assert_eq!(
            err(json!({ "compression": { "thresholdTokens": 1000.5 } })),
            "Invalid input: expected int, received number"
        );
        assert_eq!(
            err(json!({ "compression": { "threshold": "high" } })),
            "Invalid input: expected number, received string"
        );
        assert_eq!(
            err(json!({ "compression": [] })),
            "Invalid input: expected object, received array"
        );
    }
}
