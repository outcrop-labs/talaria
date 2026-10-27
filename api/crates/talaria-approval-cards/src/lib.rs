// talaria-approval-cards — an agent's approval request, put to the one person
// who may answer it.
//
// Hermes asks before a handful of things no policy may wave through: a write
// to a project's AGENTS.md / CLAUDE.md / SOUL.md / .cursorrules / `.hermes/`,
// an untrusted MCP server's write tool or spend limit, a saved card fill. On a
// streamed turn the ask is an `approval.request` frame, and the fleet chat
// door answers every one `deny` (talaria-gateway's fleet_approvals), because
// a door cannot consent for a person and cannot tell those gates apart from
// a dangerous command. Before this crate, that left a Developer Agent unable
// ever to do one of those things on a chat turn.
//
// THE RULE, IN FOUR PARTS.
//
//   * WHICH REQUESTS. Only a Developer Agent's. Its render is
//     `approvals.mode: off`, under which every request that still arrives is
//     by construction one of those human-only gates; the frame carries no
//     `pattern_key` to say so, so the agent's policy is the discriminator.
//     Any other agent's request (an `rm -rf` under `mode: manual`) is policy,
//     and the door refuses it in milliseconds, as before.
//   * WHO ANSWERS. The owner of the conversation the turn belongs to, on an
//     ATTENDED turn (`TurnAttribution::attended`: sent from /api/chat, or
//     chained or resumed from one), and only when that owner manages the
//     agent (a named manager, its personal owner, or an admin). Owning the
//     conversation is not enough: a member chatting with a Developer Agent
//     they do not manage must not authorize a write that steers its future
//     runs. Channel, ticket, research, app and harness turns get no card;
//     nobody is reliably reading them. The decision route checks all of this
//     again; the door is never trusted with it.
//   * WHAT CAN BE SAID. `once` or `deny`. `session` and `always` would write
//     the config.yaml the render owns and mounts read-only.
//   * HOW LONG. A Developer Agent renders `approvals.timeout: 120`; the card
//     closes itself at 115 s (`card_window`), answers `deny`, and says
//     "expired", rather than leaving Hermes' timeout to refuse it silently.
//     The session's later turns queue behind an open request, so it is never
//     longer. The 115 s count from when Hermes asked. A live frame arrives as
//     it is asked; a request first seen by a reattached stream may have been
//     asked any time after the lost stream's last bytes (or the previous
//     poll), so its card counts from then (`closes_at`), and one with too
//     little left to answer is refused instead.
//
// THE CARD IS A `chat_approvals` ROW of kind `hermes_approval` and an
// `approval` chip on the turn's own message, the plane every other chat
// approval uses. It stays out of the approvals census (talaria-approvals):
// a card lives two minutes, and the census's announce sweep, digest and
// escalation nags work in hours.
//
// THE RECORD. A person's answer is an `agent.approval_allowed` or
// `agent.approval_denied` audit row with that person as the actor; a card
// that expired is `agent.approval_denied` by `talaria`, beside the door's
// own policy refusals. An answer that reached an agent no longer waiting
// (Hermes' own timeout, a stop, the run's end got there first) took nothing,
// so it is neither: `agent.approval_expired`, by the person, with the choice
// they made.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_chips::{
    ApprovalRow, Target, ensure_approval, land_chips, landing_target, set_chip_status,
};
use talaria_gateway::fleet_approvals::{
    ANSWER_WINDOW, ApprovalAsked, ApprovalTurn, CardAsk, Delivery, answer_card, card_window,
    recorded_command,
};

pub use talaria_gateway::fleet_approvals::Choice;

/// The `chat_approvals.kind` (and the chip's `actionKind`) of a card.
pub const KIND: &str = "hermes_approval";

type Redis = Option<redis::aio::ConnectionManager>;

const LOG: &str = "[approval-cards]";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Who may answer ───────────────────────────────────────────────────────────

/// Why a request is not a card. The door refuses each of these itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nobody reads this turn as it is written: a channel reply, a ticket's
    /// work session, research, an app, the workbench, a harness step.
    Unattended,
    /// Not a Developer Agent: its requests are policy, refused by the door.
    NotDeveloper,
    /// The conversation is not this agent's, or is gone.
    NoConversation,
    /// The conversation's owner may not change this agent.
    OwnerDoesNotManage,
    /// Hermes offers no `once` for this request, so there is nothing a
    /// person could approve.
    NothingToApprove,
}

