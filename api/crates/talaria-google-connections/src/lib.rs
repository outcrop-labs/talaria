// Per-user Google connection — the status read the admin Google-client panel
// needs, plus the token vending every Google surface reads through. The OAuth
// exchange itself lives in oauth.rs.
//
// Every stored grant — a person's (`google_connections`), the org account's
// (`google_org_connection`), an agent identity's (`agent_google_connections`)
// — remembers the OAuth client that minted it (`oauth_client_id`), and
// refreshes with THAT client wherever it now sits (`vend_grant`). OAuth binds
// a refresh token to the client it was issued to (RFC 6749 §6), so the slot's
// current client is the wrong question once an operator moves clients
// between slots. A grant whose client is configured nowhere is never sent to
// Google: it answers a named reconnect (`TokenError::ClientGone`). Google
// refusing the client itself flags the row with Google's words and never
// clears it (`reconnect_reason`).

use axum::http::header;
use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_gateway::provider::http;

/// The whole-request budget for a Google token call. The shared client bounds
/// only the connect; a Google that accepts the socket and then stalls must
/// not hold an approval, a brief tick or a disconnect open forever.
pub const GOOGLE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
use talaria_google_client::{GoogleClient, configured_google_clients};
use talaria_secretbox::SecretBox;

/// Refresh a little early so a token doesn't expire mid-request.
pub const EXPIRY_SKEW_MS: i64 = 60_000;
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// The connection's public face. `client_id` is the OAuth client that minted
/// the stored grant (None: not recorded yet — a grant stored before clients
/// were recorded, until its next refresh); `flagged` is Google's own words
/// from the last time it refused that client, until a refresh succeeds.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub connected: bool,
    pub email: Option<String>,
    pub scope: Vec<String>,
    pub connected_at: Option<String>,
    pub client_id: Option<String>,
    pub flagged: Option<String>,
}

/// One row: connected means a refresh token is stored (truthiness — a blank
/// cipher counts as not connected). Scope is the space-joined
/// grant list, null-or-blank → empty.
/// (email, scope, refresh cipher, created-at epoch ms, minting client, flag) — one row.
type ConnRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

pub async fn get_connection_status(
    pg: &PgPool,
    user_id: &str,
) -> Result<ConnectionStatus, sqlx::Error> {
    let row: Option<ConnRow> = sqlx::query_as(
        "select email, scope, refresh_token_enc, \
                    (trunc(extract(epoch from created_at) * 1000))::bigint, \
                    oauth_client_id, reconnect_reason \
             from google_connections where user_id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let not_connected = || ConnectionStatus {
        connected: false,
        email: None,
        scope: vec![],
        connected_at: None,
        client_id: None,
        flagged: None,
    };
    let Some((email, scope, refresh_enc, created_ms, client_id, flagged)) = row else {
        return Ok(not_connected());
    };
    let connected = refresh_enc.as_deref().is_some_and(|s| !s.is_empty());
    if !connected {
        return Ok(not_connected());
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
        client_id,
        flagged,
    })
}

/// The sentence a card shows for a stored grant that needs a person, or None
/// when it needs nothing: the grant's minting client is configured nowhere
/// (`client_gone_sentence` — no refresh will be tried), else Google's words
/// from the last refusal — addressed to an admin when the grant's own client
/// was refused, to the person first when no client is recorded yet (a trial
/// no configured client passed). `configured` is
/// `talaria_google_client::configured_google_clients`. Computed on read, so
/// adding the client back clears the first case at once.
pub fn reconnect_reason(
    client_id: Option<&str>,
    flagged: Option<&str>,
    configured: &[GoogleClient],
) -> Option<String> {
    let client_id = client_id.filter(|s| !s.is_empty());
    if let Some(id) = client_id
        && !configured.iter().any(|c| c.client_id == id)
    {
        return Some(client_gone_sentence(id));
    }
    let words = flagged.filter(|s| !s.is_empty())?;
    Some(match client_id {
        // Its own client was refused: the server's configuration.
        Some(_) => format!(
            "Google refused the OAuth client this connection refreshes with: «{words}». The connection was kept; an admin needs to check that client under Admin → Google Workspace."
        ),
        // No client recorded yet, and none accepted it (a trial): the grant
        // may simply belong to none of them, so the person's reconnect comes
        // first and the admin's check second.
        None => format!(
            "Google refused this connection with every OAuth client configured here: «{words}». The connection was kept. Reconnect it; if Google refuses again, an admin needs to check those clients under Admin → Google Workspace."
        ),
    })
}

/// The lead of the sentence a grant answers with when the client that minted
/// it is configured nowhere on this server. Our own sentence, not Google's:
/// every path that carries a token failure as text (a `RequireError::Failed`,
/// a `CalendarError::Failed`) keeps it whole, so `client_gone_in` can find it
/// and the route can still answer the 409 reconnect.
pub const CLIENT_GONE_LEAD: &str = "This Google connection was granted to OAuth client ";
const CLIENT_GONE_TAIL: &str = ", which is no longer configured on this server.";

/// The named reconnect for a grant whose minting client is gone.
pub fn client_gone_sentence(client_id: &str) -> String {
    format!(
        "{CLIENT_GONE_LEAD}{client_id}{CLIENT_GONE_TAIL} Reconnect it, or an admin can add that client back under Admin → Google Workspace."
    )
}

/// The named reconnect inside a failure message, rebuilt from the client id
/// it names — None when the message is not one.
pub fn client_gone_in(message: &str) -> Option<String> {
    let at = message.find(CLIENT_GONE_LEAD)? + CLIENT_GONE_LEAD.len();
    let rest = &message[at..];
    let id = &rest[..rest.find(CLIENT_GONE_TAIL)?];
    (!id.is_empty()).then(|| client_gone_sentence(id))
}

/// Why a token vending failed. `InvalidGrant` is its own case because it means
/// the CONNECTION IS DEAD (revoked or expired at Google's end) — the caller
/// clears the stored refresh token so the UI prompts a reconnect instead of
/// erroring forever, rather than treating it like a transient failure.
///
/// `ClientRejected` is Google refusing the OAuth CLIENT (`invalid_client`,
/// `unauthorized_client`): a wrong secret pasted, a client deleted. That is
/// the server's configuration, not this connection — nothing clears the
/// stored grant on it, or one bad paste would disconnect everyone; the vend
/// flags the row with Google's words instead (`reconnect_reason`). Each of
/// those variants carries the status and Google's body verbatim.
///
/// `ClientGone` is the grant's minting client configured nowhere on this
/// server: nothing was sent to Google, and the variant carries the named
/// reconnect (`client_gone_sentence`).
#[derive(Debug)]
pub enum TokenError {
    InvalidGrant(String),
    ClientRejected(String),
    ClientGone(String),
    Other(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::InvalidGrant(m)
            | TokenError::ClientRejected(m)
            | TokenError::ClientGone(m)
            | TokenError::Other(m) => {
                write!(f, "{m}")
            }
        }
    }
}

/// A token endpoint's refusal, by the `error` code Google put in its body
/// (RFC 6749 §5.2). A body that is not JSON falls back to the substring test
/// this classification replaced, so nothing that read as a dead grant before
/// reads as anything else now.
pub fn classify_token_refusal(body: &str, err: String) -> TokenError {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from));
    match code.as_deref() {
        Some("invalid_grant") => TokenError::InvalidGrant(err),
        Some("invalid_client" | "unauthorized_client") => TokenError::ClientRejected(err),
        Some(_) => TokenError::Other(err),
        None if body.contains("invalid_grant") => TokenError::InvalidGrant(err),
        None => TokenError::Other(err),
    }
}

/// Exchange a refresh token for a fresh access token with one client, at any
/// token endpoint — Google's (`TOKEN_ENDPOINT`) in production, a loopback
/// stub in the live test. A stored grant reaches it only through
/// `vend_grant`, which picks the client (`plan_refresh` over
/// `configured_google_clients`). Errors carry the status and body verbatim,
/// so the answer a person or agent reads can quote what Google said
/// (`talaria_google_errors::upstream_reason`).
async fn request_refresh_at(
    endpoint: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<(String, Option<i64>), TokenError> {
    // Built — and dropped — before the await, on purpose: the Serializer holds
    // a fn-pointer encoder that is not Send, and a binding still in scope at
    // the await would make every caller's future (the brief's, namely)
    // non-Send. The block is what ends its scope.
    let body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ] {
            form.append_pair(k, v);
        }
        form.finish()
    };
    let res = http()
        .post(endpoint)
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
        return Err(classify_token_refusal(&body, err));
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

