// google/pending-actions.ts — the confirm-send half the Inbox reads.
//
// `queueAction` (the agent-drafting writer, with its approval announce) stays
// TS-side until the integrations plane crosses: the fleet app-server still
// queues through it. What the focus queue needs is `listPending` (the approval
// source) and `decideAction` (the execute/reject arm of `executeAction`).

use base64::Engine;
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::gateway::provider::http;
use crate::google_connections::get_access_token;
use crate::google_org::{get_org_access_token, get_org_targets};
use crate::secretbox::SecretBox;

const GMAIL_BASE: &str = "https://www.googleapis.com/gmail/v1/users/me";

/// One held outbound action (PendingAction). `created_ms` epoch — every caller
/// renders it through `as_iso`, which is what postgres.js + `toISOString`
/// produced in TS.
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub id: String,
    pub kind: String,
    pub summary: Option<String>,
    pub payload: Value,
    pub agent_model: Option<String>,
    pub owner_user_id: Option<String>,
    pub is_org: bool,
    pub status: String,
    pub created_ms: i64,
}

/// `listPending` — the actions a user should decide: their own personal ones,
/// plus — for an admin — the org-scoped ones. Newest first.
pub async fn list_pending(
    pg: &PgPool,
    user_id: &str,
    is_admin: bool,
) -> Result<Vec<PendingAction>, sqlx::Error> {
    // TS splices the admin arm in as SQL TEXT (`${isAdmin ? sql`or is_org = true` : sql``}`)
    // — for a member the clause VANISHES, it does not become `is_org = false`
    // (which would match every other person's personal actions). The literal
    // query keeps sqlx's static-string contract; `$2 and …` gates the arm so a
    // member's false bind short-circuits it to no rows, exactly like the
    // absent clause.
    #[allow(clippy::type_complexity)] // the listing's own columns, one each
    let rows: Vec<(
        String,
        String,
        Option<String>,
        Value,
        Option<String>,
        Option<String>,
        bool,
        String,
        i64,
    )> = sqlx::query_as(
        "select id::text, kind, summary, payload, agent_model, owner_user_id::text, is_org, status, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from google_pending_actions \
         where status = 'pending' \
           and ((is_org = false and owner_user_id = $1::uuid) or ($2 and is_org = true)) \
         order by created_at desc",
    )
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                kind,
                summary,
                payload,
                agent_model,
                owner_user_id,
                is_org,
                status,
                created_ms,
            )| PendingAction {
                id,
                kind,
                summary,
                payload,
                agent_model,
                owner_user_id,
                is_org,
                status,
                created_ms,
            },
        )
        .collect())
}

/// What a decision produced. `{status, message?}` — the TS return shape,
/// message present only on the not-connected/failed statuses.
#[derive(Debug, Clone)]
pub struct DecideOutcome {
    pub status: String,
    pub message: Option<String>,
}

