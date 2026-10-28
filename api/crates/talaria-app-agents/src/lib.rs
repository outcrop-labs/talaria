// An app's code driving an agent turn — `ctx.agents.turn(...)` in
// @talaria/sdk/server, which the app host sends here
// (`POST /api/internal/apps/agent-turn`, the root-derived service credential).
//
// WHAT IT IS: one message to one fleet agent, through the door every
// Talaria-driven persona turn takes (talaria-gateway `fleet_chat`): the
// container held for while it restarts, credentials sealed out of the
// message, Hermes's approval prompts refused, and the turn's key recorded so
// every call the agent makes for it is booked to ledger source `app`
// (`TurnAttribution`, #584). A `session` key lands the turn on a Hermes
// SESSION, where the agent keeps the transcript (#582) — Talaria stores no
// copy of it; without one, the turn is a fresh completion. The answer streams
// back as it lands, one JSON line per event, so the caller can watch the
// agent work or just await the reply.
//
// WHO MAY DRIVE WHOM is decided here, from what the host says started the
// app's code — never from anything the app says:
//
//   request   a signed-in person's request to the app: only an agent THEY may
//             use (`usable_agent_gate` — never someone else's personal
//             assistant, never an agent their access list leaves out);
//   mcp       an agent calling one of the app's MCP tools: only that agent,
//             and only when the gateway proved it by its own key;
//   trigger   a schedule or a platform event: no agent at all. A triggered run
//             acts as the app with no user, and lending it an agent would
//             give every app trigger the reach of whichever agent it named.
//             An admin-chosen agent for an app's triggers is a follow-up.
//
// LIVENESS, not a fixed deadline: an agent may work for a long time on one
// turn, so the turn is abandoned only after `IDLE` with no frame at all from
// the agent — and stopped in the container when the caller leaves.
//
// THE CALLER IS WATCHED, NOT ONLY NOTICED AT THE NEXT WRITE. The host hangs
// up when its own request goes (the person's tab, a tool's deadline), which
// closes the answer channel. The drive waits on that close beside the agent's
// next frame, so it lets go of the turn the moment the caller does. Reading
// on until the agent next said something was not harmless: a one-shot that
// Hermes refuses because its derived session lost a turn is moved to a fresh
// session inside that read (talaria-gateway's lost-turn watch), so a caller
// that gave up during the refusal's wait still had its turn moved, and run,
// for nobody.

use std::future::Future;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::mpsc;

use talaria_gateway::fleet_chat::{AgentStreamEvent, AgentStreamParser, ChatStream, stop_run};
use talaria_gateway::usage::TurnAttribution;

/// How long a turn may go with no frame from the agent before it is
/// abandoned. Tool calls stream frames as they start and end, so this is
/// silence, not work.
pub const IDLE: Duration = Duration::from_secs(10 * 60);
/// How long to hold for an agent that is not serving yet (a restart under a
/// config roll) — the chat plane's own window.
pub const HOLD_MS: u64 = 120_000;
/// The longest message a turn takes, in chars. A turn is a message to an
/// agent, not a document upload.
pub const MESSAGE_MAX: usize = 100_000;
pub const SYSTEM_MAX: usize = 20_000;
pub const SESSION_KEY_MAX: usize = 200;

/// What started the app's code, as the host authenticated it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// A signed-in person's request to the app's server.
    Request { user_id: String },
    /// An agent calling one of the app's MCP tools. `proven` is the gateway's
    /// word that the call came on that agent's own key.
    Mcp { agent: String, proven: bool },
    /// A schedule or a platform event: the app itself.
    Trigger { trigger: String },
}

/// One turn, as the host asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRequest {
    pub app: String,
    pub actor: Actor,
    /// The agent's fleet id (its model id).
    pub agent: String,
    pub message: String,
    /// The turn's ephemeral system prompt (Hermes takes one per turn).
    pub system: Option<String>,
    /// The app's key for a conversation; same key, same Hermes session.
    pub session: Option<String>,
    pub effort: Option<String>,
}