/// What the eligibility rule reads about a request's agent and conversation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// The agent's def has the Developer switch on.
    pub developer: bool,
    /// The conversation's owner, when the conversation exists and is this
    /// agent's.
    pub owner: Option<String>,
    /// That owner manages the agent: a named manager, its personal owner, or
    /// an admin (`talaria_agent_managers::manages_agent_model`).
    pub owner_manages: bool,
}

/// The one person who may answer a request, or why nobody may. `choices`
/// are the answers Hermes put on the frame (empty on a frame that listed
/// none, which offers both).
pub fn decider(turn: &ApprovalTurn, facts: &Facts, choices: &[String]) -> Result<String, Refusal> {
    if turn.conversation_id.is_none() {
        return Err(Refusal::Unattended);
    }
    if !facts.developer {
        return Err(Refusal::NotDeveloper);
    }
    let Some(owner) = &facts.owner else {
        return Err(Refusal::NoConversation);
    };
    if !facts.owner_manages {
        return Err(Refusal::OwnerDoesNotManage);
    }
    if !offered(choices).contains(&Choice::Once) {
        return Err(Refusal::NothingToApprove);
    }
    Ok(owner.clone())
}

/// The answers a card offers: `once` and `deny`, as far as Hermes takes
/// them. Never `session` or `always` (see the header).
pub fn offered(choices: &[String]) -> Vec<Choice> {
    [Choice::Once, Choice::Deny]
        .into_iter()
        .filter(|c| choices.is_empty() || choices.iter().any(|h| h == c.as_str()))
        .collect()
}

async fn read_facts(
    pg: &PgPool,
    agent_model: &str,
    conversation_id: &str,
) -> Result<Facts, sqlx::Error> {
    let row: Option<(bool, Option<String>, Option<String>)> = sqlx::query_as(
        "select d.developer, c.user_id::text, u.role \
           from agent_defs d \
           left join conversations c on c.id = $2::uuid and c.agent_model = d.model \
           left join users u on u.id = c.user_id \
          where d.model = $1",
    )
    .bind(agent_model)
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    let Some((developer, owner, role)) = row else {
        return Ok(Facts::default());
    };
    let owner_manages = match &owner {
        Some(user) => {
            talaria_agent_managers::manages_agent_model(
                pg,
                user,
                role.as_deref().unwrap_or("member"),
                agent_model,
            )
            .await?
        }
        None => false,
    };
    Ok(Facts {
        developer,
        owner,
        owner_manages,
    })
}

// ── Raising a card ───────────────────────────────────────────────────────────

/// The least time a card is worth raising with: under this, nobody could
/// read the request and answer it, and the door refuses it now instead.
const MIN_CARD_WINDOW: Duration = Duration::from_secs(10);

/// When a card raised at `now_ms` closes: `card_window` after Hermes asked,
/// which is now on a live frame and no earlier than `asked_after_ms` on a
/// frame a reattached stream rebuilt. That bound only ever shortens the
/// window (a later one than now is read as now). None when less than
/// `MIN_CARD_WINDOW` would be left.
pub fn closes_at(now_ms: i64, asked_after_ms: Option<i64>) -> Option<i64> {
    let asked = asked_after_ms.map_or(now_ms, |t| t.min(now_ms));
    let closes = asked + card_window().as_millis() as i64;
    (closes - now_ms >= MIN_CARD_WINDOW.as_millis() as i64).then_some(closes)
}

/// How long a person's answer holds its request after the card is claimed:
/// the send's own window, plus the manifest read and the claim before it.
const ANSWER_IN_FLIGHT: Duration = Duration::from_secs(ANSWER_WINDOW.as_secs() + 5);

/// Whether a settled card is still holding its request: a person's answer
/// claimed it `decided_ms_ago` and may still be on its way to the agent.
/// The door must not race it with a `deny` of its own; if the answer fails,
/// the card opens again and its deadline answers.
pub fn answer_in_flight(status: &str, decided_ms_ago: Option<i64>) -> bool {
    matches!(status, "approved" | "denied")
        && decided_ms_ago.is_some_and(|ago| ago < ANSWER_IN_FLIGHT.as_millis() as i64)
}

/// The row's `external_id`: one card per request, so a reattached stream
/// that raises the same request again finds the card already waiting.
fn external_id(asked: &ApprovalAsked) -> String {
    format!("hermes:{}:{}", asked.run_id, asked.request_id)
}