// ── Which client a stored grant refreshes with ───────────────────────────────

/// The clients a grant's refresh may use, decided before anything is sent.
pub enum RefreshPlan {
    /// The grant names its client, and that client is configured.
    Stamped(GoogleClient),
    /// The grant names no client yet (stored before clients were recorded):
    /// try every configured client in order and record the first Google
    /// accepts. Asking Google is the only way to know.
    Trial(Vec<GoogleClient>),
    /// The grant names a client configured nowhere here. Nothing is sent.
    Gone(String),
}

/// The plan for a grant stamped `stamp`, given every configured client
/// (`configured_google_clients`).
pub fn plan_refresh(stamp: Option<&str>, configured: Vec<GoogleClient>) -> RefreshPlan {
    match stamp.filter(|s| !s.is_empty()) {
        Some(id) => match configured.into_iter().find(|c| c.client_id == id) {
            Some(client) => RefreshPlan::Stamped(client),
            None => RefreshPlan::Gone(id.to_string()),
        },
        None => RefreshPlan::Trial(configured),
    }
}

/// What the stored row owes a refusal.
#[derive(Debug, PartialEq, Eq)]
pub enum AfterRefusal {
    /// Leave the row as it is: nothing was learned about the grant (a
    /// transport fault, no client to ask, a client configured nowhere).
    Keep,
    /// Every client asked refused the grant itself (`invalid_grant`): the
    /// grant is dead, clear it so the card prompts a reconnect.
    ClearGrant,
    /// Google refused a client (`invalid_client`, `unauthorized_client`):
    /// keep the grant and record Google's words on the row.
    Flag(String),
}

/// A refresh, decided.
pub enum RefreshOutcome {
    Refreshed {
        /// The client Google accepted — the grant's stamp from now on.
        client_id: String,
        access_token: String,
        expires_in: Option<i64>,
    },
    Refused {
        error: TokenError,
        then: AfterRefusal,
    },
}

/// Run a plan through `refresh` (the token endpoint, or a stub that counts
/// its calls). A stamped grant asks its own client once; a trial asks each
/// configured client in turn and stops at the first Google accepts; a gone
/// client asks nobody.
pub async fn run_refresh<F, Fut>(plan: RefreshPlan, mut refresh: F) -> RefreshOutcome
where
    F: FnMut(&GoogleClient) -> Fut,
    Fut: std::future::Future<Output = Result<(String, Option<i64>), TokenError>>,
{
    let candidates = match plan {
        RefreshPlan::Gone(id) => {
            return RefreshOutcome::Refused {
                error: TokenError::ClientGone(client_gone_sentence(&id)),
                then: AfterRefusal::Keep,
            };
        }
        RefreshPlan::Stamped(client) => vec![client],
        RefreshPlan::Trial(clients) => clients,
    };
    if candidates.is_empty() {
        return RefreshOutcome::Refused {
            error: TokenError::Other("no google client configured".into()),
            then: AfterRefusal::Keep,
        };
    }
    let mut refusals: Vec<(String, TokenError)> = Vec::new();
    for client in &candidates {
        match refresh(client).await {
            Ok((access_token, expires_in)) => {
                return RefreshOutcome::Refreshed {
                    client_id: client.client_id.clone(),
                    access_token,
                    expires_in,
                };
            }
            Err(e) => refusals.push((client.client_id.clone(), e)),
        }
    }
    decide_refusal(refusals)
}