/// `decideAction` — approve → execute (as the owner, or the org for org
/// actions), or reject → drop. `Err` is every path TS lets THROW (a DB error,
/// a token read that failed rather than answered null); the caller's catch is
/// the only thing that should see it.
pub async fn decide_action(
    pg: &PgPool,
    sb: &SecretBox,
    action_id: &str,
    actor_id: &str,
    actor_is_admin: bool,
    decision: &str,
    now_ms: i64,
) -> Result<Option<DecideOutcome>, String> {
    #[allow(clippy::type_complexity)] // the decided row's own columns, one each
    let action: Option<(String, Value, Option<String>, Option<String>, bool, String)> =
        sqlx::query_as(
            "select kind, payload, agent_model, owner_user_id::text, is_org, status \
             from google_pending_actions where id = $1::uuid",
        )
        .bind(action_id)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("google pending action read: {e}"))?;
    let Some((kind, payload, agent_model, owner_user_id, is_org, status)) = action else {
        return Ok(None);
    };
    let authorized = if is_org {
        actor_is_admin
    } else {
        owner_user_id.as_deref() == Some(actor_id)
    };
    if !authorized {
        return Ok(Some(DecideOutcome {
            status: "forbidden".into(),
            message: None,
        }));
    }
    if status != "pending" {
        return Ok(Some(DecideOutcome {
            status,
            message: None,
        })); // already decided
    }

    // A terminal decision resolves the approval line on the owner's (and the
    // decider's) brief. Detached and silent: the decider is waiting on this
    // response. Org actions decided by one admin leave other admins' briefs
    // to the scheduled sweep — the nudge is an optimization, never the floor.
    let nudge_brief = |ids: Vec<String>| {
        let pg = pg.clone();
        tokio::spawn(async move {
            let deps = crate::notify::NotifyDeps::publishing(pg, None);
            let _ = crate::notify::mark_brief_stale(&deps, &ids).await;
        });
    };
    let stale_ids: Vec<String> = [owner_user_id.clone(), Some(actor_id.to_string())]
        .into_iter()
        .flatten()
        .collect();

    if decision == "reject" {
        sqlx::query(
            "update google_pending_actions set status = 'rejected', decided_at = now(), decided_by = $2::uuid \
             where id = $1::uuid",
        )
        .bind(action_id)
        .bind(actor_id)
        .execute(pg)
        .await
        .map_err(|e| format!("google pending action update: {e}"))?;
        nudge_brief(stale_ids);
        return Ok(Some(DecideOutcome {
            status: "rejected".into(),
            message: None,
        }));
    }

    // Approve → resolve the executing token (org account, or the owner's).
    let token = if is_org {
        get_org_access_token(pg, sb, now_ms)
            .await
            .map_err(|e| e.to_string())?
    } else {
        get_access_token(
            pg,
            sb,
            owner_user_id
                .as_deref()
                .expect("personal actions carry an owner"),
            now_ms,
        )
        .await
        .map_err(|e| e.to_string())?
    };
    let Some(token) = token else {
        return Ok(Some(DecideOutcome {
            status: "not_connected".into(),
            message: Some(
                if is_org {
                    "Reconnect the org Google account to run this."
                } else {
                    "Reconnect Google to run this action."
                }
                .to_string(),
            ),
        }));
    };

    // Org actions land on the configured shared targets (calendar / send-as
    // alias) — except an org agent's mail, which carries ITS OWN address: the
    // stored override, else the org account's plus-address for its slug, else
    // the send-as target as before. Resolved at execution (not queueing) so an
    // alias edit between draft and approve is honored.
    let targets = if is_org {
        get_org_targets(pg).await.map_err(|e| e.to_string())?
    } else {
        Default::default()
    };
    let agent_from = if is_org && kind == "gmail_send" && agent_model.is_some() {
        org_agent_from_address(pg, agent_model.as_deref().expect("checked some"))
            .await
            .map_err(|e| e.to_string())?
    } else {
        None
    };

    let executed = if kind == "gmail_send" {
        send_message_with_token(
            &token,
            &payload,
            agent_from.as_deref().or(targets.send_as.as_deref()),
        )
        .await
        .map(|(id, thread_id)| json!({ "id": id, "threadId": thread_id }))
    } else if kind == "calendar_create" {
        create_event_with_token(&token, &payload, targets.calendar_id.as_deref()).await
    } else {
        Err(format!("unknown action kind: {kind}"))
    };

    match executed {
        Ok(result) => {
            sqlx::query(
                "update google_pending_actions \
                 set status = 'executed', result = $2, decided_at = now(), decided_by = $3::uuid \
                 where id = $1::uuid",
            )
            .bind(action_id)
            .bind(&result)
            .bind(actor_id)
            .execute(pg)
            .await
            .map_err(|e| format!("google pending action update: {e}"))?;
            nudge_brief(stale_ids);
            Ok(Some(DecideOutcome {
                status: "executed".into(),
                message: None,
            }))
        }
        Err(message) => {
            sqlx::query(
                "update google_pending_actions \
                 set status = 'failed', result = $2, decided_at = now(), decided_by = $3::uuid \
                 where id = $1::uuid",
            )
            .bind(action_id)
            .bind(json!({ "error": message }))
            .bind(actor_id)
            .execute(pg)
            .await
            .map_err(|e| format!("google pending action update: {e}"))?;
            nudge_brief(stale_ids);
            Ok(Some(DecideOutcome {
                status: "failed".into(),
                message: Some("Google rejected the action.".into()),
            }))
        }
    }
}

// ── The two executions ───────────────────────────────────────────────────────

/// RFC 2047 encode a header value if it has non-ASCII characters.
fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(value);
    format!("=?UTF-8?B?{b64}?=")
}