fn text_field(
    body: &Value,
    key: &str,
    max: usize,
    required: bool,
) -> Result<Option<String>, String> {
    match body.get(key) {
        None | Some(Value::Null) if !required => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => {
            if s.chars().count() > max {
                Err(format!("\"{key}\" is longer than {max} characters"))
            } else {
                Ok(Some(s.clone()))
            }
        }
        _ => Err(format!("\"{key}\" must be a non-empty string")),
    }
}

/// The host's body, read: `{ app, actor: { kind, … }, agent, message,
/// system?, session?, effort? }`.
pub fn parse_request(body: &Value) -> Result<TurnRequest, String> {
    let app = text_field(body, "app", 64, true)?.unwrap_or_default();
    if !talaria_users::slug_ok(&app) {
        return Err("\"app\" is not an app slug".into());
    }
    let actor = body.get("actor").cloned().unwrap_or(Value::Null);
    let actor = match actor.get("kind").and_then(Value::as_str) {
        Some("request") => Actor::Request {
            user_id: text_field(&actor, "userId", 64, true)?.unwrap_or_default(),
        },
        Some("mcp") => Actor::Mcp {
            agent: text_field(&actor, "agent", 200, true)?.unwrap_or_default(),
            proven: actor.get("proven").and_then(Value::as_bool) == Some(true),
        },
        Some("trigger") => Actor::Trigger {
            trigger: text_field(&actor, "trigger", 64, true)?.unwrap_or_default(),
        },
        _ => return Err("\"actor.kind\" must be request, mcp or trigger".into()),
    };
    let agent = text_field(body, "agent", 200, true)?.unwrap_or_default();
    let message = text_field(body, "message", MESSAGE_MAX, true)?.unwrap_or_default();
    let system = text_field(body, "system", SYSTEM_MAX, false)?;
    let session = text_field(body, "session", SESSION_KEY_MAX, false)?;
    let effort = text_field(body, "effort", 32, false)?;
    Ok(TurnRequest {
        app,
        actor,
        agent,
        message,
        system,
        session,
        effort,
    })
}

/// The pure half of who-may-drive-whom: what the actor alone decides, before
/// any database read. `Some` is the refusal sentence.
pub fn actor_refusal(req: &TurnRequest) -> Option<String> {
    match &req.actor {
        Actor::Trigger { trigger } => Some(format!(
            "app \"{}\" may not drive an agent from its trigger \"{trigger}\": a triggered run acts as the app, with no user, and no agent is lent to it",
            req.app
        )),
        Actor::Mcp { agent, proven } if !proven => Some(format!(
            "app \"{}\" may not drive an agent from this tool call: the calling agent \"{agent}\" was not proven by its own key",
            req.app
        )),
        Actor::Mcp { agent, .. } if *agent != req.agent => Some(format!(
            "app \"{}\" may not drive agent \"{}\" from an MCP tool: from a tool, only the agent calling it (\"{agent}\") may be driven",
            req.app, req.agent
        )),
        _ => None,
    }
}