/// Every client asked refused. The grant is cleared only when every client
/// asked either refused the grant itself (`invalid_grant`) or, in a trial,
/// disowned it (`refusal_disowns`), and at least one refused it — a trial's
/// other client answering for a grant it never minted must not erase a grant
/// whose own client merely has a bad secret. A transport fault anywhere means
/// not every client was really asked: nothing is written. Otherwise a client
/// was refused: the row keeps its grant and carries Google's words — each
/// client's, by id, when more than one was asked.
fn decide_refusal(refusals: Vec<(String, TokenError)>) -> RefreshOutcome {
    let unknown = |e: &TokenError| matches!(e, TokenError::Other(_) | TokenError::ClientGone(_));
    let dead = |e: &TokenError| matches!(e, TokenError::InvalidGrant(_));
    let trial = refusals.len() > 1;
    if refusals.iter().any(|(_, e)| dead(e))
        && refusals
            .iter()
            .all(|(_, e)| dead(e) || (trial && refusal_disowns(e)))
    {
        let (_, error) = refusals
            .into_iter()
            .find(|(_, e)| dead(e))
            .expect("one client refused the grant");
        return RefreshOutcome::Refused {
            error,
            then: AfterRefusal::ClearGrant,
        };
    }
    if let Some(at) = refusals.iter().position(|(_, e)| unknown(e)) {
        let (_, error) = refusals.into_iter().nth(at).expect("found above");
        return RefreshOutcome::Refused {
            error,
            then: AfterRefusal::Keep,
        };
    }
    let words = if refusals.len() == 1 {
        refusal_words(&refusals[0].1)
    } else {
        refusals
            .iter()
            .map(|(id, e)| format!("{id}: {}", refusal_words(e)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    // Not all invalid_grant and none unknown: at least one refused a client.
    let (_, error) = refusals
        .into_iter()
        .find(|(_, e)| matches!(e, TokenError::ClientRejected(_)))
        .expect("a client refusal");
    RefreshOutcome::Refused {
        error,
        then: AfterRefusal::Flag(words),
    }
}

/// A client that authenticated but may not use this grant: Google's
/// `unauthorized_client` (RFC 6749 §5.2: "The authenticated client is not
/// authorized to use this authorization grant type."). Read by its `error`
/// code, never its sentence. Asked of one client among several in a trial,
/// that is the client saying the grant is not its own — so it neither keeps
/// a dead grant alive nor reads as the client's fault. Asked of a grant's
/// own recorded client, it stays a refused client (flagged, never cleared).
/// Google's body for a refresh token presented with a client that did not
/// mint it has NOT been captured; whether it is this or `invalid_grant`, a
/// trial over a dead grant clears it and a trial over a live one moves on.
fn refusal_disowns(e: &TokenError) -> bool {
    matches!(e, TokenError::ClientRejected(_))
        && google_error_body(&e.to_string())
            .and_then(|body| body.get("error")?.as_str().map(String::from))
            .as_deref()
            == Some("unauthorized_client")
}

/// Google's words from a refused refresh: the OAuth body's `code: sentence`
/// (`oauth_reason`), else the message folded and capped.
fn refusal_words(e: &TokenError) -> String {
    let message = e.to_string();
    google_error_body(&message)
        .and_then(|body| oauth_reason(&body))
        .unwrap_or_else(|| fold_and_cap(&message))
}

/// Google's error body inside a failure message: the first `{` from which a
/// JSON object with an `error` member parses. Every Google crate formats its
/// failure as `<what> failed: <status> <body>`, so the body is the tail.
/// `talaria_google_errors::upstream_reason` reads failures through this too.
pub fn google_error_body(message: &str) -> Option<serde_json::Value> {
    message.match_indices('{').find_map(|(at, _)| {
        let mut stream =
            serde_json::Deserializer::from_str(&message[at..]).into_iter::<serde_json::Value>();
        match stream.next() {
            Some(Ok(v)) if v.get("error").is_some() => Some(v),
            _ => None,
        }
    })
}

// ── Vending a stored grant ───────────────────────────────────────────────────

/// A token handed out, and the client that minted the grant behind it (None
/// while a cached token's grant has not yet recorded its client).
pub struct Vended {
    pub token: String,
    pub client_id: Option<String>,
}

/// One store's token statements. Every write after a refresh is a
/// compare-and-set on the refresh cipher the refresh used: a reconnect that
/// stored a new grant meanwhile is neither re-stamped, cleared nor flagged by
/// an answer about the old one.
struct GrantSql {
    label: &'static str,
    /// → (refresh cipher, access cipher, access expiry epoch ms, minting client)
    read: &'static str,
    /// $2 access cipher, $3 expiry, $4 client id, $5 refresh cipher.
    cache: &'static str,
    /// $2 refresh cipher.
    clear: &'static str,
    /// $2 Google's words, $3 refresh cipher. `reconnect_flagged_at` keeps
    /// the first refusal's time while the words stay the same.
    flag: &'static str,
}

macro_rules! grant_sql {
    ($label:literal, $table:literal, $key:literal) => {
        GrantSql {
            label: $label,
            read: concat!(
                "select refresh_token_enc, access_token_enc, ",
                "(trunc(extract(epoch from access_expires_at) * 1000))::bigint, oauth_client_id ",
                "from ", $table, " where ", $key
            ),
            cache: concat!(
                "update ", $table, " set access_token_enc = $2, access_expires_at = $3::timestamptz, ",
                "oauth_client_id = $4, reconnect_reason = null, reconnect_flagged_at = null, ",
                "updated_at = now() where ", $key, " and refresh_token_enc = $5"
            ),
            clear: concat!(
                "update ", $table, " set refresh_token_enc = null where ", $key,
                " and refresh_token_enc = $2"
            ),
            flag: concat!(
                "update ", $table, " set reconnect_flagged_at = case ",
                "when reconnect_reason is not distinct from $2 then coalesce(reconnect_flagged_at, now()) ",
                "else now() end, reconnect_reason = $2 where ", $key, " and refresh_token_enc = $3"
            ),
        }
    };
}

impl GrantRow {
    fn grant_sql(&self) -> GrantSql {
        match self {
            GrantRow::User(_) => grant_sql!("google", "google_connections", "user_id = $1::uuid"),
            GrantRow::Org => grant_sql!("google org", "google_org_connection", "id = $1::int4"),
            GrantRow::Agent(_) => grant_sql!(
                "agent google",
                "agent_google_connections",
                "agent_model = $1"
            ),
        }
    }

    /// `$1` of every `GrantSql` statement: the row's key, the org
    /// singleton's id for the org row.
    fn bind_key(&self) -> &str {
        match self {
            GrantRow::User(k) | GrantRow::Agent(k) => k,
            GrantRow::Org => "1",
        }
    }
}

/// A valid access token for one stored grant, refreshing if needed, with the
/// client that minted it. `Ok(None)` ⇒ not connected. The cached token is
/// reused while it is comfortably valid; an unopenable cipher falls through
/// to a refresh rather than erroring the caller.
///
/// The refresh follows `plan_refresh`: the grant's own client wherever it now
/// sits; a grant with no recorded client tries each configured client and
/// records the first Google accepts; a grant whose client is configured
/// nowhere answers `TokenError::ClientGone` with nothing sent. A refusal
/// does to the row what `AfterRefusal` says — cleared only on a unanimous
/// `invalid_grant`, flagged with Google's words on a refused client, else
/// untouched — and a success clears any flag.
pub async fn vend_grant(
    pg: &PgPool,
    sb: &SecretBox,
    row: &GrantRow,
    now_ms: i64,
) -> Result<Option<Vended>, TokenError> {
    vend_grant_at(pg, sb, row, now_ms, TOKEN_ENDPOINT).await
}

/// Read-through shape: (refresh cipher, access cipher, access expiry epoch
/// ms, minting client). The expiry is read as epoch ms in SQL — parsing PG's
/// `timestamptz::text` spelling back into a clock would just be another way
/// to be wrong.
type TokenRow = (Option<String>, Option<String>, Option<i64>, Option<String>);

async fn vend_grant_at(
    pg: &PgPool,
    sb: &SecretBox,
    row: &GrantRow,
    now_ms: i64,
    endpoint: &str,
) -> Result<Option<Vended>, TokenError> {
    let sql = row.grant_sql();
    let label = sql.label;
    let stored: Option<TokenRow> = sqlx::query_as(sql.read)
        .bind(row.bind_key())
        .fetch_optional(pg)
        .await
        .map_err(|e| TokenError::Other(format!("{label} connection read: {e}")))?;
    let Some((refresh_enc, access_enc, access_expires_ms, stamp)) = stored else {
        return Ok(None);
    };
    let Some(refresh_enc) = refresh_enc.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };

    // Reuse the cached access token while it's comfortably valid. An expired
    // or unopenable cache simply fails the chain and falls through to refresh.
    if let (Some(enc), Some(expires_ms)) = (access_enc, access_expires_ms)
        && expires_ms - EXPIRY_SKEW_MS > now_ms
        && let Ok(token) = sb.open(&enc)
    {
        return Ok(Some(Vended {
            token,
            client_id: stamp,
        }));
    }

    let refresh_token = sb
        .open(&refresh_enc)
        .map_err(|e| TokenError::Other(format!("{label} refresh token unseal: {e}")))?;
    let plan = plan_refresh(stamp.as_deref(), configured_google_clients(pg, sb).await);
    let refresh_token = refresh_token.as_str();
    let outcome = run_refresh(plan, |client| {
        let (id, secret) = (client.client_id.clone(), client.client_secret.clone());
        async move { request_refresh_at(endpoint, &id, &secret, refresh_token).await }
    })
    .await;
    let (client_id, access_token, expires_in) = match outcome {
        RefreshOutcome::Refreshed {
            client_id,
            access_token,
            expires_in,
        } => (client_id, access_token, expires_in),
        RefreshOutcome::Refused { error, then } => {
            let write = match &then {
                AfterRefusal::Keep => None,
                // A revoked/expired grant is dead — clear it so the UI prompts
                // a reconnect instead of erroring forever.
                AfterRefusal::ClearGrant => Some(
                    sqlx::query(sql.clear)
                        .bind(row.bind_key())
                        .bind(&refresh_enc),
                ),
                // The client's fault, not the grant's: keep it, say why.
                AfterRefusal::Flag(words) => Some(
                    sqlx::query(sql.flag)
                        .bind(row.bind_key())
                        .bind(words.as_str())
                        .bind(&refresh_enc),
                ),
            };
            if let Some(write) = write
                && let Err(e) = write.execute(pg).await
            {
                tracing::error!("[{label}/refresh] recording {then:?} failed: {e}");
            }
            return Err(error);
        }
    };
    let expires_at = expires_in.map(|secs| epoch_ms_to_iso(now_ms + secs * 1000));
    let sealed = sb
        .seal(&access_token)
        .map_err(|e| TokenError::Other(format!("{label} access token seal: {e}")))?;
    let _ = sqlx::query(sql.cache)
        .bind(row.bind_key())
        .bind(sealed)
        .bind(expires_at)
        .bind(&client_id)
        .bind(&refresh_enc)
        .execute(pg)
        .await;
    Ok(Some(Vended {
        token: access_token,
        client_id: Some(client_id),
    }))
}

/// A valid access token for the user (`vend_grant` on their own row).
/// `Ok(None)` ⇒ not connected.
pub async fn get_access_token(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
) -> Result<Option<String>, TokenError> {
    Ok(
        vend_grant(pg, sb, &GrantRow::User(user_id.to_string()), now_ms)
            .await?
            .map(|v| v.token),
    )
}

/// Forget every flag on grants minted by `client_id` — its secret was just
/// replaced, so the refusal they record may no longer stand. The next
/// refresh asks Google again and flags anew if it still refuses.
pub async fn clear_client_flags(pg: &PgPool, client_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "with u as (update google_connections set reconnect_reason = null, reconnect_flagged_at = null \
                     where oauth_client_id = $1 and reconnect_reason is not null returning 1), \
              o as (update google_org_connection set reconnect_reason = null, reconnect_flagged_at = null \
                     where oauth_client_id = $1 and reconnect_reason is not null returning 1), \
              a as (update agent_google_connections set reconnect_reason = null, reconnect_flagged_at = null \
                     where oauth_client_id = $1 and reconnect_reason is not null returning 1) \
         select ((select count(*) from u) + (select count(*) from o) + (select count(*) from a))::int8",
    )
    .bind(client_id)
    .fetch_one(pg)
    .await
}

/// Live grants per minting client, across the three stores — the admin
/// panel's census. `client_id` None is the grants not yet recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCensus {
    pub client_id: Option<String>,
    pub personal: i64,
    pub org: i64,
    pub agent: i64,
    /// Of those, the ones carrying a refusal of their client.
    pub flagged: i64,
}

/// Every live grant (a refresh token stored), counted per minting client and
/// store. Ordered by client id, the unrecorded bucket last.
pub async fn grant_census(pg: &PgPool) -> Result<Vec<ClientCensus>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, i64, i64)> = sqlx::query_as(
        "select 'personal', oauth_client_id, count(*)::int8, count(reconnect_reason)::int8 \
           from google_connections where coalesce(refresh_token_enc, '') <> '' group by 2 \
         union all \
         select 'org', oauth_client_id, count(*)::int8, count(reconnect_reason)::int8 \
           from google_org_connection where coalesce(refresh_token_enc, '') <> '' group by 2 \
         union all \
         select 'agent', oauth_client_id, count(*)::int8, count(reconnect_reason)::int8 \
           from agent_google_connections where coalesce(refresh_token_enc, '') <> '' group by 2",
    )
    .fetch_all(pg)
    .await?;
    Ok(fold_census(rows))
}

