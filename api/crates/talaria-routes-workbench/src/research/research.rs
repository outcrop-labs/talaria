// /api/research.
// GET → recent runs scoped to the viewer + the mode catalog. POST { question,
// mode?, agentModel?, conversationId?, effort?, breadth?, sources? } → start a
// run. The three dials override the mode's preset one at a time; a
// conversationId points the run at a thread the caller already has, which it
// discusses and reports into instead of making one of its own.
//
// Humans and agents both start research; an agent researches AS ITSELF, pinned
// to its own model. An agent-started run carries the attribution ladder's user
// as its owner — the PA's owner, the chatting human mid-chat, the hirer
// otherwise — while an org agent's run stays org-readable however owned
// (org-ness is derived from the row downstream, never from the owner's
// presence).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::{AgentSubject, agent_caller};
use talaria_api_facades::fleet::usable_agent_gate;
use talaria_api_facades::runs::defs::research::{Dials, research_modes};
use talaria_body::{
    NumKind, optional_number_member, optional_string_member, optional_uuid_member, parse,
    string_member,
};
use talaria_conversations::accessible_conversation;
use talaria_error::{house_error, internal, object_or_400};
use talaria_harness_defs::defs::research::ResearchDepth;
use talaria_permissions::has_perm;
use talaria_research as research;
use talaria_research::{ResearchRun, research_run_for_conversation, start_research};
use talaria_research_origin::{current_agent_turn, remember_research_origin};
use talaria_session::require_user;
use talaria_state::AppState;

/// `z.enum(['recon','brief','expedition']).default('brief')` — absent is the
/// default mode, a present value must be one of the three.
fn mode_member(obj: &serde_json::Map<String, serde_json::Value>) -> Result<ResearchDepth, String> {
    let mode = match obj.get("mode") {
        None => "brief".to_string(),
        Some(_) => talaria_body::enum_member(obj, "mode", &["recon", "brief", "expedition"])?,
    };
    Ok(match mode.as_str() {
        "recon" => ResearchDepth::Recon,
        "expedition" => ResearchDepth::Expedition,
        _ => ResearchDepth::Brief,
    })
}

/// THE THREE DIALS the mode is a preset over, or `None` for "whatever the mode
/// says". Each is `z.number().int().min(1).max(3).optional()`, and the field
/// name goes on the message: three members share one bound, so "expected
/// number to be <=3" on its own leaves a caller guessing which dial it got
/// wrong.
///
/// ANY ONE GIVEN FILLS THE OTHER TWO from the mode's own preset. A caller who
/// wants more sources on a brief says so about sources; they are not made to
/// restate the two dials they were happy with.
fn dials_member(
    obj: &serde_json::Map<String, serde_json::Value>,
    mode: ResearchDepth,
) -> Result<Option<Dials>, String> {
    // How hard the run works the question: rounds of searching.
    let effort = optional_number_member(obj, "effort", NumKind::Int, 1.0, 3.0)
        .map_err(|msg| format!("effort: {msg}"))?;
    // How wide each round casts: queries planned per round.
    let breadth = optional_number_member(obj, "breadth", NumKind::Int, 1.0, 3.0)
        .map_err(|msg| format!("breadth: {msg}"))?;
    // How many sources the report may end up citing.
    let sources = optional_number_member(obj, "sources", NumKind::Int, 1.0, 3.0)
        .map_err(|msg| format!("sources: {msg}"))?;
    if effort.is_none() && breadth.is_none() && sources.is_none() {
        // ABSENT IS NOT A DIAL SETTING. `None` stays None all the way to the
        // run's input, so a run started without them keeps reading as its mode
        // rather than as three numbers that happen to match today's preset.
        return Ok(None);
    }
    let preset = Dials::from_mode(mode);
    // Validated even though every member is already bounded 1..=3: the bound
    // lives in two places now, and `validate` is the one that owns it.
    Dials {
        effort: effort.map_or(preset.effort, |n| n as u8),
        breadth: breadth.map_or(preset.breadth, |n| n as u8),
        sources: sources.map_or(preset.sources, |n| n as u8),
    }
    .validate()
    .map(Some)
}

