// Per-user Google connection — the status read the admin Google-client panel
// needs, plus the token vending every Google surface reads through. The OAuth
// exchange itself lives in oauth.rs.

use axum::http::header;
use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_gateway::provider::http;

/// The whole-request budget for a Google token call. The shared client bounds
/// only the connect; a Google that accepts the socket and then stalls must
/// not hold an approval, a brief tick or a disconnect open forever.
pub const GOOGLE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
use talaria_google_client::resolve_google_client;
use talaria_secretbox::SecretBox;

/// Refresh a little early so a token doesn't expire mid-request.
pub const EXPIRY_SKEW_MS: i64 = 60_000;
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// The connection's public face — wire order pinned.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub connected: bool,
    pub email: Option<String>,
    pub scope: Vec<String>,
    pub connected_at: Option<String>,
}

/// One row: connected means a refresh token is stored (truthiness — a blank
/// cipher counts as not connected). Scope is the space-joined
/// grant list, null-or-blank → empty.
/// (email, scope, refresh cipher, created-at epoch ms) — one row.
type ConnRow = (Option<String>, Option<String>, Option<String>, Option<i64>);

pub async fn get_connection_status(
    pg: &PgPool,
    user_id: &str,
) -> Result<ConnectionStatus, sqlx::Error> {
    let row: Option<ConnRow> = sqlx::query_as(
        "select email, scope, refresh_token_enc, \
                    (trunc(extract(epoch from created_at) * 1000))::bigint \
             from google_connections where user_id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let Some((email, scope, refresh_enc, created_ms)) = row else {
        return Ok(ConnectionStatus {
            connected: false,
            email: None,
            scope: vec![],
            connected_at: None,
        });
    };
    let connected = refresh_enc.as_deref().is_some_and(|s| !s.is_empty());
    if !connected {
        return Ok(ConnectionStatus {
            connected: false,
            email: None,
            scope: vec![],
            connected_at: None,
        });
    }
    let scope = scope
        .as_deref()
        .map(|s| {
            s.split(' ')
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Ok(ConnectionStatus {
        connected: true,
        email,
        scope,
        connected_at: created_ms.map(epoch_ms_to_iso),
    })
}

/// Why a token vending failed. `InvalidGrant` is its own case because it means
/// the CONNECTION IS DEAD (revoked or expired at Google's end) — the caller
/// clears the stored refresh token so the UI prompts a reconnect instead of
/// erroring forever, rather than treating it like a transient failure.
#[derive(Debug)]
pub enum TokenError {
    InvalidGrant(String),
    Other(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::InvalidGrant(m) | TokenError::Other(m) => write!(f, "{m}"),
        }
    }
}

/// Exchange a refresh token for a fresh access token. Shared by the per-user
/// and org connections. Errors carry the status and body verbatim — the only
/// consumer is the log.
pub async fn request_refresh(
    pg: &PgPool,
    sb: &SecretBox,
    refresh_token: &str,
) -> Result<(String, Option<i64>), TokenError> {
    let Some(cfg) = resolve_google_client(pg, sb).await else {
        return Err(TokenError::Other("no google client configured".into()));
    };
    // Built — and dropped — before the await, on purpose: the Serializer holds
    // a fn-pointer encoder that is not Send, and a binding still in scope at
    // the await would make every caller's future (the brief's, namely)
    // non-Send. The block is what ends its scope.
    let body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in [
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ] {
            form.append_pair(k, v);
        }
        form.finish()
    };
    let res = http()
        .post(TOKEN_ENDPOINT)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .timeout(GOOGLE_REQUEST_TIMEOUT)
        .body(body)
        .send()
        .await
        .map_err(|e| TokenError::Other(format!("google token refresh request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        let err = format!("google token refresh failed: {status} {body}");
        return Err(if body.contains("invalid_grant") {
            TokenError::InvalidGrant(err)
        } else {
            TokenError::Other(err)
        });
    }
    let tokens: serde_json::Value = res
        .json()
        .await
        .map_err(|e| TokenError::Other(format!("google token refresh body: {e}")))?;
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| TokenError::Other("google token refresh: no access_token".into()))?
        .to_string();
    Ok((
        access_token,
        tokens.get("expires_in").and_then(|v| v.as_i64()),
    ))
}

/// A valid access token for the user, refreshing if needed. `Ok(None)` ⇒ not
/// connected. The cached token is reused while it is comfortably valid; an
/// unopenable cipher falls through to a refresh rather than erroring the
/// caller.
///
/// Read-through shape: (refresh cipher, access cipher, access expiry epoch ms).
/// The expiry is read as epoch ms in SQL — parsing PG's `timestamptz::text`
/// spelling back into a clock would just be another way to be wrong.
type TokenRow = (Option<String>, Option<String>, Option<i64>);

pub async fn get_access_token(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
) -> Result<Option<String>, TokenError> {
    let row: Option<TokenRow> = sqlx::query_as(
        "select refresh_token_enc, access_token_enc, \
                (trunc(extract(epoch from access_expires_at) * 1000))::bigint \
         from google_connections where user_id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| TokenError::Other(format!("google connection read: {e}")))?;
    let Some((refresh_enc, access_enc, access_expires_ms)) = row else {
        return Ok(None);
    };
    let refresh_enc = refresh_enc.filter(|s| !s.is_empty());
    let Some(refresh_enc) = refresh_enc else {
        return Ok(None);
    };

    // Reuse the cached access token while it's comfortably valid. An expired
    // or unopenable cache simply fails the chain and falls through to refresh.
    if let (Some(enc), Some(expires_ms)) = (access_enc, access_expires_ms)
        && expires_ms - EXPIRY_SKEW_MS > now_ms
        && let Ok(token) = sb.open(&enc)
    {
        return Ok(Some(token));
    }

    let refresh_token = sb
        .open(&refresh_enc)
        .map_err(|e| TokenError::Other(format!("google refresh token unseal: {e}")))?;
    let (access_token, expires_in) = match request_refresh(pg, sb, &refresh_token).await {
        Ok(ok) => ok,
        Err(e @ TokenError::InvalidGrant(_)) => {
            // A revoked/expired refresh token means the connection is dead —
            // clear it so the UI prompts a reconnect instead of erroring forever.
            let _ = sqlx::query(
                "update google_connections set refresh_token_enc = null where user_id = $1::uuid",
            )
            .bind(user_id)
            .execute(pg)
            .await;
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    let expires_at = expires_in.map(|secs| epoch_ms_to_iso(now_ms + secs * 1000));
    let sealed = sb
        .seal(&access_token)
        .map_err(|e| TokenError::Other(format!("google access token seal: {e}")))?;
    let _ = sqlx::query(
        "update google_connections set access_token_enc = $2, access_expires_at = $3::timestamptz, updated_at = now() \
         where user_id = $1::uuid",
    )
    .bind(user_id)
    .bind(sealed)
    .bind(expires_at)
    .execute(pg)
    .await;
    Ok(Some(access_token))
}

// ── The connect flow's writers ───────────────────────────────────────────────

/// What an OAuth exchange hands the saver. A missing refresh token (Google
/// omits it when the user re-consents without prompt=consent) preserves the
/// one already stored, so a re-connect never silently drops offline access.
pub struct SaveConnection<'a> {
    pub google_sub: &'a str,
    pub email: Option<&'a str>,
    pub scope: &'a str,
    pub refresh_token: Option<&'a str>,
    pub access_token: Option<&'a str>,
    pub expires_in_seconds: Option<i64>,
    pub now_ms: i64,
}

/// Upsert a connection after an OAuth exchange.
pub async fn save_connection(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    input: &SaveConnection<'_>,
) -> Result<(), String> {
    let seal_opt = |v: Option<&str>| -> Result<Option<String>, String> {
        match v {
            Some(v) => Ok(Some(
                sb.seal(v).map_err(|e| format!("google token seal: {e}"))?,
            )),
            None => Ok(None),
        }
    };
    let refresh_enc = seal_opt(input.refresh_token)?;
    let access_enc = seal_opt(input.access_token)?;
    let expires_at = match (input.access_token, input.expires_in_seconds) {
        // Truthiness: an empty-string token or a zero/null expiry writes
        // no expiry at all.
        (Some(t), Some(secs)) if !t.is_empty() && secs != 0 => Some(
            talaria_agent_auth::epoch_ms_to_iso(input.now_ms + secs * 1000),
        ),
        _ => None,
    };
    sqlx::query(
        "insert into google_connections \
             (user_id, google_sub, email, scope, refresh_token_enc, access_token_enc, access_expires_at, updated_at) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7::timestamptz, now()) \
         on conflict (user_id) do update set \
             google_sub = excluded.google_sub, \
             email = excluded.email, \
             scope = excluded.scope, \
             refresh_token_enc = coalesce(excluded.refresh_token_enc, google_connections.refresh_token_enc), \
             access_token_enc = excluded.access_token_enc, \
             access_expires_at = excluded.access_expires_at, \
             updated_at = now()",
    )
    .bind(user_id)
    .bind(input.google_sub)
    .bind(input.email)
    .bind(input.scope)
    .bind(refresh_enc)
    .bind(access_enc)
    .bind(expires_at)
    .execute(pg)
    .await
    .map_err(|e| format!("google connection save: {e}"))?;
    Ok(())
}

/// Forget a user's connection, and revoke its grant at Google unless another
/// stored connection holds the same grant (`forget_grant`). A revoke that
/// fails (network, already revoked) still drops the row: the stored state and
/// Google's may disagree for the token's remaining lifetime, which is the
/// safer direction of the two to disagree in. The outcome says which of those
/// happened, so the route can tell the person.
pub async fn disconnect(pg: &PgPool, sb: &SecretBox, user_id: &str) -> RevokeOutcome {
    forget_grant_best_effort(pg, sb, &GrantRow::User(user_id.to_string())).await
}

// ── Revoke, and the shared-grant check before it ─────────────────────────────
//
// A revoke ends more than the token sent. Google, "Token revocation" on
// https://developers.google.com/identity/protocols/oauth2/web-server:
// "Revocation removes all OAuth 2.0 scopes previously granted to a project,
// invalidating any issued access or refresh tokens for all clients registered
// under that project." (Documented, not yet observed: no real grant has been
// revoked here.) The same account can be stored more than once: a person's
// own connection, the org account, an agent identity connected with its
// manager's account. A shared `google_sub` therefore means a shared grant,
// whichever of the project's clients minted each row, and revoking for one
// row would silently cut off the others. Such a disconnect forgets its row
// and leaves the grant alone; the last row holding it revokes it. Two
// disconnects of rows that share a grant would each see the other and
// neither revoke, so the read of the other rows and the delete run under one
// advisory lock per Google account, and the revoke goes out after that
// commit. (Narrowing "same grant" to the minting client would be wrong for
// two clients in one project, which that sentence covers. Until rows record
// their client and its project, the check errs towards not revoking, the
// direction that cannot break a connection.)

/// One stored Google connection, by store and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantRow {
    /// A person's own connection (`google_connections`), by user id.
    User(String),
    /// The shared org connection (`google_org_connection`, the singleton).
    Org,
    /// An agent's own identity (`agent_google_connections`), by agent model.
    Agent(String),
}

impl GrantRow {
    /// This row's store: the read of its account and refresh cipher, and its
    /// delete, both keyed by `$1` when the store has a key (`key`).
    fn sql(&self) -> (&'static str, &'static str) {
        match self {
            GrantRow::User(_) => (
                "select google_sub, refresh_token_enc from google_connections where user_id = $1::uuid",
                "delete from google_connections where user_id = $1::uuid",
            ),
            GrantRow::Org => (
                "select google_sub, refresh_token_enc from google_org_connection where id = 1",
                "delete from google_org_connection where id = 1",
            ),
            GrantRow::Agent(_) => (
                "select google_sub, refresh_token_enc from agent_google_connections where agent_model = $1",
                "delete from agent_google_connections where agent_model = $1",
            ),
        }
    }

    fn key(&self) -> Option<&str> {
        match self {
            GrantRow::User(k) | GrantRow::Agent(k) => Some(k),
            GrantRow::Org => None,
        }
    }
}

/// A stored row for the same Google account as the one being dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGrant {
    pub row: GrantRow,
    /// It still holds a refresh token. A row whose token was cleared (Google
    /// answered `invalid_grant`) has no live grant to lose.
    pub live: bool,
    /// The agent's display name, for an agent row; None otherwise.
    pub label: Option<String>,
}

/// What a disconnect did at Google.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// Google accepted the revoke.
    Revoked,
    /// No refresh token was stored, so there was nothing to revoke.
    NothingStored,
    /// Not sent: these other stored connections hold the same grant.
    Shared(Vec<StoredGrant>),
    /// Not done. The clause says why, in Google's own words when Google
    /// refused.
    NotRevoked(String),
}