/// Why this turn may not run, or None. The app enabled with a ready build,
/// the actor's rule (`actor_refusal`), the person's agent access for a
/// request, the agent a rendered member of the fleet, and the app under its
/// budget.
pub async fn refusal(pg: &PgPool, req: &TurnRequest) -> Result<Option<String>, sqlx::Error> {
    if !talaria_apps::enabled_app_slugs(pg).await.contains(&req.app) {
        return Ok(Some(format!("app \"{}\" is not enabled", req.app)));
    }
    let build = talaria_users::app_build_status(&req.app);
    if build.status != "ready" {
        return Ok(Some(format!(
            "app \"{}\" has no ready build (its build is {})",
            req.app, build.status
        )));
    }
    if let Some(why) = actor_refusal(req) {
        return Ok(Some(why));
    }
    if let Actor::Request { user_id } = &req.actor {
        let role = talaria_users::get_user_role(pg, user_id).await?;
        let usable = talaria_fleet_agents::usable_agent_gate(pg, user_id, &role).await?;
        if !usable(&req.agent) {
            return Ok(Some(format!(
                "app \"{}\" may not drive agent \"{}\" for this person: it is not an agent they may use",
                req.app, req.agent
            )));
        }
    }
    if !talaria_fleet_layout::read_manifest()
        .await
        .iter()
        .any(|m| m.model == req.agent)
    {
        return Ok(Some(format!(
            "\"{}\" is not a running agent of this fleet",
            req.agent
        )));
    }
    // The app's budget, as the relay holds every other call an app makes to
    // it (`app:<slug>`, summed over every row under the app's stamps): an app
    // over its budget starts no agent turn either. The agent's own calls
    // still count against the agent's budget, as any turn's do.
    let subject = talaria_gateway::app_keys::app_key_name(&req.app);
    if let Some(denial) = talaria_gateway::budget::check_budget(
        pg,
        &subject,
        talaria_gateway::budget::BudgetLimits::default(),
    )
    .await
    {
        return Ok(Some(talaria_gateway::budget::budget_message(&denial)));
    }
    Ok(None)
}