/// The row's payload: what the answer is addressed to, what was asked, and
/// when the card closes.
pub fn card_payload(
    agent_model: &str,
    conversation_id: &str,
    asked: &ApprovalAsked,
    expires_at_ms: i64,
) -> Value {
    json!({
        "runId": asked.run_id,
        "requestId": asked.request_id,
        "executionGeneration": asked.execution_generation,
        "agentModel": agent_model,
        "conversationId": conversation_id,
        "choices": offered(&asked.choices).iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        "command": recorded_command(&asked.command),
        "description": asked.description,
        "expiresAt": expires_at_ms,
    })
}

/// The chip the card is drawn from. `deciderId` is who sees the buttons;
/// everyone else reading the conversation sees the card read-only.
pub fn card_chip(approval_id: &str, decider: &str, payload: &Value) -> Value {
    json!({
        "id": chip_id(approval_id),
        "kind": "approval",
        "actionId": approval_id,
        "actionKind": KIND,
        "summary": summary_of(payload),
        "command": payload["command"],
        "choices": payload["choices"],
        "deciderId": decider,
        "expiresAt": payload["expiresAt"],
        "status": "pending",
    })
}

fn chip_id(approval_id: &str) -> String {
    format!("approval:{approval_id}")
}

/// Hermes' own sentence for why it asks, or a plain one.
fn summary_of(payload: &Value) -> String {
    let description = payload["description"].as_str().unwrap_or("").trim();
    if description.is_empty() {
        "The agent is asking before it goes on.".into()
    } else {
        description.chars().take(500).collect()
    }
}

/// The request a payload answers.
fn asked_of(payload: &Value) -> ApprovalAsked {
    let text = |k: &str| payload[k].as_str().unwrap_or("").to_string();
    ApprovalAsked {
        run_id: text("runId"),
        request_id: text("requestId"),
        execution_generation: payload["executionGeneration"].as_i64(),
        command: text("command"),
        description: text("description"),
        choices: payload["choices"]
            .as_array()
            .map(|c| {
                c.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        asked_after_ms: None,
    }
}

/// The card seam (`talaria_gateway::fleet_approvals::RAISE_CARD`): raise a
/// card for this request when a person may answer it. `false` leaves the
/// request to the door, which refuses it; so does every failure here.
pub async fn raise(pg: &PgPool, redis: Redis, ask: CardAsk) -> bool {
    let CardAsk {
        agent_model,
        turn,
        asked,
    } = ask;
    let source = turn.source.clone().unwrap_or_else(|| "unattributed".into());
    let Some(conversation_id) = turn.conversation_id.clone() else {
        return false;
    };
    let facts = match read_facts(pg, &agent_model, &conversation_id).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("{LOG} {agent_model}: could not read who may answer: {e}");
            return false;
        }
    };
    let decider = match decider(&turn, &facts, &asked.choices) {
        Ok(d) => d,
        Err(why) => {
            if why != Refusal::NotDeveloper {
                tracing::info!(
                    "{LOG} {agent_model}: no card for a request on a {source} turn ({why:?}); \
                     the door refuses it"
                );
            }
            return false;
        }
    };
    match raise_for(pg, redis, &agent_model, &conversation_id, &decider, &asked).await {
        Ok(raised) => raised,
        Err(e) => {
            tracing::warn!("{LOG} {agent_model}: could not raise a card: {e}");
            false
        }
    }
}

