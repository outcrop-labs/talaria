// Invites — the third door in (after env allow-lists and verified sign-up
// domains). An admin invites an email; the
// invitee gets a transactional email with a /join link and signs in with
// Google on that address. Invites expire, revoke instantly, and re-send with
// a fresh token.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sqlx::PgPool;

use crate::email::{EmailInput, SendOutcome, email_escape, email_shell, send_email};
use crate::instance::instance_base_url;
use crate::org::org_profile;
use crate::secretbox::SecretBox;

const TTL_DAYS: i32 = 14;

/// The invite row in wire order, timestamps ISO.
fn invite_json(
    id: String,
    email: String,
    invited_by: Option<String>,
    created_ms: i64,
    expires_ms: i64,
    accepted_ms: Option<i64>,
    revoked_ms: Option<i64>,
) -> serde_json::Value {
    let iso = crate::agent_auth::epoch_ms_to_iso;
    serde_json::json!({
        "id": id,
        "email": email,
        "invitedBy": invited_by,
        "createdAt": iso(created_ms),
        "expiresAt": iso(expires_ms),
        "acceptedAt": accepted_ms.map(iso),
        "revokedAt": revoked_ms.map(iso),
    })
}

/// Recent invites, newest first.
pub async fn list_invites(pg: &PgPool) -> Vec<serde_json::Value> {
    let rows: Result<
        Vec<(
            String,
            String,
            Option<String>,
            i64,
            i64,
            Option<i64>,
            Option<i64>,
        )>,
        _,
    > = sqlx::query_as(
        "select id::text, email, invited_by, \
                (trunc(extract(epoch from created_at) * 1000))::bigint, \
                (trunc(extract(epoch from expires_at) * 1000))::bigint, \
                (trunc(extract(epoch from accepted_at) * 1000))::bigint, \
                (trunc(extract(epoch from revoked_at) * 1000))::bigint \
         from invites order by created_at desc limit 200",
    )
    .fetch_all(pg)
    .await;
    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|(id, email, invited_by, c, e, a, r)| {
                invite_json(id, email, invited_by, c, e, a, r)
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 24 random bytes, base64url.
fn fresh_token() -> Result<String, String> {
    let mut buf = [0u8; 24];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// Admission door: a live invite exists for this (provider-verified) email —
/// unaccepted, unrevoked, unexpired.
pub async fn invite_allowed(pg: &PgPool, email: Option<&str>) -> Result<bool, sqlx::Error> {
    let Some(email) = email else { return Ok(false) };
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from invites \
         where email = $1 and accepted_at is null and revoked_at is null and expires_at > now()",
    )
    .bind(email.to_lowercase())
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// Stamp acceptance once the invitee's account exists.
pub async fn mark_invite_accepted(
    pg: &PgPool,
    email: &str,
    user_id: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "update invites set accepted_at = now(), accepted_user_id = $2::uuid \
         where email = $1 and accepted_at is null and revoked_at is null",
    )
    .bind(email.to_lowercase())
    .bind(user_id)
    .execute(pg)
    .await
    .map(|r| r.rows_affected())
}

async fn send_invite_email(
    pg: &PgPool,
    sb: &SecretBox,
    email: &str,
    token: &str,
    invited_by: &str,
    origin: Option<&str>,
) -> Result<(), String> {
    let org = org_profile(pg).await;
    let base = instance_base_url(pg)
        .await
        .or_else(|| origin.map(String::from));
    let org_name = {
        let n = org.name.trim();
        if n.is_empty() {
            "the team".to_string()
        } else {
            n.to_string()
        }
    };
    let link = base.as_ref().map(|b| format!("{b}/join?token={token}"));
    // Every interpolation is operator- or user-sourced and the invite email
    // regex permits quotes and angle brackets — escape all of it. The text
    // alternative needs no escaping.
    let esc = email_escape;
    let body = match &link {
        Some(link) => {
            format!(
                "<p><strong>{}</strong> invited you to {}'s Talaria workspace — where the team and its AI agents work together.</p>\
                 <p style=\"margin:24px 0\"><a href=\"{}\" style=\"background:#1a1a18;color:#ffffff;text-decoration:none;padding:10px 18px;border-radius:8px;font-size:14px\">Accept the invite</a></p>\
                 <p style=\"font-size:12px;color:#8a8a84\">Or open {} — then sign in with Google using this address ({}).</p>",
                esc(invited_by),
                esc(&org_name),
                esc(link),
                esc(link),
                esc(email),
            )
        }
        None => format!(
            "<p>Sign in with Google using this address ({}) at your team's Talaria instance to join.</p>",
            esc(email)
        ),
    };
    let html = email_shell(
        &format!("Join {}", esc(&org_name)),
        &body,
        &format!("Sent by Talaria for {}", esc(&org_name)),
    );
    let text = match &link {
        Some(link) => format!(
            "{invited_by} invited you to join {org_name} on Talaria. Accept: {link} Sign in with Google using {email}."
        ),
        None => format!(
            "{invited_by} invited you to join {org_name} on Talaria. Sign in with Google using {email}."
        ),
    };
    match send_email(
        pg,
        sb,
        &EmailInput {
            to: email.to_string(),
            subject: format!("{invited_by} invited you to join {org_name} on Talaria"),
            html,
            text: Some(text),
            headers: Vec::new(),
        },
    )
    .await
    {
        SendOutcome::Sent => Ok(()),
        SendOutcome::Failed(e) => Err(e),
    }
}

/// Create (or refresh) an invite and send the email. An
/// existing PENDING invite for the address is retired — one live invite per
/// address, re-invited with a fresh token and a fresh 14 days.
pub async fn create_invite(
    pg: &PgPool,
    sb: &SecretBox,
    email: &str,
    invited_by: &str,
    origin: Option<&str>,
) -> Result<(serde_json::Value, bool, Option<String>), String> {
    let e = email.trim().to_lowercase();
    // local@domain.tld, no spaces anywhere: one @, and a dotted domain with
    // a non-empty tail.
    let looks_like_email = !e.contains(char::is_whitespace)
        && match e.find('@') {
            Some(at) if at > 0 && e.rfind('@') == Some(at) => match e[at + 1..].find('.') {
                Some(dot) => dot > 0 && at + 1 + dot + 1 < e.len(),
                None => false,
            },
            _ => false,
        };
    if !looks_like_email {
        return Err("that does not look like an email address".into());
    }
    let token = fresh_token()?;
    let row: (
        String,
        String,
        Option<String>,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
    ) = sqlx::query_as(
        "insert into invites (email, token, invited_by, expires_at) \
         values ($1, $2, $3, now() + make_interval(days => $4)) \
         returning id::text, email, invited_by, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint, \
                   (trunc(extract(epoch from expires_at) * 1000))::bigint, \
                   (trunc(extract(epoch from accepted_at) * 1000))::bigint, \
                   (trunc(extract(epoch from revoked_at) * 1000))::bigint",
    )
    .bind(&e)
    .bind(&token)
    .bind(invited_by)
    .bind(TTL_DAYS)
    .fetch_one(pg)
    .await
    .map_err(|err| err.to_string())?;
    let (id, email_out, invited_by_out, c, x, a, r) = row;
    // One live invite per address: retire older pending ones.
    let _ = sqlx::query(
        "update invites set revoked_at = now() \
         where email = $1 and id <> $2::uuid and accepted_at is null and revoked_at is null",
    )
    .bind(&e)
    .bind(&id)
    .execute(pg)
    .await;
    let invite = invite_json(id, email_out, invited_by_out, c, x, a, r);
    let email_error = send_invite_email(pg, sb, &e, &token, invited_by, origin)
        .await
        .err();
    let email_sent = email_error.is_none();
    Ok((invite, email_sent, email_error))
}

/// Revoke: a no-op on an already-accepted invite — acceptance
/// is history, not a lever.
pub async fn revoke_invite(pg: &PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update invites set revoked_at = now() where id = $1::uuid and accepted_at is null",
    )
    .bind(id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Public join-page lookup: what does this token invite, and is it live?
pub async fn invite_by_token(
    pg: &PgPool,
    token: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "select email, invited_by from invites \
         where token = $1 and accepted_at is null and revoked_at is null and expires_at > now()",
    )
    .bind(token)
    .fetch_optional(pg)
    .await?;
    let Some((email, invited_by)) = row else {
        return Ok(None);
    };
    let org = org_profile(pg).await;
    let org_name = {
        let n = org.name.trim();
        if n.is_empty() {
            "the team".to_string()
        } else {
            n.to_string()
        }
    };
    Ok(Some(serde_json::json!({
        "email": email,
        "invitedBy": invited_by,
        "orgName": org_name,
    })))
}