/// The Hermes session a key names: one per app, per person (a request) or
/// per calling agent (a tool), per key — so an app's key can never reach a
/// chat's or a work session's transcript, or another person's, whatever the
/// app passes. UUID-shaped, the form every Talaria-keyed session takes.
pub fn session_id(app: &str, actor: &Actor, key: &str) -> String {
    let scope = match actor {
        Actor::Request { user_id } => format!("user:{user_id}"),
        Actor::Mcp { agent, .. } => format!("agent:{agent}"),
        Actor::Trigger { trigger } => format!("trigger:{trigger}"),
    };
    let digest = Sha256::digest(format!("talaria-app-session\0{app}\0{scope}\0{key}").as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    // RFC 9562 version 8 (custom), variant 10.
    b[6] = (b[6] & 0x0f) | 0x80;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// One line of the answer the host reads.
pub fn line(v: Value) -> String {
    format!("{v}\n")
}

/// The event a parsed agent frame becomes for the app, or None for a frame
/// the app has no use for (usage, reasoning, the run handle).
pub fn event_of(ev: &AgentStreamEvent) -> Option<Value> {
    match ev {
        AgentStreamEvent::Content { text } if !text.is_empty() => {
            Some(json!({ "type": "text", "text": text }))
        }
        AgentStreamEvent::Tool {
            name,
            label,
            status,
            ..
        } => Some(json!({
            "type": "tool",
            "name": name,
            "status": status.as_deref().unwrap_or("running"),
            "label": label,
        })),
        _ => None,
    }
}

/// What a finished turn answers.
#[derive(Debug, Default)]
struct Reply {
    text: String,
    tools: Vec<String>,
    error: Option<String>,
    run_id: Option<String>,
}

/// Open the turn: on the keyed session, or as a fresh completion.
async fn open(pg: &PgPool, req: &TurnRequest) -> Result<(ChatStream, Option<String>), String> {
    let attribution = TurnAttribution {
        pg: pg.clone(),
        agent_model: req.agent.clone(),
        source: "app".into(),
        ref_id: None,
        task_id: None,
        attended: false,
    };
    match &req.session {
        Some(key) => {
            let id = session_id(&req.app, &req.actor, key);
            // Created on first use, found after (a 409 is the steady state):
            // the transcript is the container's, so nothing here says whether
            // the session had turns before.
            let stream = talaria_gateway::fleet_chat::proxy_session_turn(
                &req.agent,
                &id,
                req.system.as_deref(),
                &Value::String(req.message.clone()),
                req.effort.as_deref(),
                Some(HOLD_MS),
                false,
                Some(attribution),
            )
            .await
            .ok_or_else(|| {
                format!(
                    "the session for key \"{key}\" was deleted in the agent and cannot be used again; use another key"
                )
            })?;
            Ok((stream, Some(id)))
        }
        None => {
            let mut messages = Vec::new();
            if let Some(system) = &req.system {
                messages.push(json!({ "role": "system", "content": system }));
            }
            messages.push(json!({ "role": "user", "content": req.message }));
            let payload = talaria_gateway::fleet_chat::chat_payload(
                &req.agent,
                &Value::Array(messages),
                req.effort.as_deref(),
            );
            let stream =
                talaria_gateway::fleet_chat::proxy_chat(&payload, Some(HOLD_MS), Some(attribution))
                    .await;
            Ok((stream, None))
        }
    }
}

/// Run the turn, sending each event to `tx` as one JSON line and ending with
/// `{type:"done", text, tools, session}` or `{type:"error", error}`. A
/// closed `tx` is the caller gone, watched for the whole turn (see the
/// header): the turn is let go of at once, the agent's run is stopped, and
/// nothing more is sent.
pub async fn drive(pg: &PgPool, req: &TurnRequest, tx: mpsc::Sender<String>) {
    drive_opened(open(pg, req), req, tx).await
}

/// [`drive`] with the turn's opening handed in, so the tests can stand in for
/// the agent.
async fn drive_opened(
    opening: impl Future<Output = Result<(ChatStream, Option<String>), String>>,
    req: &TurnRequest,
    tx: mpsc::Sender<String>,
) {
    // The opening holds for an agent that is restarting (`HOLD_MS`). A caller
    // that leaves meanwhile has nobody for the turn to be sent to. An opening
    // dropped once the agent's stream is open (its key still being recorded)
    // drops that stream unread, and the gateway reads it on just far enough
    // to stop the turn it names (talaria-gateway fleet_abandoned.rs).
    let opened = tokio::select! {
        biased;
        () = tx.closed() => {
            tracing::info!(
                "[app-agents] {}: the caller left while agent \"{}\"'s turn was opening; it is let go of (a turn the agent already took is stopped by the gateway)",
                req.app,
                req.agent
            );
            return;
        }
        opened = opening => opened,
    };
    let (upstream, session) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            let _ = tx.send(line(json!({ "type": "error", "error": e }))).await;
            return;
        }
    };
    if let Some(canned) = upstream.canned {
        let error = match canned {
            "mock" => format!("\"{}\" is not a rendered agent", req.agent),
            _ => format!(
                "agent \"{}\" did not come back within {} s (it is restarting or down)",
                req.agent,
                HOLD_MS / 1000
            ),
        };
        let _ = tx
            .send(line(json!({ "type": "error", "error": error })))
            .await;
        return;
    }
    if !upstream.ok() {
        let error = format!("agent \"{}\" answered {}", req.agent, upstream.status);
        let _ = tx
            .send(line(json!({ "type": "error", "error": error })))
            .await;
        return;
    }

    let mut body = upstream.body;
    let mut parser = AgentStreamParser::new();
    let mut reply = Reply::default();
    let mut gone = false;
    loop {
        let next = tokio::select! {
            biased;
            () = tx.closed() => {
                gone = true;
                break;
            }
            next = tokio::time::timeout(IDLE, body.next()) => next,
        };
        let chunk = match next {
            Err(_) => {
                reply.error = Some(format!(
                    "agent \"{}\" sent nothing for {} minutes; the turn was abandoned",
                    req.agent,
                    IDLE.as_secs() / 60
                ));
                break;
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                reply.error = Some(format!("the agent's stream broke: {e}"));
                break;
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        for ev in parser.feed(&chunk) {
            if !fold(&mut reply, &ev, &tx).await {
                gone = true;
            }
        }
        if gone || reply.error.is_some() {
            break;
        }
    }
    if !gone && reply.error.is_none() {
        for ev in parser.finish() {
            if !fold(&mut reply, &ev, &tx).await {
                gone = true;
            }
        }
    }
    if gone || reply.error.is_some() {
        // Nobody is reading (or the turn was abandoned): stop the agent's run
        // rather than let it work on for nobody. The stream goes first. On
        // the completions arm, letting go of it is the stop: the gateway's
        // door stops a completion its reader let go of (fleet_abandoned.rs),
        // and that arm names no `run.started` here. On the session arm the
        // run id below is the stop.
        drop(body);
        if let Some(run) = &reply.run_id
            && let Err(e) = stop_run(&req.agent, run).await
        {
            tracing::warn!("[app-agents] {}: could not stop run {run}: {e}", req.app);
        }
    }
    if gone {
        return;
    }
    let last = match reply.error {
        Some(error) => json!({ "type": "error", "error": error }),
        None => json!({
            "type": "done",
            "text": reply.text,
            "tools": reply.tools,
            "session": session,
        }),
    };
    let _ = tx.send(line(last)).await;
}

/// Fold one frame into the reply and send its event. False when the caller
/// is gone.
async fn fold(reply: &mut Reply, ev: &AgentStreamEvent, tx: &mpsc::Sender<String>) -> bool {
    match ev {
        AgentStreamEvent::Content { text } => reply.text.push_str(text),
        AgentStreamEvent::Tool { name, .. } => {
            if !reply.tools.contains(name) {
                reply.tools.push(name.clone());
            }
        }
        AgentStreamEvent::RunStarted { run_id } => reply.run_id = Some(run_id.clone()),
        AgentStreamEvent::Error { message } => reply.error = Some(message.clone()),
        _ => {}
    }
    match event_of(ev) {
        Some(v) => tx.send(line(v)).await.is_ok(),
        None => !tx.is_closed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(actor: Actor, agent: &str) -> TurnRequest {
        TurnRequest {
            app: "crm".into(),
            actor,
            agent: agent.into(),
            message: "hi".into(),
            system: None,
            session: None,
            effort: None,
        }
    }

    #[test]
    fn reads_the_hosts_body() {
        let r = parse_request(&json!({
            "app": "crm",
            "actor": { "kind": "mcp", "agent": "ada-ops", "proven": true },
            "agent": "ada-ops",
            "message": "summarise",
            "session": "lead-42",
        }))
        .unwrap();
        assert_eq!(
            r.actor,
            Actor::Mcp {
                agent: "ada-ops".into(),
                proven: true
            }
        );
        assert_eq!(r.session.as_deref(), Some("lead-42"));
        assert!(
            parse_request(
                &json!({"app": "crm", "actor": {"kind": "admin"}, "agent": "a", "message": "m"})
            )
            .unwrap_err()
            .contains("actor.kind")
        );
        assert!(
            parse_request(&json!({"app": "crm", "actor": {"kind": "request", "userId": "u"}, "agent": "a", "message": " "}))
                .unwrap_err()
                .contains("\"message\"")
        );
        assert!(
            parse_request(&json!({"app": "../x", "actor": {"kind": "trigger", "trigger": "t"}, "agent": "a", "message": "m"}))
                .unwrap_err()
                .contains("not an app slug")
        );
    }

    #[test]
    fn a_trigger_drives_no_agent_and_a_tool_only_its_caller() {
        let t = actor_refusal(&req(
            Actor::Trigger {
                trigger: "nightly".into(),
            },
            "ada-ops",
        ))
        .unwrap();
        assert!(t.contains("from its trigger \"nightly\""), "{t}");
        let unproven = actor_refusal(&req(
            Actor::Mcp {
                agent: "ada-ops".into(),
                proven: false,
            },
            "ada-ops",
        ))
        .unwrap();
        assert!(unproven.contains("not proven"), "{unproven}");
        let other = actor_refusal(&req(
            Actor::Mcp {
                agent: "ada-ops".into(),
                proven: true,
            },
            "bob-assistant",
        ))
        .unwrap();
        assert!(
            other.contains("only the agent calling it (\"ada-ops\")"),
            "{other}"
        );
        assert_eq!(
            actor_refusal(&req(
                Actor::Mcp {
                    agent: "ada-ops".into(),
                    proven: true
                },
                "ada-ops"
            )),
            None
        );
        // A request's rule needs the database (usable_agent_gate).
        assert_eq!(
            actor_refusal(&req(
                Actor::Request {
                    user_id: "u".into()
                },
                "ada-ops"
            )),
            None
        );
    }

    #[test]
    fn a_session_key_is_scoped_to_the_app_and_the_actor() {
        let alice = Actor::Request {
            user_id: "alice".into(),
        };
        let bob = Actor::Request {
            user_id: "bob".into(),
        };
        let a = session_id("crm", &alice, "lead-42");
        assert_eq!(a, session_id("crm", &alice, "lead-42"), "stable");
        assert_ne!(a, session_id("crm", &bob, "lead-42"), "per person");
        assert_ne!(a, session_id("notes", &alice, "lead-42"), "per app");
        assert_ne!(a, session_id("crm", &alice, "lead-43"), "per key");
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "8", "version 8");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"), "variant");
    }

    // ── The drive and a caller who leaves ───────────────────────────────

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use talaria_gateway::fleet_chat::ByteStream;

    type Item = <ByteStream as futures_util::Stream>::Item;

    fn frame(s: &'static str) -> Item {
        Ok(s.as_bytes().into())
    }

    const ROLE: &str = "data: {\"id\": \"chatcmpl-1\", \"choices\": [{\"index\": 0, \"delta\": {\"role\": \"assistant\"}, \"finish_reason\": null}]}\n\n";
    const WORDS: &str = "data: {\"id\": \"chatcmpl-1\", \"choices\": [{\"index\": 0, \"delta\": {\"content\": \"on it\"}, \"finish_reason\": null}]}\n\n";
    const FINISH: &str = "data: {\"id\": \"chatcmpl-1\", \"choices\": [{\"index\": 0, \"delta\": {}, \"finish_reason\": \"stop\"}]}\n\ndata: [DONE]\n\n";

    fn opened(body: ByteStream) -> Result<(ChatStream, Option<String>), String> {
        Ok((
            ChatStream {
                status: 200,
                content_type: "text/event-stream".into(),
                body,
                canned: None,
                hermes_session: None,
            },
            None,
        ))
    }

    /// The agent's stream as the lost-turn watch makes it for a refused
    /// one-shot: the role chunk, then a long wait for Hermes' answer, and the
    /// move to a fresh session made INSIDE the read that answer ends (the
    /// watch sends the turn again from within its poll). `moved` counts the
    /// moves that ran.
    fn refused_after(wait: Duration, moved: Arc<AtomicUsize>) -> ByteStream {
        Box::pin(futures_util::stream::unfold(0u8, move |step| {
            let moved = moved.clone();
            async move {
                match step {
                    0 => Some((frame(ROLE), 1)),
                    1 => {
                        tokio::time::sleep(wait).await;
                        moved.fetch_add(1, Ordering::SeqCst);
                        Some((frame(WORDS), 2))
                    }
                    _ => None,
                }
            }
        }))
    }

    async fn drained(mut rx: mpsc::Receiver<String>) -> Vec<Value> {
        let mut lines = Vec::new();
        while let Some(l) = rx.recv().await {
            lines.push(serde_json::from_str(l.trim()).unwrap());
        }
        lines
    }

    /// A caller that gives up while Hermes holds a refused turn is seen at
    /// once: the drive ends without reading on, so the move the refusal would
    /// set off never runs. On the old drive (a read, then a check on the next
    /// write) the move ran: that was R12's two app resends whose `curl` had
    /// timed out.
    #[tokio::test]
    async fn a_caller_that_leaves_mid_turn_is_let_go_before_the_turn_moves() {
        let moved = Arc::new(AtomicUsize::new(0));
        let body = refused_after(Duration::from_millis(400), moved.clone());
        let (tx, rx) = mpsc::channel::<String>(64);
        let turn = req(
            Actor::Request {
                user_id: "u".into(),
            },
            "ada-ops",
        );
        let drive =
            tokio::spawn(async move { drive_opened(async { opened(body) }, &turn, tx).await });
        tokio::time::sleep(Duration::from_millis(60)).await;
        drop(rx);
        tokio::time::timeout(Duration::from_millis(200), drive)
            .await
            .expect("the drive lets go as soon as the caller does")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            moved.load(Ordering::SeqCst),
            0,
            "nothing read on, nothing moved"
        );
    }

    /// A caller that leaves while the opening still holds for the agent (a
    /// container restarting under a roll) is seen too: the turn is never
    /// sent.
    #[tokio::test]
    async fn a_caller_that_leaves_during_the_hold_sends_nothing() {
        let sent = Arc::new(AtomicUsize::new(0));
        let counted = sent.clone();
        let opening = async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            counted.fetch_add(1, Ordering::SeqCst);
            opened(Box::pin(futures_util::stream::empty()))
        };
        let (tx, rx) = mpsc::channel::<String>(64);
        let turn = req(
            Actor::Request {
                user_id: "u".into(),
            },
            "ada-ops",
        );
        let drive = tokio::spawn(async move { drive_opened(opening, &turn, tx).await });
        tokio::time::sleep(Duration::from_millis(60)).await;
        drop(rx);
        tokio::time::timeout(Duration::from_millis(200), drive)
            .await
            .expect("the drive lets go during the hold")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(sent.load(Ordering::SeqCst), 0);
    }

    /// The control: a caller that stays reads the turn through to its answer,
    /// a slow one included.
    #[tokio::test]
    async fn a_caller_that_stays_gets_the_answer() {
        let moved = Arc::new(AtomicUsize::new(0));
        let slow = refused_after(Duration::from_millis(100), moved.clone());
        let body: ByteStream = Box::pin(slow.chain(futures_util::stream::iter([frame(FINISH)])));
        let (tx, rx) = mpsc::channel::<String>(64);
        let turn = req(
            Actor::Request {
                user_id: "u".into(),
            },
            "ada-ops",
        );
        let drive =
            tokio::spawn(async move { drive_opened(async { opened(body) }, &turn, tx).await });
        let lines = drained(rx).await;
        drive.await.unwrap();
        assert_eq!(moved.load(Ordering::SeqCst), 1);
        assert_eq!(
            lines,
            vec![
                json!({"type": "text", "text": "on it"}),
                json!({"type": "done", "text": "on it", "tools": [], "session": null}),
            ]
        );
    }

    #[test]
    fn the_app_sees_words_and_tools_only() {
        assert_eq!(
            event_of(&AgentStreamEvent::Content { text: "hi".into() }),
            Some(json!({"type": "text", "text": "hi"}))
        );
        assert_eq!(
            event_of(&AgentStreamEvent::Tool {
                id: Some("1".into()),
                name: "terminal".into(),
                label: "ls".into(),
                status: None
            }),
            Some(json!({"type": "tool", "name": "terminal", "status": "running", "label": "ls"}))
        );
        assert_eq!(
            event_of(&AgentStreamEvent::Reasoning { text: "hm".into() }),
            None
        );
        assert_eq!(
            event_of(&AgentStreamEvent::Usage {
                prompt_tokens: 1,
                completion_tokens: 1
            }),
            None
        );
    }
}