fn fold_census(rows: Vec<(String, Option<String>, i64, i64)>) -> Vec<ClientCensus> {
    let mut out: Vec<ClientCensus> = Vec::new();
    for (store, client_id, n, flagged) in rows {
        let at = match out.iter().position(|c| c.client_id == client_id) {
            Some(at) => at,
            None => {
                out.push(ClientCensus {
                    client_id,
                    ..Default::default()
                });
                out.len() - 1
            }
        };
        let entry = &mut out[at];
        match store.as_str() {
            "personal" => entry.personal += n,
            "org" => entry.org += n,
            _ => entry.agent += n,
        }
        entry.flagged += flagged;
    }
    // Recorded clients by id, then the unrecorded bucket.
    out.sort_by(|a, b| match (&a.client_id, &b.client_id) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    out
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
    /// The OAuth client the exchange ran on — the client that minted the
    /// refresh token, and the one it refreshes with from now on.
    pub oauth_client_id: &'a str,
    pub now_ms: i64,
}

/// Upsert a connection after an OAuth exchange. The row records the client
/// that minted its refresh token: a new token is the exchange's client's and
/// clears any flag on the old one; with no new token the old one stays, and
/// so does the client that minted it.
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
             (user_id, google_sub, email, scope, refresh_token_enc, access_token_enc, access_expires_at, oauth_client_id, updated_at) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7::timestamptz, $8, now()) \
         on conflict (user_id) do update set \
             google_sub = excluded.google_sub, \
             email = excluded.email, \
             scope = excluded.scope, \
             refresh_token_enc = coalesce(excluded.refresh_token_enc, google_connections.refresh_token_enc), \
             oauth_client_id = case when excluded.refresh_token_enc is null \
                 then google_connections.oauth_client_id else excluded.oauth_client_id end, \
             reconnect_reason = case when excluded.refresh_token_enc is null \
                 then google_connections.reconnect_reason end, \
             reconnect_flagged_at = case when excluded.refresh_token_enc is null \
                 then google_connections.reconnect_flagged_at end, \
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
    .bind(input.oauth_client_id)
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
// two clients in one project, which that sentence covers. Rows record their
// minting client (`oauth_client_id`) but not its project, so the check still
// errs towards not revoking, the direction that cannot break a connection.)

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
/// `talaria_google_errors::REASON_MAX_CHARS`, which asserts the two agree.
pub const OAUTH_REASON_MAX_CHARS: usize = 300;

/// Google's words from an OAuth endpoint's error body,
/// `{"error": "<code>", "error_description": "<sentence>"}` (RFC 6749 §5.2):
/// `code: sentence`, or whichever of the two Google sent. Whitespace is
/// folded and the whole is capped at `OAUTH_REASON_MAX_CHARS`, with an
/// ellipsis when cut; the code leads, so a cut never loses it. None when the
/// body is not that shape. This is the one parser for the OAuth shape:
/// `talaria_google_errors::upstream_reason` quotes a refused refresh through
/// it, so a refused revoke and a refused refresh read alike.
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

// ── What Google's Workspace MCP servers say about a grant ────────────────────
//
// The MCP gateway relays an agent's call to Google's Workspace MCP servers
// with the token of the principal's own grant. What it learns comes back
// here: the grant's stored scope (to say which scope a tool needs before
// asking Google), a cached token to drop (a 401 is retried once, after a
// refresh), and Google's refusal — always recorded as `last_mcp_refusal`
// `{status, text, tool, at}`, and, for a 401 that a fresh token did not
// cure, as the grant's `reconnect_reason` too. A 403 never flags the grant:
// it may be one file, one calendar, one quota.

/// One store's statements for the MCP relay, each keyed by `$1`.
struct McpSql {
    /// → (scope,) of a live grant.
    scope: &'static str,
    /// Drop the cached access token.
    forget: &'static str,
    /// $2 the refusal (jsonb).
    refusal: &'static str,
    /// $2 the refusal (jsonb), $3 Google's words.
    flag: &'static str,
}

macro_rules! mcp_sql {
    ($table:literal, $key:literal) => {
        McpSql {
            scope: concat!(
                "select scope from ", $table, " where ", $key,
                " and coalesce(refresh_token_enc, '') <> ''"
            ),
            forget: concat!(
                "update ", $table, " set access_token_enc = null, access_expires_at = null where ", $key
            ),
            refusal: concat!(
                "update ", $table, " set last_mcp_refusal = $2 where ", $key
            ),
            flag: concat!(
                "update ", $table, " set last_mcp_refusal = $2, reconnect_flagged_at = case ",
                "when reconnect_reason is not distinct from $3 then coalesce(reconnect_flagged_at, now()) ",
                "else now() end, reconnect_reason = $3 where ", $key,
                " and coalesce(refresh_token_enc, '') <> ''"
            ),
        }
    };
}

impl GrantRow {
    fn mcp_sql(&self) -> McpSql {
        match self {
            GrantRow::User(_) => mcp_sql!("google_connections", "user_id = $1::uuid"),
            GrantRow::Org => mcp_sql!("google_org_connection", "id = $1::int4"),
            GrantRow::Agent(_) => mcp_sql!("agent_google_connections", "agent_model = $1"),
        }
    }
}

/// The scopes a live grant was given (space-joined, as Google's token answer
/// listed them), or None: no grant, no refresh token, no scope recorded.
pub async fn grant_scope(pg: &PgPool, row: &GrantRow) -> Result<Option<String>, sqlx::Error> {
    let scope: Option<(Option<String>,)> = sqlx::query_as(row.mcp_sql().scope)
        .bind(row.bind_key())
        .fetch_optional(pg)
        .await?;
    Ok(scope.and_then(|(s,)| s).filter(|s| !s.trim().is_empty()))
}

/// Drop a grant's cached access token, so the next vend refreshes it.
pub async fn forget_access_token(pg: &PgPool, row: &GrantRow) -> Result<(), sqlx::Error> {
    sqlx::query(row.mcp_sql().forget)
        .bind(row.bind_key())
        .execute(pg)
        .await?;
    Ok(())
}

/// Google's Workspace MCP servers refused a call made with a grant's token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRefusal {
    /// The HTTP status Google answered.
    pub status: u16,
    /// Google's own words, quoted; None when it sent none.
    pub text: Option<String>,
    /// The tool called, when it was a `tools/call`.
    pub tool: Option<String>,
}

impl McpRefusal {
    /// The `last_mcp_refusal` document: `{status, text, tool, at}`.
    pub fn document(&self, now_ms: i64) -> serde_json::Value {
        serde_json::json!({
            "status": self.status,
            "text": self.text,
            "tool": self.tool,
            "at": epoch_ms_to_iso(now_ms),
        })
    }

    /// What the grant's `reconnect_reason` reads when the refusal flags it.
    pub fn flag_words(&self) -> String {
        match &self.text {
            Some(t) => t.clone(),
            None => format!(
                "Google's Workspace MCP servers refused this connection (HTTP {}).",
                self.status
            ),
        }
    }
}