/// The mode catalog the picker renders: [{mode, blurb}].
fn modes_json() -> serde_json::Value {
    json!(
        research_modes()
            .into_iter()
            .map(|(mode, blurb)| json!({ "mode": mode, "blurb": blurb }))
            .collect::<Vec<_>>()
    )
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    // Scope to the viewer: a user sees their own + shared + org runs; an
    // agent sees through its owner's eyes (general agents: org runs only).
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return get_as_user(&state, &headers).await,
        Err(resp) => return Err(resp),
    };
    // The CALLER: seeing a human's private runs is owner-proxying, so an
    // asserted identity resolves to no owner (org runs only).
    let viewer =
        match talaria_users::assistant_owner_for(&state.pg, &AgentSubject::Caller(caller)).await {
            Ok(v) => v,
            Err(e) => return Ok(internal("[research] owner resolve on list failed", e)),
        };
    Ok(
        match research::list_research_runs(&state.pg, viewer.as_deref(), 60).await {
            Ok(runs) => Json(json!({ "runs": runs, "modes": modes_json() })).into_response(),
            Err(e) => internal("[research] list failed", e),
        },
    )
}

async fn get_as_user(state: &AppState, headers: &HeaderMap) -> Result<Response, Response> {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return Err(gate),
    };
    Ok(
        match research::list_research_runs(&state.pg, Some(&user.id), 60).await {
            Ok(runs) => Json(json!({ "runs": runs, "modes": modes_json() })).into_response(),
            Err(e) => internal("[research] list failed", e),
        },
    )
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let question = match string_member(obj, "question", 8, 4000) {
        Ok(q) => q,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let mode = match mode_member(obj) {
        Ok(m) => m,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let body_agent = match optional_string_member(obj, "agentModel", 200) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The thread the run reports into, instead of one of its own.
    let conversation_id = match optional_uuid_member(obj, "conversationId") {
        Ok(c) => c,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let dials = match dials_member(obj, mode) {
        Ok(d) => d,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    let caller = agent_caller(&state.pg, &headers).await?;
    let (agent_model, owner_user_id, requested_by) = if let Some(caller) = caller.as_ref() {
        // The ladder, not the PA-only resolve: the run's owner is a STAMP —
        // the PA's owner, the human an org agent is answering mid-chat, the
        // hirer for crons and standalone calls. Reach is NOT this value (an
        // org agent's run stays org-readable however owned; the driver
        // derives that from the row), and the finished-run notification below
        // reaches this owner — for a chat-attributed run, the person who
        // asked.
        let owner = match talaria_attribution::responsible_user_for(
            &state.pg,
            state.redis().await.ok(),
            &AgentSubject::Caller(caller.clone()),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(internal("[research] owner resolve on start failed", e)),
        };
        (caller.model.clone(), owner, caller.model.clone())
    } else {
        let user = require_user(&state, &headers).await?;
        match has_perm(&state.pg, &user.id, &user.role, "research.run").await {
            Ok(true) => {}
            Ok(false) => {
                return Ok(house_error(
                    StatusCode::FORBIDDEN,
                    "no permission to run research",
                ));
            }
            Err(e) => return Ok(internal("[research] perm read on start failed", e)),
        }
        // Humans pick the agent (and need access to it); an agent-key caller
        // is pinned to itself above and never reaches this leg.
        let Some(agent_model) = body_agent else {
            return Ok(house_error(StatusCode::BAD_REQUEST, "agentModel required"));
        };
        let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
            Ok(g) => g,
            Err(e) => return Ok(internal("[research] agent access read on start failed", e)),
        };
        if !gate(&agent_model) {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "forbidden: no access to this agent",
            ));
        }
        (
            agent_model,
            Some(user.id.clone()),
            user.email
                .clone()
                .or(user.name.clone())
                .unwrap_or_else(|| "user".into()),
        )
    };

    // ── MAY THIS CALLER POINT A RUN AT THAT THREAD? ──────────────────────────
    //
    // THE SAME DOOR THE CHAT ROUTE USES, and it has to be: the run posts its
    // scoping question, its progress and its report into this conversation,
    // and everyone who can read the run can then read the thread. A caller who
    // could name a conversation they cannot open would be writing into
    // somebody else's room.
    //
    // GATED AS THE OWNER STAMP, which is the same person on both legs — a
    // human's own id on the session path, and on the agent path the
    // attribution ladder's responsible user, the person the agent is answering
    // for. An agent with no responsible user has nobody whose access could be
    // checked, so it cannot name a conversation at all; it still gets a run,
    // just the thread-less kind it has always had.
    if let Some(conversation) = conversation_id.as_deref() {
        let reachable = match owner_user_id.as_deref() {
            Some(user_id) => {
                match accessible_conversation(&state.pg, user_id, conversation).await {
                    Ok(c) => c.is_some(),
                    Err(e) => {
                        return Ok(internal(
                            "[research] conversation access read on start failed",
                            e,
                        ));
                    }
                }
            }
            None => false,
        };
        if !reachable {
            // NOT FOUND, never forbidden — the run's own routes answer the
            // same way. A caller who cannot see a conversation must not learn
            // from the status code that it exists.
            return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found"));
        }
        // ONE LIVE RUN PER THREAD. The conversation-keyed readers hand back
        // one run (the newest), and the discussion beside it narrates one
        // run's progress; a second run working under the same thread would
        // have two scopers, two report-ready turns and one rail row to show
        // for them. A finished run is a different matter — the next question
        // in the same session is the newest, and the readers follow it.
        let live: Option<(String,)> = match sqlx::query_as(
            "select id::text from research_runs \
             where conversation_id = $1::uuid and status in ('queued', 'running', 'awaiting') \
             limit 1",
        )
        .bind(conversation)
        .fetch_optional(&state.pg)
        .await
        {
            Ok(l) => l,
            Err(e) => return Ok(internal("[research] live-run read on start failed", e)),
        };
        if let Some((id,)) = live {
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({ "run": null, "duplicateOf": id, "reason": "thread" })),
            )
                .into_response());
        }
    }

    // A queued/running duplicate of the same question is a double-click —
    // answered by a raw column scan, not the projection's run-aware CASE
    // (`active_research_on`).
    let dupe: Option<(String,)> = match sqlx::query_as(
        "select id::text from research_runs \
         where question = $1 and status in ('queued', 'running') limit 1",
    )
    .bind(&question)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(d) => d,
        Err(e) => return Ok(internal("[research] duplicate check failed", e)),
    };
    if let Some((id,)) = dupe {
        return Ok((
            StatusCode::CONFLICT,
            Json(json!({ "run": null, "duplicateOf": id, "reason": "question" })),
        )
            .into_response());
    }

    // ── IS THIS A FOLLOW-UP? ──────────────────────────────────────────────────
    //
    // INFERRED, NEVER ASKED FOR. An agent commissioning research from inside a
    // research conversation is extending THAT report, and it already knows
    // which one — the conversation it is speaking in belongs to a run. So the
    // parent is looked up here rather than added as a tool parameter for a
    // model to fill in, which means a model cannot get it wrong, pass a stale
    // runId, or forget. The best fix for a mistake is to make it unavailable.
    //
    // Agent callers only: a person on the Research page who types a new
    // question is starting a new subject, and the page they are looking at
    // has its own "ask a follow-up" affordance for the other case.
    let origin = if caller.is_some() {
        current_agent_turn(&state, &agent_model).await
    } else {
        None
    };
    let parent_run_id = match &origin {
        Some(conversation) => research_run_for_conversation(&state.pg, conversation)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    let run: ResearchRun = match start_research(
        &state,
        talaria_api_facades::runs::defs::research::ResearchInput {
            question: question.clone(),
            mode,
            agent_model: agent_model.clone(),
            owner_user_id,
            requested_by,
            parent_run_id: parent_run_id.clone(),
            conversation_id: conversation_id.clone(),
            dials,
        },
    )
    .await
    {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // WHO IS OWED THE ANSWER. An agent that starts a run mid-conversation
    // cannot wait for it — the tool returns a runId and the turn ends — so the
    // run remembers the chat it came out of and reports back there when it
    // finishes. Only for agent callers: a human who started a run from the
    // Research page is already looking at the page that updates.
    if caller.is_some()
        && let Some(origin) = &origin
    {
        remember_research_origin(&state, &run.id, origin).await;
    }
    Ok(Json(json!({ "run": run })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().expect("a body").clone()
    }

    #[test]
    fn no_dials_in_the_body_is_not_a_dial_setting() {
        // The run's input keeps `None`, so a run started off a mode button
        // reads as that mode for ever rather than as three numbers that
        // happened to be the preset on the day it ran.
        let body = obj(json!({ "question": "does pg17 vacuum faster?" }));
        assert!(
            dials_member(&body, ResearchDepth::Brief)
                .expect("nothing to validate")
                .is_none()
        );
    }

    #[test]
    fn one_dial_given_fills_the_other_two_from_the_mode() {
        // A caller who wants a wider sweep on a brief says so about breadth;
        // the other two come from the preset rather than from a default of
        // this route's own invention — two spellings of the preset is how they
        // come to disagree.
        for mode in [
            ResearchDepth::Recon,
            ResearchDepth::Brief,
            ResearchDepth::Expedition,
        ] {
            let preset = Dials::from_mode(mode);
            let got = dials_member(&obj(json!({ "breadth": 3 })), mode)
                .expect("3 is in range")
                .expect("a dial was given");
            assert_eq!(got.breadth, 3);
            assert_eq!(got.effort, preset.effort, "{mode:?} effort");
            assert_eq!(got.sources, preset.sources, "{mode:?} sources");
        }
    }

    #[test]
    fn all_three_given_are_all_three_taken() {
        let got = dials_member(
            &obj(json!({ "effort": 1, "breadth": 2, "sources": 3 })),
            ResearchDepth::Recon,
        )
        .expect("all in range")
        .expect("dials were given");
        assert_eq!((got.effort, got.breadth, got.sources), (1, 2, 3));
    }

    #[test]
    fn an_out_of_range_dial_names_itself_in_the_message() {
        // THE FIELD NAME IS THE POINT. Three members share one bound, so a
        // bare "expected number to be <=3" would leave the caller to guess
        // which of the three they got wrong.
        let msg = dials_member(&obj(json!({ "sources": 7 })), ResearchDepth::Brief)
            .expect_err("7 is out of range");
        assert!(msg.starts_with("sources: "), "{msg}");
        assert!(msg.contains("<=3"), "{msg}");

        let msg = dials_member(&obj(json!({ "effort": 0 })), ResearchDepth::Brief)
            .expect_err("0 is below the range");
        assert!(msg.starts_with("effort: "), "{msg}");
        assert!(msg.contains(">=1"), "{msg}");
    }

    #[test]
    fn a_dial_is_an_int_and_null_is_not_a_dial() {
        // zod's order, and zod's words: the int guard answers before the
        // bounds, and an `.optional()`-only member refuses a present null
        // rather than folding it into "absent".
        let msg = dials_member(&obj(json!({ "breadth": 1.5 })), ResearchDepth::Brief)
            .expect_err("1.5 is not an int");
        assert_eq!(msg, "breadth: Invalid input: expected int, received number");

        let msg = dials_member(
            &obj(json!({ "effort": serde_json::Value::Null })),
            ResearchDepth::Brief,
        )
        .expect_err("null is a type error here");
        assert_eq!(msg, "effort: Invalid input: expected number, received null");

        let msg = dials_member(&obj(json!({ "sources": "3" })), ResearchDepth::Brief)
            .expect_err("a string is not a number");
        assert_eq!(
            msg,
            "sources: Invalid input: expected number, received string"
        );
    }

    #[test]
    fn the_mode_member_defaults_and_then_closes_the_set() {
        // Absent is the default mode; a present value must be one of the
        // three. Pinned beside the dials because the dials READ the mode —
        // a body that got the mode wrong must not reach a preset lookup.
        assert_eq!(
            mode_member(&obj(json!({}))).expect("absent is the default"),
            ResearchDepth::Brief
        );
        assert_eq!(
            mode_member(&obj(json!({ "mode": "expedition" }))).expect("one of the three"),
            ResearchDepth::Expedition
        );
        assert!(mode_member(&obj(json!({ "mode": "deep" }))).is_err());
    }
}