impl RevokeOutcome {
    pub fn revoked(&self) -> bool {
        matches!(self, RevokeOutcome::Revoked)
    }
}

/// The other stored rows that would lose their grant if `dropping`'s were
/// revoked: every live row for the same Google account except `dropping`
/// itself. `same_account` must already be filtered to that account's
/// `google_sub` (`stored_grants_for` does it).
pub fn grant_shared_with(dropping: &GrantRow, same_account: Vec<StoredGrant>) -> Vec<StoredGrant> {
    same_account
        .into_iter()
        .filter(|g| g.live && g.row != *dropping)
        .collect()
}

/// Revoke `token` through `revoke` only when no other row shares the grant.
/// The revoke is a parameter so the decision is provable without Google.
pub async fn revoke_unless_shared<F, Fut>(
    token: Option<String>,
    shared_with: Vec<StoredGrant>,
    revoke: F,
) -> RevokeOutcome
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return RevokeOutcome::NothingStored;
    };
    if !shared_with.is_empty() {
        return RevokeOutcome::Shared(shared_with);
    }
    match revoke(token).await {
        Ok(()) => RevokeOutcome::Revoked,
        Err(why) => RevokeOutcome::NotRevoked(why),
    }
}

/// Every stored row, in all three stores, for one Google account.
pub async fn stored_grants_for(
    conn: &mut sqlx::PgConnection,
    google_sub: &str,
) -> Result<Vec<StoredGrant>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "select 'user', user_id::text, null::text, coalesce(refresh_token_enc, '') <> '' \
           from google_connections where google_sub = $1 \
         union all \
         select 'org', null::text, null::text, coalesce(refresh_token_enc, '') <> '' \
           from google_org_connection where google_sub = $1 \
         union all \
         select 'agent', a.agent_model, \
                (select d.display_name from agent_defs d where d.model = a.agent_model limit 1), \
                coalesce(a.refresh_token_enc, '') <> '' \
           from agent_google_connections a where a.google_sub = $1",
    )
    .bind(google_sub)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(store, key, label, live)| {
            let row = match (store.as_str(), key) {
                ("user", Some(id)) => GrantRow::User(id),
                ("org", _) => GrantRow::Org,
                ("agent", Some(model)) => GrantRow::Agent(model),
                _ => return None,
            };
            Some(StoredGrant { row, live, label })
        })
        .collect())
}