/// Record Google's refusal on the grant behind the token — `flag` also sets
/// its `reconnect_reason` (a 401 a fresh token did not cure). A successful
/// refresh forgets the flag, as it forgets a refused client's.
pub async fn record_mcp_refusal(
    pg: &PgPool,
    row: &GrantRow,
    refusal: &McpRefusal,
    flag: bool,
    now_ms: i64,
) -> Result<(), sqlx::Error> {
    let sql = row.mcp_sql();
    let doc = refusal.document(now_ms);
    if flag {
        sqlx::query(sql.flag)
            .bind(row.bind_key())
            .bind(&doc)
            .bind(refusal.flag_words())
            .execute(pg)
            .await?;
    } else {
        sqlx::query(sql.refusal)
            .bind(row.bind_key())
            .bind(&doc)
            .execute(pg)
            .await?;
    }
    Ok(())
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
    // The token endpoint's answer to a client id Google never issued (HTTP
    // 401) is NOT_FOUND_CLIENT, with the refresh classifier's captures below:
    // one fixture for the revoke note and the refresh classifier.

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

    // Bodies captured from https://oauth2.googleapis.com/token on 2026-11-12
    // with no real credential: a bogus refresh token, sent with a bogus
    // client id, a real client id and a bogus secret, and gcloud's published
    // installed-app client.
    const NOT_FOUND_CLIENT: &str = "{\n  \"error\": \"invalid_client\",\n  \"error_description\": \"The OAuth client was not found.\"\n}";
    const BAD_SECRET: &str = "{\n  \"error\": \"invalid_client\",\n  \"error_description\": \"The provided client secret is invalid.\"\n}";
    const BAD_REFRESH: &str =
        "{\n  \"error\": \"invalid_grant\",\n  \"error_description\": \"Bad Request\"\n}";

    fn class(body: &str) -> &'static str {
        match classify_token_refusal(body, format!("google token refresh failed: 400 {body}")) {
            TokenError::InvalidGrant(_) => "invalid_grant",
            TokenError::ClientRejected(_) => "client_rejected",
            TokenError::ClientGone(_) => "client_gone",
            TokenError::Other(_) => "other",
        }
    }

    #[test]
    fn a_refused_client_is_not_a_dead_grant() {
        assert_eq!(class(NOT_FOUND_CLIENT), "client_rejected");
        assert_eq!(class(BAD_SECRET), "client_rejected");
        assert_eq!(class(BAD_REFRESH), "invalid_grant");
    }

    #[test]
    fn the_message_keeps_googles_body_verbatim() {
        let e = classify_token_refusal(
            BAD_SECRET,
            format!("google token refresh failed: 401 Unauthorized {BAD_SECRET}"),
        );
        assert!(
            e.to_string()
                .contains("The provided client secret is invalid.")
        );
    }

    #[test]
    fn a_body_that_is_not_json_keeps_the_old_substring_rule() {
        assert_eq!(class("400 invalid_grant"), "invalid_grant");
        assert_eq!(class("<html>502 Bad Gateway</html>"), "other");
        // A code this classifier does not name is not guessed at.
        assert_eq!(class("{\"error\":\"invalid_request\"}"), "other");
    }
    // ── Which client a stored grant refreshes with ──────────────────────────

    use talaria_google_client::ClientRole;

    fn client(id: &str, role: ClientRole, source: &'static str) -> GoogleClient {
        GoogleClient {
            client_id: id.into(),
            client_secret: format!("secret-of-{id}"),
            hd: None,
            source,
            role,
        }
    }

    /// Connections record A, the env pair it shadows, sign-in record B —
    /// `configured_google_clients`' order.
    fn configured() -> Vec<GoogleClient> {
        vec![
            client("conn-a", ClientRole::Connections, "admin"),
            client("conn-env", ClientRole::Connections, "env"),
            client("signin-b", ClientRole::SignIn, "admin"),
        ]
    }

    /// Google's token endpoint answering with one of the captured bodies.
    fn refused(body: &str) -> TokenError {
        classify_token_refusal(
            body,
            format!("google token refresh failed: 400 Bad Request {body}"),
        )
    }

    type Answer = fn() -> Result<(String, Option<i64>), TokenError>;

    /// Run a plan against a stub token endpoint that answers per client id
    /// and records who it was asked for, in order.
    async fn run_stub(
        plan: RefreshPlan,
        answers: &[(&str, Answer)],
    ) -> (RefreshOutcome, Vec<String>) {
        let asked = std::sync::Mutex::new(Vec::<String>::new());
        let outcome = run_refresh(plan, |c| {
            asked.lock().unwrap().push(c.client_id.clone());
            let answer = answers
                .iter()
                .find(|(id, _)| *id == c.client_id)
                .map(|(_, a)| a())
                .unwrap_or_else(|| Err(TokenError::Other("stub: unexpected client".into())));
            async move { answer }
        })
        .await;
        (outcome, asked.into_inner().unwrap())
    }

    fn ok() -> Result<(String, Option<i64>), TokenError> {
        Ok(("ya29.stub".into(), Some(3599)))
    }
    fn dead_grant() -> Result<(String, Option<i64>), TokenError> {
        Err(refused(BAD_REFRESH))
    }
    fn bad_secret() -> Result<(String, Option<i64>), TokenError> {
        Err(refused(BAD_SECRET))
    }
    fn unknown_client() -> Result<(String, Option<i64>), TokenError> {
        Err(refused(NOT_FOUND_CLIENT))
    }
    /// NOT a capture: RFC 6749 §5.2's `unauthorized_client` code alone, no
    /// sentence, so no wording is put in Google's mouth. It stands in for
    /// what Google may answer a refresh token presented with a client that
    /// did not mint it — that answer has not been captured, and every rule
    /// below must hold whether it is this or `invalid_grant`.
    const RFC_UNAUTHORIZED_CLIENT: &str = "{\"error\": \"unauthorized_client\"}";
    fn disowned() -> Result<(String, Option<i64>), TokenError> {
        Err(refused(RFC_UNAUTHORIZED_CLIENT))
    }
    fn unreachable_google() -> Result<(String, Option<i64>), TokenError> {
        Err(TokenError::Other(
            "google token refresh request: error sending request".into(),
        ))
    }

    fn refreshed_with(outcome: &RefreshOutcome) -> Option<&str> {
        match outcome {
            RefreshOutcome::Refreshed { client_id, .. } => Some(client_id),
            RefreshOutcome::Refused { .. } => None,
        }
    }

    #[tokio::test]
    async fn a_stamp_refreshes_with_that_client_in_whichever_slot_it_sits() {
        for id in ["conn-a", "conn-env", "signin-b"] {
            let plan = plan_refresh(Some(id), configured());
            assert!(matches!(&plan, RefreshPlan::Stamped(c) if c.client_id == id));
            let (outcome, asked) =
                run_stub(plan, &[("conn-a", ok), ("conn-env", ok), ("signin-b", ok)]).await;
            assert_eq!(asked, [id], "only the stamped client is asked");
            assert_eq!(refreshed_with(&outcome), Some(id));
        }
        // The secret sent is the stamped client's own.
        let RefreshPlan::Stamped(c) = plan_refresh(Some("signin-b"), configured()) else {
            panic!("stamped");
        };
        assert_eq!(c.client_secret, "secret-of-signin-b");
    }

    #[tokio::test]
    async fn an_unknown_stamp_makes_zero_token_endpoint_calls() {
        let plan = plan_refresh(Some("rotated-away"), configured());
        assert!(matches!(&plan, RefreshPlan::Gone(id) if id == "rotated-away"));
        let (outcome, asked) = run_stub(plan, &[("rotated-away", ok)]).await;
        assert!(asked.is_empty(), "nothing is sent to Google: {asked:?}");
        let RefreshOutcome::Refused { error, then } = outcome else {
            panic!("refused");
        };
        assert_eq!(then, AfterRefusal::Keep, "the grant is kept as it is");
        let TokenError::ClientGone(sentence) = error else {
            panic!("ClientGone");
        };
        assert_eq!(sentence, client_gone_sentence("rotated-away"));
        // ...and so with no client configured at all.
        let (_, asked) = run_stub(plan_refresh(Some("conn-a"), vec![]), &[("conn-a", ok)]).await;
        assert!(asked.is_empty());
    }

    #[tokio::test]
    async fn an_unrecorded_grant_tries_each_client_in_order_and_records_the_first_google_accepts() {
        // Google's answer to a refresh token presented with a client that did
        // not mint it has not been captured; the trial moves on whatever it is
        // — here each captured refusal, and the RFC's `unauthorized_client`,
        // plays that part.
        for wrong_client in [dead_grant as Answer, disowned, unknown_client, bad_secret] {
            let plan = plan_refresh(None, configured());
            assert!(matches!(&plan, RefreshPlan::Trial(list) if list.len() == 3));
            let (outcome, asked) = run_stub(
                plan,
                &[("conn-a", wrong_client), ("conn-env", ok), ("signin-b", ok)],
            )
            .await;
            assert_eq!(asked, ["conn-a", "conn-env"], "stops at the first accepted");
            assert_eq!(refreshed_with(&outcome), Some("conn-env"));
        }
        // An empty stamp is no stamp.
        assert!(matches!(
            plan_refresh(Some(""), configured()),
            RefreshPlan::Trial(_)
        ));
    }

    fn refusal(outcome: RefreshOutcome) -> (TokenError, AfterRefusal) {
        match outcome {
            RefreshOutcome::Refused { error, then } => (error, then),
            RefreshOutcome::Refreshed { client_id, .. } => panic!("refreshed with {client_id}"),
        }
    }

    #[tokio::test]
    async fn a_refused_client_flags_the_grant_and_never_clears_it() {
        // Stamped: Google's own words, verbatim, become the flag.
        let (error, then) = refusal(
            run_stub(
                plan_refresh(Some("conn-a"), configured()),
                &[("conn-a", bad_secret)],
            )
            .await
            .0,
        );
        assert!(matches!(error, TokenError::ClientRejected(_)));
        assert_eq!(
            then,
            AfterRefusal::Flag("invalid_client: The provided client secret is invalid.".into())
        );
        // A trial where one client's secret is wrong and the others answer
        // invalid_grant for a grant they never minted: not unanimous, so the
        // grant is kept — each answer named by its client.
        let (error, then) = refusal(
            run_stub(
                plan_refresh(None, configured()),
                &[
                    ("conn-a", dead_grant),
                    ("conn-env", bad_secret),
                    ("signin-b", unknown_client),
                ],
            )
            .await
            .0,
        );
        assert!(matches!(error, TokenError::ClientRejected(_)));
        assert_eq!(
            then,
            AfterRefusal::Flag(
                "conn-a: invalid_grant: Bad Request; conn-env: invalid_client: The provided client secret is invalid.; signin-b: invalid_client: The OAuth client was not found.".into()
            )
        );
    }

    #[tokio::test]
    async fn a_dead_grant_is_cleared_only_when_every_client_asked_says_so() {
        let (error, then) = refusal(
            run_stub(
                plan_refresh(Some("conn-a"), configured()),
                &[("conn-a", dead_grant)],
            )
            .await
            .0,
        );
        assert!(matches!(error, TokenError::InvalidGrant(_)));
        assert_eq!(then, AfterRefusal::ClearGrant);
        let (error, then) = refusal(
            run_stub(
                plan_refresh(None, configured()),
                &[
                    ("conn-a", dead_grant),
                    ("conn-env", dead_grant),
                    ("signin-b", dead_grant),
                ],
            )
            .await
            .0,
        );
        assert!(matches!(error, TokenError::InvalidGrant(_)));
        assert_eq!(then, AfterRefusal::ClearGrant);
    }

    #[tokio::test]
    async fn a_transport_fault_writes_nothing() {
        let answers: &[(&str, Answer)] = &[
            ("conn-a", dead_grant),
            ("conn-env", unreachable_google),
            ("signin-b", bad_secret),
        ];
        // Stamped conn-a answered invalid_grant itself: cleared.
        let (error, then) = refusal(
            run_stub(plan_refresh(Some("conn-a"), configured()), answers)
                .await
                .0,
        );
        assert_eq!(then, AfterRefusal::ClearGrant);
        assert!(matches!(error, TokenError::InvalidGrant(_)));
        // The trial could not ask conn-env: it concludes nothing.
        let (error, then) = refusal(run_stub(plan_refresh(None, configured()), answers).await.0);
        assert_eq!(then, AfterRefusal::Keep);
        assert!(matches!(error, TokenError::Other(_)));
        // The case the rule exists for, with no client refused to hide it: one
        // client says invalid_grant while the grant's own client may be the
        // one Google could not be reached for. Not cleared — nor when the
        // client reached disowns it.
        let two = || {
            vec![
                client("conn-a", ClientRole::Connections, "admin"),
                client("conn-env", ClientRole::Connections, "env"),
            ]
        };
        for reached in [dead_grant as Answer, disowned] {
            let (error, then) = refusal(
                run_stub(
                    plan_refresh(None, two()),
                    &[("conn-a", reached), ("conn-env", unreachable_google)],
                )
                .await
                .0,
            );
            assert_eq!(then, AfterRefusal::Keep);
            assert!(matches!(error, TokenError::Other(_)));
        }
        // No client configured at all: today's sentence, nothing written.
        let (error, then) = refusal(run_stub(plan_refresh(None, vec![]), &[]).await.0);
        assert_eq!(then, AfterRefusal::Keep);
        assert_eq!(error.to_string(), "no google client configured");
    }

    #[tokio::test]
    async fn a_trial_reads_the_same_whichever_answer_google_gives_a_client_that_did_not_mint_the_grant()
     {
        for cross in [dead_grant as Answer, disowned] {
            // The grant is dead at its own client (conn-env); the others
            // answer for a grant they never minted. Cleared, with the dead
            // grant's own refusal.
            let (error, then) = refusal(
                run_stub(
                    plan_refresh(None, configured()),
                    &[
                        ("conn-a", cross),
                        ("conn-env", dead_grant),
                        ("signin-b", cross),
                    ],
                )
                .await
                .0,
            );
            assert_eq!(then, AfterRefusal::ClearGrant);
            assert!(matches!(error, TokenError::InvalidGrant(_)));
            // Its own client's secret is wrong: kept and flagged, never
            // cleared — an admin's fix.
            let (error, then) = refusal(
                run_stub(
                    plan_refresh(None, configured()),
                    &[
                        ("conn-a", cross),
                        ("conn-env", bad_secret),
                        ("signin-b", cross),
                    ],
                )
                .await
                .0,
            );
            assert!(matches!(error, TokenError::ClientRejected(_)));
            assert!(matches!(then, AfterRefusal::Flag(_)), "{then:?}");
        }
        // Every client disowns it: no client here minted it. Nothing says it
        // is dead, so it is kept, flagged with each client's words, and the
        // card asks the person to reconnect first.
        let (error, then) = refusal(
            run_stub(
                plan_refresh(None, configured()),
                &[
                    ("conn-a", disowned),
                    ("conn-env", disowned),
                    ("signin-b", disowned),
                ],
            )
            .await
            .0,
        );
        assert!(matches!(error, TokenError::ClientRejected(_)));
        let AfterRefusal::Flag(words) = then else {
            panic!("flagged: {then:?}");
        };
        assert_eq!(
            words,
            "conn-a: unauthorized_client; conn-env: unauthorized_client; signin-b: unauthorized_client"
        );
        assert_eq!(
            reconnect_reason(None, Some(&words), &configured()),
            Some(format!(
                "Google refused this connection with every OAuth client configured here: «{words}». The connection was kept. Reconnect it; if Google refuses again, an admin needs to check those clients under Admin → Google Workspace."
            ))
        );
        // The grant's own recorded client answering it: that client is
        // refused — flagged in Google's words, never cleared, never disowned.
        let (error, then) = refusal(
            run_stub(
                plan_refresh(Some("conn-a"), configured()),
                &[("conn-a", disowned)],
            )
            .await
            .0,
        );
        assert!(matches!(error, TokenError::ClientRejected(_)));
        assert_eq!(then, AfterRefusal::Flag("unauthorized_client".into()));
        // Disowning is read from the code, never from a sentence.
        assert!(refusal_disowns(&refused(RFC_UNAUTHORIZED_CLIENT)));
        assert!(!refusal_disowns(&refused(BAD_SECRET)));
        assert!(!refusal_disowns(&refused(NOT_FOUND_CLIENT)));
        assert!(!refusal_disowns(&refused(BAD_REFRESH)));
    }

    #[test]
    fn the_named_reconnect_survives_being_carried_as_text() {
        let sentence = client_gone_sentence("1234-abc.apps.googleusercontent.com");
        assert_eq!(client_gone_in(&sentence), Some(sentence.clone()));
        assert_eq!(
            client_gone_in(&format!("calendar: {sentence}")),
            Some(sentence.clone())
        );
        assert_eq!(
            client_gone_in(&format!("{:?}", RequireError::Failed(sentence.clone()))),
            Some(sentence.clone())
        );
        assert_eq!(client_gone_in("google token refresh failed: 401"), None);
        assert_eq!(client_gone_in(CLIENT_GONE_LEAD), None);
    }

    #[test]
    fn a_card_reads_why_a_connection_needs_a_person() {
        let configured = configured();
        // Its client is configured: nothing to say, or Google's words.
        assert_eq!(reconnect_reason(Some("signin-b"), None, &configured), None);
        assert_eq!(reconnect_reason(None, None, &configured), None);
        assert_eq!(
            reconnect_reason(
                Some("conn-a"),
                Some("invalid_client: The provided client secret is invalid."),
                &configured
            )
            .as_deref(),
            Some(
                "Google refused the OAuth client this connection refreshes with: «invalid_client: The provided client secret is invalid.». The connection was kept; an admin needs to check that client under Admin → Google Workspace."
            )
        );
        // Its client configured nowhere: the named reconnect wins.
        assert_eq!(
            reconnect_reason(Some("rotated-away"), Some("stale words"), &configured),
            Some(client_gone_sentence("rotated-away"))
        );
    }

    #[test]
    fn the_census_counts_live_grants_per_client() {
        let rows = vec![
            ("personal".to_string(), Some("conn-b".to_string()), 3, 1),
            ("org".to_string(), Some("conn-b".to_string()), 1, 0),
            ("personal".to_string(), None, 2, 0),
            ("agent".to_string(), Some("conn-a".to_string()), 2, 2),
            ("agent".to_string(), None, 1, 0),
        ];
        assert_eq!(
            fold_census(rows),
            vec![
                ClientCensus {
                    client_id: Some("conn-a".into()),
                    personal: 0,
                    org: 0,
                    agent: 2,
                    flagged: 2
                },
                ClientCensus {
                    client_id: Some("conn-b".into()),
                    personal: 3,
                    org: 1,
                    agent: 0,
                    flagged: 1
                },
                ClientCensus {
                    client_id: None,
                    personal: 2,
                    org: 0,
                    agent: 1,
                    flagged: 0
                },
            ]
        );
    }
}