/// `sendMessageWithToken` — a plain-text email through the Gmail API. Headers
/// in the TS order, `\r\n` joined, base64url raw.
async fn send_message_with_token(
    token: &str,
    payload: &Value,
    from: Option<&str>,
) -> Result<(String, String), String> {
    let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
    let mut headers = vec![format!("To: {}", s("to"))];
    if let Some(from) = from {
        headers.push(format!("From: {from}"));
    }
    if !s("cc").is_empty() {
        headers.push(format!("Cc: {}", s("cc")));
    }
    if !s("bcc").is_empty() {
        headers.push(format!("Bcc: {}", s("bcc")));
    }
    headers.push(format!("Subject: {}", encode_header(s("subject"))));
    headers.push("MIME-Version: 1.0".to_string());
    headers.push("Content-Type: text/plain; charset=\"UTF-8\"".to_string());
    headers.push("Content-Transfer-Encoding: 8bit".to_string());
    // Headers, a blank separator line, then the body.
    let mime = format!("{}\r\n\r\n{}", headers.join("\r\n"), s("body"));
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mime.as_bytes());

    let res = http()
        .post(format!("{GMAIL_BASE}/messages/send"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(json!({ "raw": raw }).to_string())
        .send()
        .await
        .map_err(|e| format!("gmail send request: {e}"))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(format!("gmail send failed: {status} {text}"));
    }
    let sent: Value = res
        .json()
        .await
        .map_err(|e| format!("gmail send body: {e}"))?;
    Ok((
        sent.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        sent.get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    ))
}

/// `createEventWithToken` — an event on the target calendar, normalized to the
/// CalendarEvent shape for the result column.
async fn create_event_with_token(
    token: &str,
    payload: &Value,
    calendar_id: Option<&str>,
) -> Result<Value, String> {
    let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or_default();
    let all_day = payload
        .get("allDay")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let time_field = if all_day { "date" } else { "dateTime" };
    let mut start = serde_json::Map::new();
    start.insert(
        time_field.to_string(),
        Value::String(s("start").to_string()),
    );
    let mut end = serde_json::Map::new();
    end.insert(time_field.to_string(), Value::String(s("end").to_string()));
    let attendees: Vec<Value> = payload
        .get("attendees")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str())
                .map(|e| json!({ "email": e }))
                .collect()
        })
        .unwrap_or_default();
    let body = json!({
        "summary": s("summary"),
        "description": s("description"),
        "location": s("location"),
        "start": start,
        "end": end,
        "attendees": attendees,
    });
    let cal = encode_uri_component(calendar_id.filter(|c| !c.is_empty()).unwrap_or("primary"));
    let res = http()
        .post(format!(
            "https://www.googleapis.com/calendar/v3/calendars/{cal}/events?sendUpdates=all"
        ))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("calendar create request: {e}"))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(format!("calendar create failed: {status} {text}"));
    }
    let created: Value = res
        .json()
        .await
        .map_err(|e| format!("calendar create body: {e}"))?;
    Ok(normalize_event(&created))
}

/// calendar.ts `normalize` — the CalendarEvent jsonb the result column stores.
fn normalize_event(e: &Value) -> Value {
    let start = e.get("start");
    let end = e.get("end");
    let date_of = |o: Option<&Value>| -> Option<String> {
        o.and_then(|o| o.get("dateTime"))
            .or_else(|| o.and_then(|o| o.get("date")))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let all_day = start
        .and_then(|o| o.get("date"))
        .is_some_and(Value::is_string)
        && start.and_then(|o| o.get("dateTime")).is_none();
    json!({
        "id": e.get("id").and_then(Value::as_str).unwrap_or_default(),
        "summary": e.get("summary").and_then(Value::as_str).unwrap_or("(no title)"),
        "start": date_of(start),
        "end": date_of(end),
        "allDay": all_day,
        "location": e.get("location").and_then(Value::as_str),
        "htmlLink": e.get("htmlLink").and_then(Value::as_str),
        "attendees": e.get("attendees").and_then(Value::as_array).map(|a| {
            a.iter().filter_map(|x| x.get("email").and_then(Value::as_str))
                .filter(|s| !s.is_empty()).collect::<Vec<_>>()
        }).unwrap_or_default(),
    })
}

// ── Org agent addressing (aliasing.ts, the send-path half) ───────────────────

/// `agentAddressIdentity` — the agent's slug + stored alias override.
async fn agent_address_identity(
    pg: &PgPool,
    model: &str,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("select slug, email_alias from agent_defs where model = $1")
            .bind(model)
            .fetch_optional(pg)
            .await?;
    Ok(row)
}

/// `getOrgEmail` — the connected org account's email alone, or null. The
/// aliasing derivation reads this on the send path — plus-addresses hang off
/// the org account, so no connection means no derived addresses (an agent's
/// override still wins).
async fn get_org_email(pg: &PgPool) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("select email, refresh_token_enc from google_org_connection where id = 1")
            .fetch_optional(pg)
            .await?;
    Ok(row
        .filter(|(_, refresh)| refresh.is_some())
        .and_then(|(email, _)| email))
}