/// The one forget-and-revoke every disconnect shares. Under a per-account
/// advisory lock: read who else holds the grant, delete this row, commit.
/// Then revoke at Google only when nobody else held it — after the commit, so
/// the lock never waits on Google. A second disconnect of a row sharing the
/// grant takes the lock after this one committed, finds no other holder, and
/// revokes.
pub async fn forget_grant(
    pg: &PgPool,
    sb: &SecretBox,
    dropping: &GrantRow,
) -> Result<RevokeOutcome, sqlx::Error> {
    let (select, delete) = dropping.sql();
    let mut tx = pg.begin().await?;
    let read = sqlx::query_as::<_, (String, Option<String>)>(select);
    let read = match dropping.key() {
        Some(k) => read.bind(k),
        None => read,
    };
    let Some((sub, enc)) = read.fetch_optional(&mut *tx).await? else {
        tx.commit().await?;
        return Ok(RevokeOutcome::NothingStored);
    };
    sqlx::query("select pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("talaria.google-grant:{sub}"))
        .execute(&mut *tx)
        .await?;
    let shared_with = grant_shared_with(dropping, stored_grants_for(&mut tx, &sub).await?);
    let del = sqlx::query(delete);
    let del = match dropping.key() {
        Some(k) => del.bind(k),
        None => del,
    };
    // Zero rows: a concurrent disconnect of this same row already forgot it
    // (and revoked, when it was alone) — nothing is left to do here.
    let gone = del.execute(&mut *tx).await?.rows_affected();
    tx.commit().await?;
    if gone == 0 {
        return Ok(RevokeOutcome::NothingStored);
    }
    let Some(enc) = enc.filter(|s| !s.is_empty()) else {
        return Ok(RevokeOutcome::NothingStored);
    };
    if !shared_with.is_empty() {
        return Ok(RevokeOutcome::Shared(shared_with));
    }
    let token = match sb.open(&enc) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("[google/revoke] refresh token unseal failed: {e}");
            return Ok(RevokeOutcome::NotRevoked(
                "the stored token could not be unsealed".into(),
            ));
        }
    };
    Ok(revoke_unless_shared(Some(token), shared_with, revoke_at_google).await)
}