/// LIVE: a stored grant refreshes with the client that minted it, through the
/// real SQL, against a loopback stand-in for Google's token endpoint that
/// records which `client_id` each refresh carried. `#[ignore]`d; ci.yml's api
/// job runs it with the ignored lib tests in its live-DB steps:
///
///   DATABASE_URL=postgres://… cargo test -p talaria-google-connections --lib -- --ignored stamp_live
///
/// It builds its own schema — the token tables copied from the migrated
/// `public` ones (`like … including all`, so the migration's columns are the
/// ones under test) plus `app_settings` — and points the pool's search_path
/// at it, so a dev database's Google rows are never read or written.
#[cfg(test)]
mod stamp_live {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::*;

    // Captured from https://oauth2.googleapis.com/token on 2026-11-12 (the
    // same bodies as the unit tests above).
    const BAD_SECRET: &str = "{\n  \"error\": \"invalid_client\",\n  \"error_description\": \"The provided client secret is invalid.\"\n}";
    const BAD_REFRESH: &str =
        "{\n  \"error\": \"invalid_grant\",\n  \"error_description\": \"Bad Request\"\n}";
    /// NOT a capture: RFC 6749 §5.2's code alone, standing in for Google's
    /// uncaptured answer to a token presented with a client that did not
    /// mint it (the unit tests hold the rules for `invalid_grant` there too).
    const RFC_UNAUTHORIZED_CLIENT: &str = "{\"error\": \"unauthorized_client\"}";