async fn raise_for(
    pg: &PgPool,
    redis: Redis,
    agent_model: &str,
    conversation_id: &str,
    decider: &str,
    asked: &ApprovalAsked,
) -> Result<bool, sqlx::Error> {
    let external = external_id(asked);
    // The same request raised again (the reattached stream after the api
    // lost the live one): a card still waiting is the card, and its deadline
    // is armed again, since a restart lost the timer. So is a card whose
    // person's answer is still on its way: a `deny` from the door could
    // reach Hermes first. A card settled otherwise means the answer went
    // out, or the card expired; the door refuses whatever Hermes still holds.
    let existing: Option<(String, String, Value, Option<i64>)> = sqlx::query_as(
        "select id::text, status, payload, \
                (extract(epoch from (now() - decided_at)) * 1000)::bigint \
           from chat_approvals \
          where external_id = $1 and kind = $2 order by created_at desc limit 1",
    )
    .bind(&external)
    .bind(KIND)
    .fetch_optional(pg)
    .await?;
    if let Some((id, status, payload, decided_ms_ago)) = existing {
        if status == "pending" || answer_in_flight(&status, decided_ms_ago) {
            arm(
                pg.clone(),
                redis,
                id,
                payload["expiresAt"].as_i64().unwrap_or(0),
            );
            return Ok(true);
        }
        return Ok(false);
    }
    let Some(expires_at) = closes_at(now_ms(), asked.asked_after_ms) else {
        tracing::info!(
            "{LOG} {agent_model}: a flagged call ({}) reached the api too late to put to a \
             person before Hermes' timeout; the door refuses it",
            asked.description
        );
        return Ok(false);
    };
    let Target::Found(landed) = landing_target(pg, agent_model, Some(conversation_id)).await?
    else {
        tracing::info!(
            "{LOG} {agent_model}: conversation {conversation_id} has no turn to show a card on; \
             the door refuses the request"
        );
        return Ok(false);
    };
    let payload = card_payload(agent_model, conversation_id, asked, expires_at);
    let id = ensure_approval(
        pg,
        KIND,
        &summary_of(&payload),
        &payload,
        &external,
        agent_model,
        Some(decider),
        Some(&landed),
    )
    .await?;
    let chip = card_chip(&id, decider, &payload);
    if let Err(e) = land_chips(pg, redis.clone(), &landed, &[chip]).await {
        // A card nobody can see is no card: close the row, and let the door
        // refuse the request.
        let _ = sqlx::query(
            "update chat_approvals set status = 'expired', decided_at = now() \
              where id = $1::uuid and status = 'pending'",
        )
        .bind(&id)
        .execute(pg)
        .await;
        return Err(e);
    }
    tracing::info!(
        "{LOG} {agent_model}: put a flagged call ({}) to its manager in conversation \
         {conversation_id}; the card closes in {}s",
        asked.description,
        (expires_at - now_ms()).max(0) / 1000
    );
    arm(pg.clone(), redis, id, expires_at);
    Ok(true)
}

/// The card's own deadline: at `expires_at_ms`, a card still waiting is
/// answered `deny` and reads "expired".
fn arm(pg: PgPool, redis: Redis, id: String, expires_at_ms: i64) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        let wait = (expires_at_ms - now_ms()).max(0) as u64;
        tokio::time::sleep(Duration::from_millis(wait)).await;
        expire(&pg, redis, &id).await;
    });
}

/// Close a waiting card: mark it expired, answer `deny`, and record it. A
/// card a person already answered, or one already expired, is left alone.
pub async fn expire(pg: &PgPool, redis: Redis, id: &str) {
    let claimed: Result<Option<(Value, Option<String>, Option<String>, Option<String>)>, _> =
        sqlx::query_as(
            "update chat_approvals set status = 'expired', decided_at = now() \
              where id = $1::uuid and kind = $2 and status = 'pending' \
             returning payload, agent_model, conversation_id::text, message_id::text",
        )
        .bind(id)
        .bind(KIND)
        .fetch_optional(pg)
        .await;
    let (payload, agent_model, conversation_id, message_id) = match claimed {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!("{LOG} card {id}: could not close it: {e}");
            return;
        }
    };
    let agent_model = agent_model.unwrap_or_default();
    let asked = asked_of(&payload);
    let delivery = answer_card(&agent_model, &asked, Choice::Deny).await;
    tracing::info!(
        "{LOG} {agent_model}: card {id} expired unanswered; answered deny ({})",
        delivery_word(&delivery)
    );
    show(pg, redis, conversation_id, message_id, id, "expired").await;
    record(
        pg,
        "talaria",
        "agent.approval_denied",
        Choice::Deny,
        &agent_model,
        id,
        &payload,
        &delivery,
        true,
    )
    .await;
}

/// Close every card a restart left waiting: the past-due ones now, the rest
/// at their own deadline. Run once at boot, since the deadline timers lived
/// in the process that went away.
pub async fn expire_overdue(pg: &PgPool, redis: Redis) {
    let waiting: Vec<(String, Value)> = match sqlx::query_as(
        "select id::text, payload from chat_approvals where kind = $1 and status = 'pending'",
    )
    .bind(KIND)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("{LOG} could not read the waiting cards: {e}");
            return;
        }
    };
    for (id, payload) in waiting {
        arm(
            pg.clone(),
            redis.clone(),
            id,
            payload["expiresAt"].as_i64().unwrap_or(0),
        );
    }
}