/// `forget_grant` for the disconnects that must always forget: a database
/// failure revokes nothing (not knowing is not permission to cut someone else
/// off) and still makes one plain attempt to drop the row.
pub async fn forget_grant_best_effort(
    pg: &PgPool,
    sb: &SecretBox,
    dropping: &GrantRow,
) -> RevokeOutcome {
    match forget_grant(pg, sb, dropping).await {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!("[google/revoke] forget {dropping:?} failed: {e}");
            let (_, delete) = dropping.sql();
            let del = sqlx::query(delete);
            let del = match dropping.key() {
                Some(k) => del.bind(k),
                None => del,
            };
            let _ = del.execute(pg).await;
            RevokeOutcome::NotRevoked(
                "Talaria could not check whether another connection holds the same grant".into(),
            )
        }
    }
}

/// POST the token to Google's revoke endpoint. `Err` is a clause that quotes
/// Google's own words (`oauth_error_text`), or names the transport failure.
pub async fn revoke_at_google(token: String) -> Result<(), String> {
    // Built — and dropped — before the await: the Serializer's encoder
    // fn-pointer is not Send, and a binding still in scope at the await
    // would make this future non-Send.
    let body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("token", &token);
        form.finish()
    };
    let res = http()
        .post("https://oauth2.googleapis.com/revoke")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .timeout(GOOGLE_REQUEST_TIMEOUT)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("the revoke request did not reach Google ({e})"))?;
    if res.status().is_success() {
        return Ok(());
    }
    let status = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    Err(oauth_error_text(status, &text))
}

/// The longest quote of Google's words a note carries: the same 300 as
/// `talaria_google_errors::REASON_MAX_CHARS`.
pub const OAUTH_REASON_MAX_CHARS: usize = 300;