    const GOOGLE_ENV: [&str; 5] = [
        "AUTH_GOOGLE_CLIENT_ID",
        "AUTH_GOOGLE_CLIENT_SECRET",
        "AUTH_GOOGLE_SIGNIN_CLIENT_ID",
        "AUTH_GOOGLE_SIGNIN_CLIENT_SECRET",
        "AUTH_GOOGLE_HD",
    ];

    fn test_box() -> SecretBox {
        let kek = talaria_secretbox::derive_kek("google-stamp-live-test");
        let dek = talaria_secretbox::new_dek().expect("entropy");
        SecretBox::from_parts(kek, HashMap::from([(1u32, dek)]), Some(1))
    }

    /// The token endpoint stand-in. A refresh token `rt-for-<client>` was
    /// minted by `<client>`: that client with its own secret (`secret-<client>`)
    /// gets a token, with another secret Google's captured `invalid_client`.
    /// `rt-dead-for-<client>` was minted by `<client>` and revoked: that
    /// client gets Google's captured `invalid_grant`. `rt-dead` is the
    /// captured `invalid_grant` for everyone. A token presented with a client
    /// that did not mint it gets `RFC_UNAUTHORIZED_CLIENT`: what Google really
    /// answers there has NOT been captured. Answers (url, the client ids
    /// asked, in order).
    fn token_stub() -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let url = format!(
            "http://127.0.0.1:{}/token",
            listener.local_addr().unwrap().port()
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let body = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break Some(text[end + 4..end + 4 + len].to_string());
                        }
                    }
                };
                let Some(body) = body else { continue };
                let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
                let field = |k: &str| form.get(k).cloned().unwrap_or_default();
                let (client, secret, rt) = (
                    field("client_id"),
                    field("client_secret"),
                    field("refresh_token"),
                );
                log.lock().unwrap().push(client.clone());
                let (live, dead) = (rt.strip_prefix("rt-for-"), rt.strip_prefix("rt-dead-for-"));
                let (status, answer) = match (live, dead) {
                    (Some(owner), _) if owner == client && secret == format!("secret-{client}") => (
                        "200 OK",
                        json!({ "access_token": format!("ya29.stub-{client}"), "expires_in": 3599, "token_type": "Bearer" })
                            .to_string(),
                    ),
                    (Some(owner), _) if owner == client => {
                        ("401 Unauthorized", BAD_SECRET.to_string())
                    }
                    (_, Some(owner)) if owner == client => ("400 Bad Request", BAD_REFRESH.to_string()),
                    // Another client's token, live or dead.
                    (Some(_), _) | (_, Some(_)) => {
                        ("400 Bad Request", RFC_UNAUTHORIZED_CLIENT.to_string())
                    }
                    _ => ("400 Bad Request", BAD_REFRESH.to_string()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        (url, seen)
    }

    fn take(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        std::mem::take(&mut *seen.lock().unwrap())
    }

    async fn scratch() -> (PgPool, PgPool, String) {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        let admin = PgPool::connect(&url).await.expect("connect");
        let schema = format!("google_stamp_live_{}", std::process::id());
        let mut ddl = vec![
            format!("drop schema if exists {schema} cascade"),
            format!("create schema {schema}"),
            format!(
                "create table {schema}.app_settings (key text primary key, value jsonb not null, \
                 updated_at timestamptz not null default now())"
            ),
        ];
        for table in [
            "google_connections",
            "google_org_connection",
            "agent_google_connections",
        ] {
            ddl.push(format!(
                "create table {schema}.{table} (like public.{table} including all)"
            ));
        }
        for sql in ddl {
            // AssertSqlSafe: the schema name is this test's own tag and a pid,
            // the table names are the three literals above.
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                .execute(&admin)
                .await
                .expect(&sql);
        }
        let opts = PgConnectOptions::from_str(&url)
            .expect("DATABASE_URL parses")
            .options([("search_path", schema.as_str())]);
        let pg = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .expect("scratch pool");
        (pg, admin, schema)
    }

    async fn set_client(pg: &PgPool, sb: &SecretBox, key: &str, id: &str, secret: &str) {
        sqlx::query(
            "insert into app_settings (key, value) values ($1, $2) \
             on conflict (key) do update set value = excluded.value",
        )
        .bind(key)
        .bind(json!({ "clientId": id, "clientSecretEnc": sb.seal(secret).unwrap(), "hd": null }))
        .execute(pg)
        .await
        .expect("client record");
    }

    /// A personal grant whose access cache has expired, so a vend refreshes.
    async fn seed_user(
        pg: &PgPool,
        sb: &SecretBox,
        n: u8,
        rt: &str,
        stamp: Option<&str>,
    ) -> String {
        let user = format!("00000000-0000-4000-8000-0000000000{n:02x}");
        sqlx::query(
            "insert into google_connections (user_id, google_sub, email, scope, refresh_token_enc, \
             access_token_enc, access_expires_at, oauth_client_id) \
             values ($1::uuid, $2, $3, '', $4, $5, now() - interval '1 hour', $6)",
        )
        .bind(&user)
        .bind(format!("sub-{n}"))
        .bind(format!("person{n}@radhires.com"))
        .bind(sb.seal(rt).unwrap())
        .bind(sb.seal("expired-access").unwrap())
        .bind(stamp)
        .execute(pg)
        .await
        .expect("personal grant");
        user
    }

    /// (minting client, Google's words, refresh token still stored)
    async fn user_row(pg: &PgPool, user: &str) -> (Option<String>, Option<String>, bool) {
        sqlx::query_as(
            "select oauth_client_id, reconnect_reason, coalesce(refresh_token_enc, '') <> '' \
             from google_connections where user_id = $1::uuid",
        )
        .bind(user)
        .fetch_one(pg)
        .await
        .expect("personal row")
    }

    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn stamp_live_every_grant_refreshes_with_the_client_that_minted_it() {
        for var in GOOGLE_ENV {
            assert!(
                std::env::var(var).is_err(),
                "unset {var}: this test pins the Admin-record clients"
            );
        }
        let (pg, admin, schema) = scratch().await;
        let sb = test_box();
        let (endpoint, seen) = token_stub();
        let now = talaria_agent_auth::now_ms();
        let vend = |row: GrantRow| {
            let (pg, sb, endpoint) = (pg.clone(), &sb, endpoint.clone());
            async move { vend_grant_at(&pg, sb, &row, now, &endpoint).await }
        };
        // Orientation N: the old client A moved to the sign-in slot, a new
        // client C saved as connections.
        set_client(&pg, &sb, "google_oauth_client", "conn-c", "secret-conn-c").await;
        set_client(
            &pg,
            &sb,
            "google_signin_oauth_client",
            "conn-a",
            "secret-conn-a",
        )
        .await;

        // A-stamped grant: refreshed with A, now in the sign-in slot. The
        // stub received A's client id and nothing else.
        let u1 = seed_user(&pg, &sb, 1, "rt-for-conn-a", Some("conn-a")).await;
        let got = vend(GrantRow::User(u1.clone()))
            .await
            .expect("refreshed")
            .expect("connected");
        assert_eq!(got.token, "ya29.stub-conn-a");
        assert_eq!(got.client_id.as_deref(), Some("conn-a"));
        assert_eq!(take(&seen), ["conn-a"]);
        assert_eq!(
            user_row(&pg, &u1).await,
            (Some("conn-a".into()), None, true)
        );
        // The cache now answers, with no refresh.
        let cached = vend(GrantRow::User(u1.clone())).await.unwrap().unwrap();
        assert_eq!(
            (cached.token.as_str(), cached.client_id.as_deref()),
            ("ya29.stub-conn-a", Some("conn-a"))
        );
        assert!(take(&seen).is_empty());

        // A grant that recorded no client: C is asked first, then A, which
        // accepts — and is recorded.
        let u2 = seed_user(&pg, &sb, 2, "rt-for-conn-a", None).await;
        let got = vend(GrantRow::User(u2.clone())).await.unwrap().unwrap();
        assert_eq!(got.client_id.as_deref(), Some("conn-a"));
        assert_eq!(take(&seen), ["conn-c", "conn-a"]);
        assert_eq!(
            user_row(&pg, &u2).await,
            (Some("conn-a".into()), None, true)
        );

        // A grant stamped with a client configured nowhere: zero calls, the
        // named reconnect, and the row untouched.
        let u3 = seed_user(&pg, &sb, 3, "rt-for-conn-a", Some("rotated-away")).await;
        match vend(GrantRow::User(u3.clone())).await {
            Err(TokenError::ClientGone(s)) => assert_eq!(s, client_gone_sentence("rotated-away")),
            other => panic!(
                "expected the named reconnect, got {:?}",
                other.map(|v| v.map(|v| v.token))
            ),
        }
        assert!(take(&seen).is_empty(), "nothing reached the token endpoint");
        assert_eq!(
            user_row(&pg, &u3).await,
            (Some("rotated-away".into()), None, true)
        );

        // A dead grant with no client recorded: cleared only after every
        // configured client was asked.
        let u4 = seed_user(&pg, &sb, 4, "rt-dead", None).await;
        assert!(matches!(
            vend(GrantRow::User(u4.clone())).await,
            Err(TokenError::InvalidGrant(_))
        ));
        assert_eq!(take(&seen), ["conn-c", "conn-a"]);
        assert_eq!(user_row(&pg, &u4).await, (None, None, false));

        // A dead grant minted by A, with no client recorded: C disowns it
        // (unauthorized_client), A refuses it (invalid_grant) — cleared, not
        // left flagged forever as an admin's problem.
        let u5 = seed_user(&pg, &sb, 5, "rt-dead-for-conn-a", None).await;
        assert!(matches!(
            vend(GrantRow::User(u5.clone())).await,
            Err(TokenError::InvalidGrant(_))
        ));
        assert_eq!(take(&seen), ["conn-c", "conn-a"]);
        assert_eq!(user_row(&pg, &u5).await, (None, None, false));

        // The org account, minted by C, whose secret is then pasted wrong:
        // Google's invalid_client flags the row in Google's words and keeps
        // the grant — the login domain gate, which reads this row's refresh
        // token, stays shut.
        sqlx::query(
            "insert into google_org_connection (id, google_sub, email, scope, refresh_token_enc, \
             access_token_enc, access_expires_at, oauth_client_id) \
             values (1, 'org-sub', 'org@radhires.com', '', $1, $2, now() - interval '1 hour', 'conn-c')",
        )
        .bind(sb.seal("rt-for-conn-c").unwrap())
        .bind(sb.seal("expired-access").unwrap())
        .execute(&pg)
        .await
        .expect("org grant");
        set_client(&pg, &sb, "google_oauth_client", "conn-c", "pasted-wrong").await;
        for round in 0..2 {
            assert!(
                matches!(
                    vend(GrantRow::Org).await,
                    Err(TokenError::ClientRejected(_))
                ),
                "round {round}"
            );
            assert_eq!(take(&seen), ["conn-c"]);
        }
        let (words, flagged_at, kept): (Option<String>, Option<i64>, bool) = sqlx::query_as(
            "select reconnect_reason, (extract(epoch from reconnect_flagged_at) * 1000)::int8, \
             coalesce(refresh_token_enc, '') <> '' from google_org_connection where id = 1",
        )
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(
            words.as_deref(),
            Some("invalid_client: The provided client secret is invalid.")
        );
        assert!(flagged_at.is_some());
        assert!(kept, "a refused client never clears the grant");
        // `org_login_email`'s own read: the gate still sees the org account.
        let gate: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "select email, refresh_token_enc from google_org_connection where id = 1",
        )
        .fetch_optional(&pg)
        .await
        .unwrap();
        assert!(
            matches!(gate, Some((Some(e), Some(r))) if e == "org@radhires.com" && !r.is_empty())
        );

        // The census counts each live grant under its client, refusals too.
        let census = grant_census(&pg).await.unwrap();
        let of = |id: Option<&str>| {
            census
                .iter()
                .find(|c| c.client_id.as_deref() == id)
                .cloned()
        };
        assert_eq!(
            of(Some("conn-a")),
            Some(ClientCensus {
                client_id: Some("conn-a".into()),
                personal: 2,
                ..Default::default()
            })
        );
        assert_eq!(
            of(Some("conn-c")),
            Some(ClientCensus {
                client_id: Some("conn-c".into()),
                org: 1,
                flagged: 1,
                ..Default::default()
            })
        );
        assert_eq!(
            of(Some("rotated-away")),
            Some(ClientCensus {
                client_id: Some("rotated-away".into()),
                personal: 1,
                ..Default::default()
            })
        );
        assert_eq!(of(None), None, "the dead grant is not live");

        // The secret fixed: its flags are forgotten, and the refresh succeeds.
        set_client(&pg, &sb, "google_oauth_client", "conn-c", "secret-conn-c").await;
        assert_eq!(clear_client_flags(&pg, "conn-c").await.unwrap(), 1);
        let got = vend(GrantRow::Org).await.unwrap().unwrap();
        assert_eq!(got.client_id.as_deref(), Some("conn-c"));
        assert_eq!(take(&seen), ["conn-c"]);

        // An agent identity, keyed by model, through the same door.
        sqlx::query(
            "insert into agent_google_connections (agent_model, google_sub, scope, refresh_token_enc, \
             access_token_enc, access_expires_at, oauth_client_id) \
             values ('stamp-live-agent', 'agent-sub', '', $1, $2, now() - interval '1 hour', 'conn-a')",
        )
        .bind(sb.seal("rt-for-conn-a").unwrap())
        .bind(sb.seal("expired-access").unwrap())
        .execute(&pg)
        .await
        .expect("agent grant");
        let got = vend(GrantRow::Agent("stamp-live-agent".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.token, "ya29.stub-conn-a");
        assert_eq!(take(&seen), ["conn-a"]);

        // A reconnect that brings no new refresh token keeps the old grant's
        // client and its flag; one that brings a new token records the
        // exchange's client and forgets the flag.
        sqlx::query("update google_connections set reconnect_reason = 'earlier words' where user_id = $1::uuid")
            .bind(&u1)
            .execute(&pg)
            .await
            .unwrap();
        let save = |refresh_token: Option<&'static str>| SaveConnection {
            google_sub: "sub-1",
            email: Some("person1@radhires.com"),
            scope: "",
            refresh_token,
            access_token: Some("ya29.from-the-exchange"),
            expires_in_seconds: Some(3599),
            oauth_client_id: "conn-c",
            now_ms: now,
        };
        save_connection(&pg, &sb, &u1, &save(None)).await.unwrap();
        assert_eq!(
            user_row(&pg, &u1).await,
            (Some("conn-a".into()), Some("earlier words".into()), true)
        );
        save_connection(&pg, &sb, &u1, &save(Some("rt-for-conn-c")))
            .await
            .unwrap();
        assert_eq!(
            user_row(&pg, &u1).await,
            (Some("conn-c".into()), None, true)
        );

        pg.close().await;
        let sql = format!("drop schema if exists {schema} cascade");
        sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&admin)
            .await
            .expect("drop the scratch schema");
    }
}