/// `orgAgentFromAddress` — the From address an org agent sends as: its alias
/// override, else its derived plus-address of the org account. null when
/// neither resolves — the caller falls back to the org sendAs target.
async fn org_agent_from_address(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    let identity = agent_address_identity(pg, agent_model).await?;
    let Some((slug, email_alias)) = identity else {
        return Ok(None);
    };
    let org_email = get_org_email(pg).await?;
    Ok(agent_from_address(
        slug.as_str(),
        email_alias.as_deref(),
        org_email.as_deref(),
    ))
}

/// aliasing.ts `plusTag` — a slug folded into a plus-address tag: lowercase,
/// [a-z0-9-], single dashes, nothing else.
fn plus_tag(slug: &str) -> String {
    let lower = slug.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut pending_dash = false;
    for ch in lower.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// aliasing.ts `plusAddress` — the org account's plus-address for a tag. An
/// org email that is ITSELF plus-addressed has its tag replaced, not stacked.
fn plus_address(org_email: &str, tag: &str) -> Option<String> {
    let at = org_email.rfind('@')?;
    if at < 1 || at == org_email.len() - 1 {
        return None;
    }
    let local = org_email[..at].split('+').next().unwrap_or_default();
    let domain = &org_email[at + 1..];
    let clean = plus_tag(tag);
    if local.is_empty() || domain.is_empty() || clean.is_empty() {
        return None;
    }
    Some(format!("{local}+{clean}@{domain}"))
}

/// aliasing.ts `agentFromAddress` — override, else derived plus-address, else
/// null. Never called for personal assistants — they send as their owner's
/// account, where no alias applies.
fn agent_from_address(
    slug: &str,
    email_alias: Option<&str>,
    org_email: Option<&str>,
) -> Option<String> {
    let override_ = email_alias.map(str::trim).filter(|s| !s.is_empty());
    if let Some(o) = override_ {
        return Some(o.to_string());
    }
    let org = org_email?;
    plus_address(org, slug)
}

/// encodeURIComponent — the calendar id rides a path segment, so its own
/// special characters (an email-shaped id's `@`) must be escaped.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if unreserved {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_address_rules() {
        assert_eq!(plus_tag("Field Ops!"), "field-ops");
        assert_eq!(plus_tag("--x--"), "x");
        assert_eq!(
            plus_address("jon@x.com", "triage").as_deref(),
            Some("jon+triage@x.com")
        );
        // An already-plus-addressed org email replaces its tag.
        assert_eq!(
            plus_address("jon+old@x.com", "new").as_deref(),
            Some("jon+new@x.com")
        );
        assert_eq!(plus_address("jon@", "x"), None);
        assert_eq!(plus_address("@x.com", "x"), None);
        assert_eq!(plus_address("jon@x.com", "!!!"), None);
    }

    #[test]
    fn agent_from_address_precedence() {
        assert_eq!(
            agent_from_address("triage", Some(" ops@x.com "), Some("jon@x.com")).as_deref(),
            Some("ops@x.com"),
            "the stored override wins, trimmed"
        );
        assert_eq!(
            agent_from_address("triage", None, Some("jon@x.com")).as_deref(),
            Some("jon+triage@x.com")
        );
        assert_eq!(agent_from_address("triage", None, None), None);
        // A blank override is no override.
        assert_eq!(
            agent_from_address("triage", Some("   "), Some("jon@x.com")).as_deref(),
            Some("jon+triage@x.com")
        );
    }

    #[test]
    fn header_encoding() {
        assert_eq!(encode_header("hello"), "hello");
        assert_eq!(encode_header("Réunion"), "=?UTF-8?B?UsOpdW5pb24=?=");
    }

    #[test]
    fn event_normalization() {
        let raw = json!({
            "id": "e1",
            "start": { "date": "2026-09-01" },
            "end": { "date": "2026-09-02" },
            "attendees": [{ "email": "a@x.com" }, { "email": "" }, {}],
        });
        let n = normalize_event(&raw);
        assert_eq!(n["allDay"], json!(true));
        assert_eq!(n["summary"], json!("(no title)"));
        assert_eq!(n["attendees"], json!(["a@x.com"]));
    }
}