/// Google's words from an OAuth endpoint's error body,
/// `{"error": "<code>", "error_description": "<sentence>"}` (RFC 6749 §5.2):
/// `code: sentence`, or whichever of the two Google sent. Whitespace is
/// folded and the whole is capped at `OAUTH_REASON_MAX_CHARS`, with an
/// ellipsis when cut; the code leads, so a cut never loses it. None when the
/// body is not that shape. This is the one parser for the OAuth shape, in the
/// format `talaria_google_errors::upstream_reason` quotes a refused refresh
/// in, so a refused revoke and a refused refresh read alike.
pub fn oauth_reason(body: &serde_json::Value) -> Option<String> {
    let text = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let joined = match (text(body.get("error")), text(body.get("error_description"))) {
        (Some(code), Some(said)) => format!("{code}: {said}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => return None,
    };
    Some(fold_and_cap(&joined))
}

/// The clause a refused OAuth call answers with: `Google answered <status>:`
/// and Google's words (`oauth_reason`), else the body itself folded and
/// capped (a non-JSON page), never reworded.
pub fn oauth_error_text(status: u16, body: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    match oauth_reason(&parsed) {
        Some(said) => format!("Google answered {status}: {said}"),
        None if !body.trim().is_empty() => {
            format!("Google answered {status}: {}", fold_and_cap(body))
        }
        None => format!("Google answered {status} with no body"),
    }
}

fn fold_and_cap(s: &str) -> String {
    let folded = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if folded.chars().count() <= OAUTH_REASON_MAX_CHARS {
        return folded;
    }
    let mut out: String = folded.chars().take(OAUTH_REASON_MAX_CHARS - 1).collect();
    out.push('…');
    out
}

/// The sentence a disconnect answers with when the grant was NOT revoked;
/// None when it was, or when nothing was stored. `viewer` is the person
/// disconnecting: their own row reads "your personal connection", anyone
/// else's never names them.
pub fn revoke_note(outcome: &RevokeOutcome, viewer_user_id: Option<&str>) -> Option<String> {
    match outcome {
        RevokeOutcome::Revoked | RevokeOutcome::NothingStored => None,
        RevokeOutcome::Shared(rows) => {
            let names: Vec<String> = rows
                .iter()
                .map(|g| match &g.row {
                    GrantRow::User(id) if Some(id.as_str()) == viewer_user_id => {
                        "your personal connection".to_string()
                    }
                    GrantRow::User(_) => "another person's personal connection".to_string(),
                    GrantRow::Org => "the org Google account".to_string(),
                    GrantRow::Agent(model) => {
                        format!("the agent {}", g.label.as_deref().unwrap_or(model))
                    }
                })
                .collect();
            Some(format!(
                "Disconnected here, but not revoked at Google: the same Google account is also connected as {}, and revoking would cut that off too. The grant is revoked when the last of them disconnects.",
                join_names(&names)
            ))
        }
        RevokeOutcome::NotRevoked(why) => {
            // Google's sentence usually brings its own full stop.
            let stop = if why.ends_with(['.', '!', '?', '…']) {
                ""
            } else {
                "."
            };
            Some(format!(
                "Disconnected here, but not revoked at Google: {why}{stop}"
            ))
        }
    }
}

/// "a", "a and b", "a, b and c".
fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} and {last}", head.join(", ")),
    }
}

/// require_token's failure: `NotConnected` means get_access_token answered
/// null — a state, not an error; `Failed` is everything the refresh path can
/// throw.
#[derive(Debug)]
pub enum RequireError {
    NotConnected,
    Failed(String),
}

impl From<TokenError> for RequireError {
    fn from(e: TokenError) -> Self {
        RequireError::Failed(e.to_string())
    }
}