// ── A person's answer ────────────────────────────────────────────────────────

/// The person answering, as the decision route knows them.
pub struct Person<'a> {
    pub user_id: &'a str,
    /// `admin` | `member`.
    pub role: &'a str,
    /// The audit actor (`talaria_session::actor_of`).
    pub actor: &'a str,
}

/// What became of a person's answer.
#[derive(Debug)]
pub enum Decided {
    /// Hermes took it: `approved` or `denied`.
    Answered(&'static str),
    /// The card was already settled; its status.
    Settled(String),
    /// This person may not answer this card.
    Forbidden,
    /// The card does not offer that answer.
    NotOffered,
    /// The agent was no longer waiting (its deadline passed, the turn
    /// ended or was stopped). The card now reads expired.
    Expired,
    /// The answer did not reach the agent; the card is still open.
    Undelivered(String),
    Db(sqlx::Error),
}

/// Answer a card for a person, through the agent's own door: `once` for
/// approve, `deny` for reject. Who may answer is checked here again, not
/// read off the card.
pub async fn decide(
    pg: &PgPool,
    redis: Redis,
    row: &ApprovalRow,
    person: &Person<'_>,
    choice: Choice,
) -> Decided {
    if row.status != "pending" {
        return Decided::Settled(row.status.clone());
    }
    let agent_model = row.agent_model.clone().unwrap_or_default();
    if row.owner_user_id.as_deref() != Some(person.user_id) {
        return Decided::Forbidden;
    }
    match talaria_agent_managers::manages_agent_model(pg, person.user_id, person.role, &agent_model)
        .await
    {
        Ok(true) => {}
        Ok(false) => return Decided::Forbidden,
        Err(e) => return Decided::Db(e),
    }
    let asked = asked_of(&row.payload);
    if !offered(&asked.choices).contains(&choice) {
        return Decided::NotOffered;
    }
    if now_ms() >= row.payload["expiresAt"].as_i64().unwrap_or(0) {
        expire(pg, redis, &row.id).await;
        return Decided::Expired;
    }
    let status = match choice {
        Choice::Once => "approved",
        Choice::Deny => "denied",
    };
    // Claimed before the answer leaves, so the deadline (which claims only a
    // waiting card) cannot answer the same request a second time.
    let claimed = sqlx::query(
        "update chat_approvals set status = $2, decided_by = $3::uuid, decided_at = now() \
          where id = $1::uuid and status = 'pending'",
    )
    .bind(&row.id)
    .bind(status)
    .bind(person.user_id)
    .execute(pg)
    .await;
    match claimed {
        Ok(done) if done.rows_affected() == 1 => {}
        Ok(_) => {
            return match sqlx::query_scalar::<_, String>(
                "select status from chat_approvals where id = $1::uuid",
            )
            .bind(&row.id)
            .fetch_one(pg)
            .await
            {
                Ok(now) => Decided::Settled(now),
                Err(e) => Decided::Db(e),
            };
        }
        Err(e) => return Decided::Db(e),
    }
    let delivery = answer_card(&agent_model, &asked, choice).await;
    let action = answer_action(choice, &delivery);
    match &delivery {
        Delivery::Taken => {
            show(
                pg,
                redis,
                row.conversation_id.clone(),
                row.message_id.clone(),
                &row.id,
                status,
            )
            .await;
            record(
                pg,
                person.actor,
                action,
                choice,
                &agent_model,
                &row.id,
                &row.payload,
                &delivery,
                false,
            )
            .await;
            Decided::Answered(status)
        }
        Delivery::NotPending(_) => {
            let _ = sqlx::query("update chat_approvals set status = 'expired' where id = $1::uuid")
                .bind(&row.id)
                .execute(pg)
                .await;
            show(
                pg,
                redis,
                row.conversation_id.clone(),
                row.message_id.clone(),
                &row.id,
                "expired",
            )
            .await;
            record(
                pg,
                person.actor,
                action,
                choice,
                &agent_model,
                &row.id,
                &row.payload,
                &delivery,
                true,
            )
            .await;
            Decided::Expired
        }
        Delivery::Failed(why) => {
            // Nothing reached the agent: the card opens again, and its
            // deadline still stands (expired now, if it passed meanwhile).
            let _ = sqlx::query(
                "update chat_approvals set status = 'pending', decided_by = null, decided_at = null \
                  where id = $1::uuid and status = $2",
            )
            .bind(&row.id)
            .bind(status)
            .execute(pg)
            .await;
            if now_ms() >= row.payload["expiresAt"].as_i64().unwrap_or(0) {
                expire(pg, redis, &row.id).await;
            }
            Decided::Undelivered(why.clone())
        }
    }
}

// ── Showing and recording ────────────────────────────────────────────────────

/// Repaint the card's chip with what became of it.
async fn show(
    pg: &PgPool,
    redis: Redis,
    conversation_id: Option<String>,
    message_id: Option<String>,
    id: &str,
    status: &str,
) {
    let (Some(conversation_id), Some(message_id)) = (conversation_id, message_id) else {
        return;
    };
    if let Err(e) = set_chip_status(
        pg,
        redis,
        &conversation_id,
        &message_id,
        &chip_id(id),
        status,
    )
    .await
    {
        tracing::warn!("{LOG} card {id}: could not show it {status}: {e}");
    }
}

fn delivery_word(delivery: &Delivery) -> String {
    match delivery {
        Delivery::Taken => "taken".into(),
        Delivery::NotPending(why) => format!("no longer waiting: {why}"),
        Delivery::Failed(why) => format!("not delivered: {why}"),
    }
}

/// The audit action for a person's answer, by what the agent did with it.
/// Only an answer Hermes took is an allow or a deny: one that reached an
/// agent no longer waiting took nothing (Hermes had already settled the
/// request), and is recorded as expired, never as an allow.
pub fn answer_action(choice: Choice, delivery: &Delivery) -> &'static str {
    match (delivery, choice) {
        (Delivery::Taken, Choice::Once) => "agent.approval_allowed",
        (Delivery::Taken, Choice::Deny) => "agent.approval_denied",
        _ => "agent.approval_expired",
    }
}

/// The audit body of an answered card.
pub fn audit_after(
    approval_id: &str,
    payload: &Value,
    delivery: &Delivery,
    choice: Choice,
    expired: bool,
) -> Value {
    let error = match delivery {
        Delivery::Taken => None,
        Delivery::NotPending(why) | Delivery::Failed(why) => Some(why.clone()),
    };
    json!({
        "choice": choice.as_str(),
        "description": payload["description"],
        "command": payload["command"],
        "runId": payload["runId"],
        "conversationId": payload["conversationId"],
        "approvalId": approval_id,
        "answered": matches!(delivery, Delivery::Taken),
        "expired": expired,
        "error": error,
    })
}

#[allow(clippy::too_many_arguments)] // one audit row's facts, each named at the call site
async fn record(
    pg: &PgPool,
    actor: &str,
    action: &str,
    choice: Choice,
    agent_model: &str,
    approval_id: &str,
    payload: &Value,
    delivery: &Delivery,
    expired: bool,
) {
    // `model` is unique, so this is the agent's own def or nothing.
    let agent_id =
        sqlx::query_scalar::<_, String>("select id::text from agent_defs where model = $1")
            .bind(agent_model)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
    talaria_audit::log_audit(
        pg,
        talaria_audit::AuditEntry {
            actor,
            action,
            target_type: "agent",
            target_id: agent_id.as_deref(),
            target_label: Some(agent_model),
            before: None,
            after: Some(audit_after(approval_id, payload, delivery, choice, expired)),
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONVERSATION: &str = "6113b5bc-6935-46e1-a67c-60bd58daf4f0";
    const OWNER: &str = "0b9d7c1e-2f4a-4c3b-9e8d-7a6b5c4d3e2f";

    fn attended() -> ApprovalTurn {
        ApprovalTurn::attended_chat(CONVERSATION)
    }

    fn developer_owned_by_a_manager() -> Facts {
        Facts {
            developer: true,
            owner: Some(OWNER.into()),
            owner_manages: true,
        }
    }

    fn choices(c: &[&str]) -> Vec<String> {
        c.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_developer_agent_s_request_on_its_manager_s_conversation_is_theirs_to_answer() {
        assert_eq!(
            decider(
                &attended(),
                &developer_owned_by_a_manager(),
                &choices(&["once", "deny"])
            ),
            Ok(OWNER.to_string())
        );
    }

    #[test]
    fn an_ordinary_agent_s_request_is_policy_never_a_card() {
        // `mode: manual`: an `rm -rf` arrives as a request, and is refused by
        // the door whoever is chatting, admin included.
        let facts = Facts {
            developer: false,
            ..developer_owned_by_a_manager()
        };
        assert_eq!(
            decider(
                &attended(),
                &facts,
                &choices(&["once", "session", "always", "deny"])
            ),
            Err(Refusal::NotDeveloper)
        );
    }

    #[test]
    fn only_an_attended_conversation_turn_can_carry_a_card() {
        // Each flow as the door reads it off its TurnAttribution: every one
        // but an attended chat turn names no conversation.
        for source in ["channel", "ticket", "research", "app", "workbench"] {
            let turn = ApprovalTurn {
                source: Some(source.into()),
                conversation_id: None,
            };
            assert_eq!(
                decider(
                    &turn,
                    &developer_owned_by_a_manager(),
                    &choices(&["once", "deny"])
                ),
                Err(Refusal::Unattended),
                "{source}"
            );
        }
        // An unattributed turn.
        assert_eq!(
            decider(
                &ApprovalTurn::default(),
                &developer_owned_by_a_manager(),
                &[]
            ),
            Err(Refusal::Unattended)
        );
    }

    #[test]
    fn owning_the_conversation_is_not_enough() {
        // A member chatting with a Developer Agent they do not manage.
        let facts = Facts {
            owner_manages: false,
            ..developer_owned_by_a_manager()
        };
        assert_eq!(
            decider(&attended(), &facts, &choices(&["once", "deny"])),
            Err(Refusal::OwnerDoesNotManage)
        );
        // A conversation that is not this agent's, or is gone.
        let facts = Facts {
            owner: None,
            owner_manages: false,
            ..developer_owned_by_a_manager()
        };
        assert_eq!(
            decider(&attended(), &facts, &choices(&["once", "deny"])),
            Err(Refusal::NoConversation)
        );
    }

    #[test]
    fn a_card_offers_once_and_deny_and_never_what_writes_the_config() {
        assert_eq!(
            offered(&choices(&["once", "session", "always", "deny"])),
            [Choice::Once, Choice::Deny]
        );
        assert_eq!(
            offered(&choices(&["once", "deny"])),
            [Choice::Once, Choice::Deny]
        );
        // A frame that lists nothing offers both.
        assert_eq!(offered(&[]), [Choice::Once, Choice::Deny]);
        // A request Hermes will not take `once` for has nothing to approve.
        assert_eq!(offered(&choices(&["deny"])), [Choice::Deny]);
        assert_eq!(
            decider(
                &attended(),
                &developer_owned_by_a_manager(),
                &choices(&["deny"])
            ),
            Err(Refusal::NothingToApprove)
        );
    }

    fn protected_write() -> ApprovalAsked {
        ApprovalAsked {
            run_id: "run_d87c07107c4244a89973e5bd2618264b".into(),
            request_id: "44e98bc6bc544867860b37c800ac72cc".into(),
            execution_generation: Some(1),
            command: "<write to AGENTS.md>".into(),
            description: "Write to protected agent-instruction file(s): AGENTS.md.".into(),
            choices: choices(&["once", "deny"]),
            asked_after_ms: None,
        }
    }

    #[test]
    fn the_card_carries_what_the_answer_is_addressed_to_and_round_trips() {
        let asked = protected_write();
        let payload = card_payload("dex", CONVERSATION, &asked, 1_792_699_082_578);
        assert_eq!(
            payload,
            json!({
                "runId": "run_d87c07107c4244a89973e5bd2618264b",
                "requestId": "44e98bc6bc544867860b37c800ac72cc",
                "executionGeneration": 1,
                "agentModel": "dex",
                "conversationId": CONVERSATION,
                "choices": ["once", "deny"],
                "command": "<write to AGENTS.md>",
                "description": "Write to protected agent-instruction file(s): AGENTS.md.",
                "expiresAt": 1_792_699_082_578_i64,
            })
        );
        // The decision route rebuilds the request from the row, and the body
        // it sends Hermes is exactly the three keys.
        let back = asked_of(&payload);
        assert_eq!(back.run_id, asked.run_id);
        assert_eq!(back.request_id, asked.request_id);
        assert_eq!(back.execution_generation, Some(1));
        assert_eq!(
            talaria_gateway::fleet_approvals::answer_body(&back, Choice::Once),
            json!({ "request_id": "44e98bc6bc544867860b37c800ac72cc",
                    "execution_generation": 1, "choice": "once" })
        );
        let chip = card_chip("a-1", OWNER, &payload);
        assert_eq!(chip["id"], "approval:a-1");
        assert_eq!(chip["kind"], "approval");
        assert_eq!(chip["actionKind"], KIND);
        assert_eq!(chip["actionId"], "a-1");
        assert_eq!(chip["deciderId"], OWNER);
        assert_eq!(chip["status"], "pending");
        assert_eq!(chip["command"], "<write to AGENTS.md>");
        assert_eq!(
            chip["summary"],
            "Write to protected agent-instruction file(s): AGENTS.md."
        );
        // One card per request: the reattached stream finds it again.
        assert_eq!(
            external_id(&asked),
            "hermes:run_d87c07107c4244a89973e5bd2618264b:44e98bc6bc544867860b37c800ac72cc"
        );
    }

    #[test]
    fn the_record_says_who_answered_what_and_whether_hermes_took_it() {
        let payload = card_payload("dex", CONVERSATION, &protected_write(), 0);
        let taken = audit_after("a-1", &payload, &Delivery::Taken, Choice::Once, false);
        assert_eq!(taken["choice"], "once");
        assert_eq!(taken["answered"], true);
        assert_eq!(taken["error"], Value::Null);
        assert_eq!(taken["runId"], "run_d87c07107c4244a89973e5bd2618264b");
        let late = audit_after(
            "a-1",
            &payload,
            &Delivery::NotPending("409 approval_not_pending".into()),
            Choice::Deny,
            true,
        );
        assert_eq!(late["answered"], false);
        assert_eq!(late["expired"], true);
        assert_eq!(late["error"], "409 approval_not_pending");
    }

    #[test]
    fn a_person_s_answer_the_agent_never_took_is_not_an_allow() {
        assert_eq!(
            answer_action(Choice::Once, &Delivery::Taken),
            "agent.approval_allowed"
        );
        assert_eq!(
            answer_action(Choice::Deny, &Delivery::Taken),
            "agent.approval_denied"
        );
        // Hermes' timeout (or a stop, or the run's end) settled it first: the
        // allow went nowhere, and the audit log must not count one.
        for late in [
            Delivery::NotPending("409 approval_not_pending".into()),
            Delivery::NotPending("already_resolved".into()),
        ] {
            assert_eq!(answer_action(Choice::Once, &late), "agent.approval_expired");
            assert_eq!(answer_action(Choice::Deny, &late), "agent.approval_expired");
            let after = audit_after("a-1", &json!({}), &late, Choice::Once, true);
            assert_eq!(after["choice"], "once");
            assert_eq!(after["answered"], false);
            assert_eq!(after["expired"], true);
        }
    }

    #[test]
    fn a_card_closes_on_the_earliest_clock_hermes_can_be_on() {
        let now = 1_792_699_100_000_i64;
        let window = card_window().as_millis() as i64;
        // A live frame: asked now, the full 115 s.
        assert_eq!(closes_at(now, None), Some(now + window));
        // Rebuilt by a reattach 16 s after the lost stream's last bytes: the
        // card closes 115 s after those, not after the reattach saw it.
        assert_eq!(
            closes_at(now, Some(now - 16_000)),
            Some(now - 16_000 + window)
        );
        // A bound in the future cannot lengthen it.
        assert_eq!(closes_at(now, Some(now + 60_000)), Some(now + window));
        // Too little left to read and answer: no card, the door refuses.
        assert_eq!(closes_at(now, Some(now - window + 9_000)), None);
        assert_eq!(closes_at(now, Some(now - 600_000)), None);
        assert_eq!(
            closes_at(now, Some(now - window + 10_000)),
            Some(now + 10_000)
        );
    }

    #[test]
    fn a_person_s_answer_on_its_way_holds_the_request_from_the_door() {
        // Claimed a moment ago: the `once` may not have reached Hermes yet,
        // and a `deny` from a reattach must not race it.
        assert!(answer_in_flight("approved", Some(200)));
        assert!(answer_in_flight("denied", Some(9_000)));
        // Long enough ago that the send is over, one way or the other.
        assert!(!answer_in_flight("approved", Some(60_000)));
        // Expired is the deadline's own close, not a person's answer.
        assert!(!answer_in_flight("expired", Some(200)));
        assert!(!answer_in_flight("approved", None));
    }
}