/// A valid access token, or NotConnected for an unconnected user. Drive,
/// Gmail, and Calendar all read through this one door — the error KIND the
/// routes branch on comes from here, once.
pub async fn require_token(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
) -> Result<String, RequireError> {
    match get_access_token(pg, sb, user_id, now_ms).await? {
        Some(token) => Ok(token),
        None => Err(RequireError::NotConnected),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn grant(row: GrantRow) -> StoredGrant {
        StoredGrant {
            row,
            live: true,
            label: None,
        }
    }

    /// The three stores, two keys each where a store has keys.
    fn every_row() -> Vec<GrantRow> {
        vec![
            GrantRow::User("u-1".into()),
            GrantRow::User("u-2".into()),
            GrantRow::Org,
            GrantRow::Agent("doug-engineering".into()),
            GrantRow::Agent("mira-devops".into()),
        ]
    }

    #[test]
    fn every_pair_of_stores_shares_a_grant_and_no_row_shares_with_itself() {
        for dropping in every_row() {
            for other in every_row() {
                let shared = grant_shared_with(&dropping, vec![grant(other.clone())]);
                if other == dropping {
                    assert!(shared.is_empty(), "{dropping:?} counted itself");
                } else {
                    assert_eq!(
                        shared,
                        vec![grant(other.clone())],
                        "{dropping:?} must not revoke while {other:?} holds the grant"
                    );
                }
            }
        }
    }

    #[test]
    fn a_row_whose_token_was_cleared_holds_no_grant_to_lose() {
        let dead = StoredGrant {
            row: GrantRow::Org,
            live: false,
            label: None,
        };
        assert!(grant_shared_with(&GrantRow::User("u-1".into()), vec![dead]).is_empty());
    }

    /// The stub stands in for Google's revoke endpoint and counts its calls.
    async fn disconnect_with_stub(
        dropping: &GrantRow,
        same_account: Vec<StoredGrant>,
    ) -> (RevokeOutcome, usize) {
        let calls = AtomicUsize::new(0);
        let outcome = revoke_unless_shared(
            Some("refresh-token".into()),
            grant_shared_with(dropping, same_account),
            |_token| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
        )
        .await;
        (outcome, calls.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn an_agent_connected_with_its_managers_account_disconnects_without_a_revoke_call() {
        let agent = GrantRow::Agent("doug-engineering".into());
        let manager = grant(GrantRow::User("manager".into()));
        let (outcome, calls) =
            disconnect_with_stub(&agent, vec![grant(agent.clone()), manager.clone()]).await;
        assert_eq!(calls, 0, "a shared grant must never reach Google's revoke");
        assert_eq!(outcome, RevokeOutcome::Shared(vec![manager]));
        assert!(!outcome.revoked());
    }

    #[tokio::test]
    async fn every_pair_of_stores_skips_the_revoke_call_and_a_lone_row_makes_it() {
        for dropping in every_row() {
            for other in every_row().into_iter().filter(|o| *o != dropping) {
                let (outcome, calls) =
                    disconnect_with_stub(&dropping, vec![grant(dropping.clone()), grant(other)])
                        .await;
                assert_eq!(calls, 0, "{dropping:?}");
                assert!(matches!(outcome, RevokeOutcome::Shared(_)));
            }
            let (outcome, calls) =
                disconnect_with_stub(&dropping, vec![grant(dropping.clone())]).await;
            assert_eq!(calls, 1, "{dropping:?} alone revokes");
            assert_eq!(outcome, RevokeOutcome::Revoked);
        }
    }

    #[tokio::test]
    async fn no_stored_token_sends_nothing_and_a_refusal_is_carried_verbatim() {
        let calls = AtomicUsize::new(0);
        let outcome = revoke_unless_shared(None, vec![], |_t| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await;
        assert_eq!(outcome, RevokeOutcome::NothingStored);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let outcome = revoke_unless_shared(Some("t".into()), vec![], |_t| async {
            Err(oauth_error_text(400, REVOKE_GARBAGE_TOKEN))
        })
        .await;
        assert_eq!(
            revoke_note(&outcome, None).as_deref(),
            Some(
                "Disconnected here, but not revoked at Google: Google answered 400: invalid_token."
            )
        );
        // Google's sentence keeps its own full stop; the note adds none.
        let outcome = revoke_unless_shared(Some("t".into()), vec![], |_t| async {
            Err(oauth_error_text(401, NOT_FOUND_CLIENT))
        })
        .await;
        assert_eq!(
            revoke_note(&outcome, None).as_deref(),
            Some(
                "Disconnected here, but not revoked at Google: Google answered 401: invalid_client: The OAuth client was not found."
            )
        );
    }

    #[test]
    fn the_note_names_the_holders_and_never_another_persons_name() {
        let outcome = RevokeOutcome::Shared(vec![
            grant(GrantRow::User("me".into())),
            grant(GrantRow::User("someone".into())),
            grant(GrantRow::Org),
            StoredGrant {
                row: GrantRow::Agent("doug-engineering".into()),
                live: true,
                label: Some("Doug".into()),
            },
        ]);
        let note = revoke_note(&outcome, Some("me")).expect("a shared grant is said");
        assert!(note.starts_with("Disconnected here, but not revoked at Google"));
        assert!(note.contains(
            "your personal connection, another person's personal connection, the org Google account and the agent Doug"
        ));
        assert!(!note.contains("someone"));
        assert!(revoke_note(&RevokeOutcome::Revoked, None).is_none());
        assert!(revoke_note(&RevokeOutcome::NothingStored, None).is_none());
    }

    /// Two disconnects of rows that share one Google grant, racing: exactly
    /// one sees the other and keeps the grant; the other, alone by then, goes
    /// on to revoke. Here it stops at the unseal (the default box opens
    /// nothing), so Google is never called. Without the per-account lock both
    /// read the other as live and neither revokes. `#[ignore]`d — it needs a
    /// database; ci.yml's api job runs it in its live-DB steps:
    ///
    ///   DATABASE_URL=postgres://… cargo test -p talaria-google-connections --lib -- --ignored live_
    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn live_racing_disconnects_of_a_shared_grant_leave_exactly_one_to_revoke() {
        let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the live tests");
        let pg = PgPool::connect(&url).await.expect("connect");
        let sb = SecretBox::default();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        for round in 0..8 {
            let tag = format!("{nanos}-{round}");
            let sub = format!("live-shared-grant-{tag}");
            let user_id: String = sqlx::query_scalar(
                "insert into users (sub, email, role) values ($1, null, 'member') returning id::text",
            )
            .bind(format!("live-grant-user-{tag}"))
            .fetch_one(&pg)
            .await
            .expect("user");
            let model = format!("live-grant-agent-{tag}");
            sqlx::query(
                "insert into agent_defs (slug, department, model, display_name) values ($1, $1, $2, $1)",
            )
            .bind(format!("lga{tag}"))
            .bind(&model)
            .execute(&pg)
            .await
            .expect("agent def");
            sqlx::query(
                "insert into google_connections (user_id, google_sub, refresh_token_enc) values ($1::uuid, $2, 'not-a-cipher')",
            )
            .bind(&user_id)
            .bind(&sub)
            .execute(&pg)
            .await
            .expect("personal row");
            sqlx::query(
                "insert into agent_google_connections (agent_model, google_sub, refresh_token_enc) values ($1, $2, 'not-a-cipher')",
            )
            .bind(&model)
            .bind(&sub)
            .execute(&pg)
            .await
            .expect("agent row");

            let person_row = GrantRow::User(user_id.clone());
            let agent_row = GrantRow::Agent(model.clone());
            let (person, agent) = tokio::join!(
                forget_grant(&pg, &sb, &person_row),
                forget_grant(&pg, &sb, &agent_row),
            );
            let outcomes = [person.expect("person"), agent.expect("agent")];
            let kept = outcomes
                .iter()
                .filter(|o| matches!(o, RevokeOutcome::Shared(_)))
                .count();
            assert_eq!(kept, 1, "round {round}: {outcomes:?}");
            assert!(
                outcomes.iter().any(
                    |o| matches!(o, RevokeOutcome::NotRevoked(why) if why.contains("unsealed"))
                ),
                "round {round}: the one left alone must go on to revoke: {outcomes:?}"
            );
            let left: i64 = sqlx::query_scalar(
                "select (select count(*) from google_connections where google_sub = $1) \
                      + (select count(*) from agent_google_connections where google_sub = $1)",
            )
            .bind(&sub)
            .fetch_one(&pg)
            .await
            .expect("count");
            assert_eq!(left, 0, "both rows are forgotten");

            sqlx::query("delete from agent_defs where model = $1")
                .bind(&model)
                .execute(&pg)
                .await
                .expect("cleanup agent");
            sqlx::query("delete from users where id = $1::uuid")
                .bind(&user_id)
                .execute(&pg)
                .await
                .expect("cleanup user");
        }
    }

    /// The lookup the decision reads, through the real SQL, for every pair of
    /// the three stores in both orders: the first disconnect finds the other
    /// row (so each store's branch of `stored_grants_for` is read, the org
    /// one included) and keeps the grant; the second, alone by then, goes on
    /// to revoke and stops at the unseal, so Google is never called. The org
    /// store is a singleton: a row already there is saved first and put back
    /// after, even when an assertion fails. `#[ignore]`d like the race above.
    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn live_every_pair_of_stores_is_found_by_the_real_lookup() {
        let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the live tests");
        let pg = PgPool::connect(&url).await.expect("connect");
        let saved: Option<String> = sqlx::query_scalar(
            "select to_jsonb(o)::text from google_org_connection o where id = 1",
        )
        .fetch_optional(&pg)
        .await
        .expect("read the org row");
        sqlx::query("delete from google_org_connection where id = 1")
            .execute(&pg)
            .await
            .expect("set the org row aside");
        let result = tokio::spawn(every_pair_through_the_lookup(pg.clone())).await;
        sqlx::query("delete from google_org_connection where id = 1")
            .execute(&pg)
            .await
            .expect("clear the test's org row");
        if let Some(row) = saved {
            sqlx::query(
                "insert into google_org_connection \
                 select * from jsonb_populate_record(null::google_org_connection, $1::jsonb)",
            )
            .bind(row)
            .execute(&pg)
            .await
            .expect("put the org row back");
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e.into_panic());
        }
    }

    async fn every_pair_through_the_lookup(pg: PgPool) {
        #[derive(Clone, Copy, Debug)]
        enum Store {
            User,
            Org,
            Agent,
        }
        let sb = SecretBox::default();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let pairs = [
            (Store::User, Store::Org),
            (Store::Org, Store::User),
            (Store::Agent, Store::Org),
            (Store::Org, Store::Agent),
            (Store::User, Store::Agent),
            (Store::Agent, Store::User),
        ];
        for (i, (first, second)) in pairs.into_iter().enumerate() {
            let tag = format!("{nanos}-{i}");
            let sub = format!("live-pair-grant-{tag}");
            let user_id: String = sqlx::query_scalar(
                "insert into users (sub, email, role) values ($1, null, 'member') returning id::text",
            )
            .bind(format!("live-pair-user-{tag}"))
            .fetch_one(&pg)
            .await
            .expect("user");
            let model = format!("live-pair-agent-{tag}");
            let label = format!("Pair agent {i}");
            sqlx::query(
                "insert into agent_defs (slug, department, model, display_name) values ($1, $1, $2, $3)",
            )
            .bind(format!("lpa{tag}"))
            .bind(&model)
            .bind(&label)
            .execute(&pg)
            .await
            .expect("agent def");
            let row_of = |store: Store| match store {
                Store::User => GrantRow::User(user_id.clone()),
                Store::Org => GrantRow::Org,
                Store::Agent => GrantRow::Agent(model.clone()),
            };
            for store in [first, second] {
                let seed = match store {
                    Store::User => sqlx::query(
                        "insert into google_connections (user_id, google_sub, refresh_token_enc) values ($1::uuid, $2, 'not-a-cipher')",
                    )
                    .bind(&user_id),
                    Store::Org => sqlx::query(
                        "insert into google_org_connection (id, google_sub, refresh_token_enc) values (1, $1, 'not-a-cipher')",
                    ),
                    Store::Agent => sqlx::query(
                        "insert into agent_google_connections (agent_model, google_sub, refresh_token_enc) values ($1, $2, 'not-a-cipher')",
                    )
                    .bind(&model),
                };
                seed.bind(&sub)
                    .execute(&pg)
                    .await
                    .unwrap_or_else(|e| panic!("seed {store:?}: {e}"));
            }

            let kept = forget_grant(&pg, &sb, &row_of(first))
                .await
                .expect("first disconnect");
            let RevokeOutcome::Shared(holders) = &kept else {
                panic!("{first:?} then {second:?}: the first must keep the grant: {kept:?}");
            };
            let found: Vec<GrantRow> = holders.iter().map(|g| g.row.clone()).collect();
            assert_eq!(found, vec![row_of(second)], "{first:?} then {second:?}");
            if let Store::Agent = second {
                assert_eq!(holders[0].label.as_deref(), Some(label.as_str()));
            }
            let alone = forget_grant(&pg, &sb, &row_of(second))
                .await
                .expect("second disconnect");
            assert!(
                matches!(&alone, RevokeOutcome::NotRevoked(why) if why.contains("unsealed")),
                "{first:?} then {second:?}: the one left alone must go on to revoke: {alone:?}"
            );
            let left: i64 = sqlx::query_scalar(
                "select (select count(*) from google_connections where google_sub = $1) \
                      + (select count(*) from google_org_connection where google_sub = $1) \
                      + (select count(*) from agent_google_connections where google_sub = $1)",
            )
            .bind(&sub)
            .fetch_one(&pg)
            .await
            .expect("count");
            assert_eq!(
                left, 0,
                "{first:?} then {second:?}: both rows are forgotten"
            );

            sqlx::query("delete from agent_defs where model = $1")
                .bind(&model)
                .execute(&pg)
                .await
                .expect("cleanup agent");
            sqlx::query("delete from users where id = $1::uuid")
                .bind(&user_id)
                .execute(&pg)
                .await
                .expect("cleanup user");
        }
    }

    // Captured from Google on 2026-11-12, byte for byte:
    // POST https://oauth2.googleapis.com/revoke with a token that was never
    // issued → HTTP 400.
    const REVOKE_GARBAGE_TOKEN: &str = "{\n  \"error\": \"invalid_token\"\n}";
    // POST https://oauth2.googleapis.com/token, grant_type=refresh_token, with
    // a client id Google never issued → HTTP 401. (The same bytes, under the
    // same name, as the refresh classifier's fixture.)
    const NOT_FOUND_CLIENT: &str = "{\n  \"error\": \"invalid_client\",\n  \"error_description\": \"The OAuth client was not found.\"\n}";

    #[test]
    fn oauth_errors_are_quoted_in_googles_words() {
        // The shape talaria_google_errors::upstream_reason quotes a refused
        // refresh in: the code, then Google's sentence.
        assert_eq!(
            oauth_error_text(401, NOT_FOUND_CLIENT),
            "Google answered 401: invalid_client: The OAuth client was not found."
        );
        assert_eq!(
            oauth_error_text(400, REVOKE_GARBAGE_TOKEN),
            "Google answered 400: invalid_token"
        );
        // Not JSON: the body itself, folded and capped — never reworded.
        assert_eq!(
            oauth_error_text(502, " Bad \n Gateway \n"),
            "Google answered 502: Bad Gateway"
        );
        assert_eq!(
            oauth_error_text(503, "  "),
            "Google answered 503 with no body"
        );
        let long = oauth_reason(&serde_json::json!({
            "error": "invalid_request",
            "error_description": "x".repeat(400),
        }))
        .expect("an OAuth body");
        assert_eq!(long.chars().count(), OAUTH_REASON_MAX_CHARS);
        assert!(long.starts_with("invalid_request: xxx"), "the code leads");
        assert!(long.ends_with('…'));
        // A REST-shaped error is not the OAuth shape.
        assert_eq!(
            oauth_reason(&serde_json::json!({"error": {"code": 403, "message": "no"}})),
            None
        );
    }
}
