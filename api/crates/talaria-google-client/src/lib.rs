// The Google OAuth CLIENT credentials, by ROLE. Each role's record lives in
// app_settings with the secret SEALED (secretbox); env is the fallback, and a
// record wins only when it is complete — a half-saved record never takes over
// from working env credentials.
//
// Two roles, because Google now sorts clients into agentic and not
// ("Use this client for an AI-powered agent", ticked or not, per client):
//   - CONNECTIONS (`google_oauth_client`, AUTH_GOOGLE_CLIENT_ID/_SECRET/_HD):
//     every stored grant — personal, org and per-agent connects — and Google
//     login too while no sign-in client is set. Exactly one client per role.
//   - SIGN-IN (`google_signin_oauth_client`, AUTH_GOOGLE_SIGNIN_CLIENT_ID/
//     _SECRET), optional: Google login only. Empty, login rides the
//     connections client, which is how every install ran before the split.
// The Workspace domain restriction (`hd`) belongs to the CONNECTIONS side
// alone: login reads it from there even when a sign-in client is set, so
// configuring the sign-in slot can never open login to every Google account.
//
// Each client also carries what is known about its agentic mark, keyed by
// client id in `google_client_evidence` (env pins for headless installs).
// Google exposes no API and no token claim for the mark, so for now the only
// evidence is the operator's own declaration; nothing here refuses anything
// on it.

use serde_json::Value;
use sqlx::PgPool;
use talaria_gateway::settings::{get_setting, set_setting};
use talaria_secretbox::SecretBox;

const KEY: &str = "google_oauth_client";
const SIGNIN_KEY: &str = "google_signin_oauth_client";
const EVIDENCE_KEY: &str = "google_client_evidence";
const LOGIN_KEY: &str = "google_login_enabled";

/// The jobs a Google OAuth client does here. A role names a slot, not a
/// client: one Google client may fill both, and an operator may move a client
/// from one slot to the other (the orientations docs/GOOGLE-WORKSPACE.md
/// walks through).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientRole {
    /// Personal, org and per-agent connects: every stored grant. Also login
    /// while the sign-in slot is empty.
    Connections,
    /// Google login only. Optional.
    SignIn,
}

impl ClientRole {
    pub const ALL: [ClientRole; 2] = [ClientRole::Connections, ClientRole::SignIn];

    /// The wire name — the admin route's `?role=` value.
    pub fn as_str(self) -> &'static str {
        match self {
            ClientRole::Connections => "connections",
            ClientRole::SignIn => "signin",
        }
    }

    /// `?role=` parsing: absent is connections (every caller before the split
    /// meant that client), anything unknown is None for the caller to refuse.
    pub fn parse(s: Option<&str>) -> Option<ClientRole> {
        match s {
            None | Some("connections") => Some(ClientRole::Connections),
            Some("signin") => Some(ClientRole::SignIn),
            Some(_) => None,
        }
    }

    fn settings_key(self) -> &'static str {
        match self {
            ClientRole::Connections => KEY,
            ClientRole::SignIn => SIGNIN_KEY,
        }
    }

    /// The env twin of the record: (client id var, client secret var).
    pub fn env_vars(self) -> (&'static str, &'static str) {
        match self {
            ClientRole::Connections => ("AUTH_GOOGLE_CLIENT_ID", "AUTH_GOOGLE_CLIENT_SECRET"),
            ClientRole::SignIn => (
                "AUTH_GOOGLE_SIGNIN_CLIENT_ID",
                "AUTH_GOOGLE_SIGNIN_CLIENT_SECRET",
            ),
        }
    }

    /// The env pin for the agentic declaration of THIS role's env client
    /// (`1`/`true` ticked, `0`/`false` not) — it describes the client the
    /// role's env id names, never an Admin-UI record that overrides it.
    pub fn agentic_env_var(self) -> &'static str {
        match self {
            ClientRole::Connections => "AUTH_GOOGLE_CLIENT_AGENTIC",
            ClientRole::SignIn => "AUTH_GOOGLE_SIGNIN_CLIENT_AGENTIC",
        }
    }
}

pub struct GoogleClient {
    pub client_id: String,
    pub client_secret: String,
    /// The Workspace domain restriction this client's flows enforce. For a
    /// sign-in client it is the CONNECTIONS side's, never its own.
    pub hd: Option<String>,
    /// Where the credentials came from — the admin panel labels it.
    pub source: &'static str,
    /// The slot the credentials came from. A sign-in resolution that fell
    /// back says Connections — the truth about which client login uses.
    pub role: ClientRole,
}

/// The process environment, as the pure resolvers below read it.
fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

fn str_field<'a>(stored: &'a Value, key: &str) -> Option<&'a str> {
    stored.get(key).and_then(|v| v.as_str())
}

/// A slot's OWN client — its record when complete (and its secret opens),
/// else its env pair when both halves are set — as (id, secret, source).
/// Never another slot's.
fn own_client(
    role: ClientRole,
    stored: &Value,
    open: &dyn Fn(&str) -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, String, &'static str)> {
    let client_id = str_field(stored, "clientId").filter(|s| !s.is_empty());
    let enc = str_field(stored, "clientSecretEnc").filter(|s| !s.is_empty());
    if let (Some(client_id), Some(enc)) = (client_id, enc) {
        // Sealed with a lost/rotated key → fall through to env rather than die.
        if let Some(secret) = open(enc) {
            return Some((client_id.to_string(), secret, "admin"));
        }
    }
    let (id_var, secret_var) = role.env_vars();
    let id = env(id_var).unwrap_or_default();
    let secret = env(secret_var).unwrap_or_default();
    (!id.is_empty() && !secret.is_empty()).then_some((id, secret, "env"))
}

fn env_hd(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    Some(env("AUTH_GOOGLE_HD").unwrap_or_default().trim().to_string()).filter(|s| !s.is_empty())
}

/// The connections client from its stored record and the env.
fn pick_connections(
    stored: &Value,
    open: &dyn Fn(&str) -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<GoogleClient> {
    let (client_id, client_secret, source) =
        own_client(ClientRole::Connections, stored, open, env)?;
    let hd = if source == "admin" {
        // hd: absent → null; a present string rides as-is.
        str_field(stored, "hd").map(String::from)
    } else {
        env_hd(env)
    };
    Some(GoogleClient {
        client_id,
        client_secret,
        hd,
        source,
        role: ClientRole::Connections,
    })
}

/// The domain restriction login enforces: the connections client's, whatever
/// the sign-in slot holds. With no connections client at all it is still never
/// looser than what an operator wrote down — a half-saved record's domain,
/// else AUTH_GOOGLE_HD.
fn connections_hd(
    conn_stored: &Value,
    open: &dyn Fn(&str) -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    match pick_connections(conn_stored, open, env) {
        Some(client) => client.hd,
        None => str_field(conn_stored, "hd")
            .filter(|s| !s.trim().is_empty())
            .map(String::from)
            .or_else(|| env_hd(env)),
    }
}

/// The client login runs on: the sign-in slot's own client when it has one
/// (carrying the connections side's domain), else the connections client.
fn pick_signin(
    signin_stored: &Value,
    conn_stored: &Value,
    open: &dyn Fn(&str) -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<GoogleClient> {
    match own_client(ClientRole::SignIn, signin_stored, open, env) {
        Some((client_id, client_secret, source)) => Some(GoogleClient {
            client_id,
            client_secret,
            hd: connections_hd(conn_stored, open, env),
            source,
            role: ClientRole::SignIn,
        }),
        None => pick_connections(conn_stored, open, env),
    }
}

async fn stored_record(pg: &PgPool, role: ClientRole) -> Value {
    get_setting(pg, role.settings_key(), empty_object()).await
}

/// The CONNECTIONS client — every connect flow, every token refresh, the
/// integration gate — or None when nothing usable is configured anywhere.
/// DB record wins when complete; env is the fallback.
pub async fn resolve_google_client(pg: &PgPool, sb: &SecretBox) -> Option<GoogleClient> {
    let stored = stored_record(pg, ClientRole::Connections).await;
    pick_connections(&stored, &|enc| sb.open(enc).ok(), &process_env)
}

/// The client Google LOGIN runs on (start, code exchange and the login gate):
/// the sign-in record if complete, then the sign-in env pair, then the
/// connections client. `hd` always comes from the connections side.
pub async fn resolve_google_signin_client(pg: &PgPool, sb: &SecretBox) -> Option<GoogleClient> {
    let signin = stored_record(pg, ClientRole::SignIn).await;
    let conn = stored_record(pg, ClientRole::Connections).await;
    pick_signin(&signin, &conn, &|enc| sb.open(enc).ok(), &process_env)
}

// ── Agentic evidence ─────────────────────────────────────────────────────────
//
// `google_client_evidence` = { "<clientId>": { "declaredAgentic": bool|null } }.
// Keyed by client id, not by slot, so a client moved between slots, or set in
// env, keeps its own facts. A declaration is what the operator says Google
// Cloud shows; nothing verifies it yet, and nothing refuses on it.

/// What is known about one client's agentic mark.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgenticDeclaration {
    /// true: the operator says "Use this client for an AI-powered agent" is
    /// ticked; false: unticked; None: nobody has said.
    pub declared: Option<bool>,
    /// The env var that pins the declaration, when one does — the panel then
    /// shows it read-only.
    pub pinned_by: Option<&'static str>,
}

/// An env pin's value: exact strings, like AUTH_GOOGLE_ENABLED; anything
/// else pins nothing.
fn parse_pin(v: Option<String>) -> Option<bool> {
    match v.as_deref() {
        Some("1") | Some("true") => Some(true),
        Some("0") | Some("false") => Some(false),
        _ => None,
    }
}

fn declaration_with(
    evidence: &Value,
    client_id: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> AgenticDeclaration {
    if client_id.is_empty() {
        return AgenticDeclaration::default();
    }
    for role in ClientRole::ALL {
        let (id_var, _) = role.env_vars();
        if env(id_var).as_deref() == Some(client_id)
            && let Some(pinned) = parse_pin(env(role.agentic_env_var()))
        {
            return AgenticDeclaration {
                declared: Some(pinned),
                pinned_by: Some(role.agentic_env_var()),
            };
        }
    }
    AgenticDeclaration {
        declared: evidence
            .get(client_id)
            .and_then(|e| e.get("declaredAgentic"))
            .and_then(|v| v.as_bool()),
        pinned_by: None,
    }
}

/// The whole evidence map (an empty object when nothing is stored).
pub async fn client_evidence(pg: &PgPool) -> Value {
    get_setting(pg, EVIDENCE_KEY, empty_object()).await
}

/// One client's declaration, the env pin winning over the stored one.
pub fn agentic_declaration(evidence: &Value, client_id: &str) -> AgenticDeclaration {
    declaration_with(evidence, client_id, &process_env)
}

/// Record (or, with None, withdraw) the operator's declaration for one client
/// id. One statement, so two admins declaring two clients at once both land;
/// any other facts stored under the id are kept.
pub async fn declare_agentic(
    pg: &PgPool,
    client_id: &str,
    declared: Option<bool>,
) -> Result<(), sqlx::Error> {
    let value = declared.map(Value::Bool).unwrap_or(Value::Null);
    sqlx::query(
        "insert into app_settings (key, value) \
         values ($1, jsonb_build_object($2::text, jsonb_build_object('declaredAgentic', $3::jsonb))) \
         on conflict (key) do update set value = \
           (case when jsonb_typeof(app_settings.value) = 'object' then app_settings.value else '{}'::jsonb end) \
           || jsonb_build_object($2::text, \
                (case when jsonb_typeof(app_settings.value -> $2::text) = 'object' \
                      then app_settings.value -> $2::text else '{}'::jsonb end) \
                || jsonb_build_object('declaredAgentic', $3::jsonb)), \
           updated_at = now()",
    )
    .bind(EVIDENCE_KEY)
    .bind(client_id)
    .bind(value)
    .execute(pg)
    .await
    .map(|_| ())
}

// ── Status, patch and clear, per role ────────────────────────────────────────

/// Status for the admin panel — says which source is live and surfaces a
/// half-saved record (id stored, secret missing) so the panel can finish it
/// instead of silently falling back to env. Wire order pinned by the SPA.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleClientStatus {
    /// The slot has its OWN usable client. For the sign-in slot, false means
    /// login rides the connections client.
    pub configured: bool,
    pub source: Option<&'static str>,
    pub client_id: String,
    pub secret_set: bool,
    /// The domain restriction. For the sign-in slot this is the one it
    /// inherits from the connections side — read-only there.
    pub hd: Option<String>,
    pub agentic: AgenticDeclaration,
}

fn status_with(
    role: ClientRole,
    stored: &Value,
    conn_stored: &Value,
    evidence: &Value,
    open: &dyn Fn(&str) -> Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> GoogleClientStatus {
    let own = own_client(role, stored, open, env);
    let hd = match role {
        ClientRole::SignIn => connections_hd(conn_stored, open, env),
        ClientRole::Connections if own.is_some() => {
            pick_connections(stored, open, env).and_then(|c| c.hd)
        }
        ClientRole::Connections => str_field(stored, "hd").map(String::from),
    };
    let (configured, source, client_id, secret_set) = match own {
        Some((client_id, _, source)) => (true, Some(source), client_id, true),
        None => (
            false,
            None,
            str_field(stored, "clientId")
                .unwrap_or_default()
                .to_string(),
            str_field(stored, "clientSecretEnc").is_some_and(|s| !s.is_empty()),
        ),
    };
    GoogleClientStatus {
        configured,
        source,
        agentic: declaration_with(evidence, &client_id, env),
        client_id,
        secret_set,
        hd,
    }
}

/// One role's status. Every read is fresh — the panel is the only reader.
pub async fn google_client_status(
    pg: &PgPool,
    sb: &SecretBox,
    role: ClientRole,
) -> GoogleClientStatus {
    let stored = stored_record(pg, role).await;
    let conn = match role {
        ClientRole::Connections => stored.clone(),
        ClientRole::SignIn => stored_record(pg, ClientRole::Connections).await,
    };
    let evidence = client_evidence(pg).await;
    status_with(
        role,
        &stored,
        &conn,
        &evidence,
        &|enc| sb.open(enc).ok(),
        &process_env,
    )
}

/// A configuration the panel should name, never a refusal: login still runs,
/// connects still run. `code` is stable for the SPA; `message` is the sentence.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct ClientWarning {
    pub code: &'static str,
    pub message: String,
}

/// Google's own guidance for a client used by both, quoted on every warning.
const GOOGLE_SPLIT_ADVICE: &str = "Google: \u{201c}If an existing OAuth client is used for both AI agent and non-agent features, create a separate OAuth client for the AI agent and mark only that client as agentic. Keep the existing client for your non-agent features.\u{201d}";

/// The panel's warnings, from both statuses and whether login is on.
pub fn client_warnings(
    connections: &GoogleClientStatus,
    signin: &GoogleClientStatus,
    login_enabled: bool,
) -> Vec<ClientWarning> {
    let mut out = Vec::new();
    if signin.configured && signin.agentic.declared == Some(true) {
        out.push(ClientWarning {
            code: "signin_client_agentic",
            message: format!(
                "The sign-in client is declared as marked for AI agents. Signing in is a person, not an agent: use a client without \u{201c}Use this client for an AI-powered agent\u{201d} ticked. {GOOGLE_SPLIT_ADVICE}"
            ),
        });
    }
    if login_enabled
        && !signin.configured
        && connections.configured
        && connections.agentic.declared == Some(true)
    {
        out.push(ClientWarning {
            code: "login_on_agentic_client",
            message: format!(
                "Google login runs on the connections client, which is declared as marked for AI agents. Add a sign-in client that is not marked, so people sign in through it. {GOOGLE_SPLIT_ADVICE}"
            ),
        });
    }
    out
}

/// The PUT body's patch semantics — `None` is "key absent" (keep the stored
/// value), `Some(None)` is "explicit null" (clear it). The distinction is the
/// rotation flow: save a new id without re-entering the secret.
pub struct ClientConfigPatch {
    pub client_id: Option<String>,
    pub client_secret: Option<Option<String>>,
    /// Connections only — the sign-in record never carries a domain.
    pub hd: Option<Option<String>>,
}

/// Patch one role's stored client. The secret follows the house rule: a
/// non-empty string seals + stores it, null/'' clears it, absent leaves it
/// untouched. A client id that trims to nothing is refused, and so is a
/// domain on the sign-in record — the Err surfaces as an unstructured 500;
/// the route refuses both with a 400 first.
pub async fn set_google_client_config(
    pg: &PgPool,
    sb: &SecretBox,
    role: ClientRole,
    patch: &ClientConfigPatch,
) -> Result<(), String> {
    if role == ClientRole::SignIn && matches!(&patch.hd, Some(Some(hd)) if !hd.trim().is_empty()) {
        return Err("the sign-in client takes its domain from the connections client".into());
    }
    let cur = stored_record(pg, role).await;
    let stored_str = |k: &str| str_field(&cur, k);

    let next_client_id = match &patch.client_id {
        Some(v) => v.trim().to_string(),
        None => stored_str("clientId").unwrap_or_default().to_string(),
    };
    let next_enc = match &patch.client_secret {
        Some(Some(secret)) => {
            let trimmed = secret.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(sb.seal(trimmed).map_err(|e| format!("{e}"))?)
            }
        }
        // Present but null, or present-but-blank — either way: cleared.
        Some(None) => None,
        None => stored_str("clientSecretEnc").map(String::from),
    };
    let next_hd = match &patch.hd {
        Some(Some(hd)) => {
            let trimmed = hd.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(None) => None,
        None => stored_str("hd").map(String::from),
    };

    if next_client_id.is_empty() {
        return Err("a client id is required".into());
    }

    // Literal key order (clientId, clientSecretEnc[, hd]) — the stored jsonb
    // keeps it. The sign-in record has no hd key at all.
    let mut next = serde_json::Map::new();
    next.insert("clientId".into(), Value::String(next_client_id));
    next.insert(
        "clientSecretEnc".into(),
        next_enc.map(Value::String).unwrap_or(Value::Null),
    );
    if role == ClientRole::Connections {
        next.insert(
            "hd".into(),
            next_hd.map(Value::String).unwrap_or(Value::Null),
        );
    }
    set_setting(pg, role.settings_key(), &Value::Object(next))
        .await
        .map_err(|e| format!("{e}"))
}

/// Drop one role's stored record. Env credentials (if any) take over again —
/// clearing the Admin record never disables a working env deployment; an
/// empty sign-in slot hands login back to the connections client.
pub async fn clear_google_client_config(pg: &PgPool, role: ClientRole) -> Result<(), sqlx::Error> {
    sqlx::query("delete from app_settings where key = $1")
        .bind(role.settings_key())
        .execute(pg)
        .await
        .map(|_| ())
}

// ── The login switch ─────────────────────────────────────────────────────────

/// Flip the Admin UI's login switch.
pub async fn set_google_login_enabled(pg: &PgPool, on: bool) -> Result<(), sqlx::Error> {
    set_setting(pg, LOGIN_KEY, &Value::Bool(on)).await
}

/// The env flag's view alone — `AUTH_GOOGLE_ENABLED === '1' || === 'true'`,
/// exact strings, no trimming (an operator who pinned login on in env keeps
/// it on; the UI toggle cannot turn a deliberate env policy back off).
pub fn google_login_pinned_by_env() -> bool {
    matches!(
        std::env::var("AUTH_GOOGLE_ENABLED").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Whether GOOGLE LOGIN is offered. Login is a policy decision, not a
/// credential decision: configuring a client must not by itself open a new
/// way into the instance. The switch is the Admin UI toggle (stored beside
/// the client) with the env pin winning towards ON — and either way a client
/// login can run on (the sign-in client, else the connections client) is
/// still required: a toggle with no credential behind it must never render a
/// login button that cannot work.
pub async fn google_login_enabled(pg: &PgPool, sb: &SecretBox) -> bool {
    let toggled = get_setting(pg, LOGIN_KEY, Value::Bool(false))
        .await
        .as_bool()
        == Some(true);
    (google_login_pinned_by_env() || toggled)
        && resolve_google_signin_client(pg, sb).await.is_some()
}

/// The domain of an email address, lowercased — None when there is no usable
/// one (no @, nothing before it, nothing after it, null input). The LAST @
/// wins: a plus-addressed org account gates on its REAL domain.
pub fn email_domain_of(email: Option<&str>) -> Option<String> {
    let email = email?;
    let at = email.rfind('@')?;
    if at < 1 || at == email.len() - 1 {
        return None;
    }
    Some(email[at + 1..].trim().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// A sealed secret as the fixtures write it: `sealed:<plain>` opens to
    /// `<plain>`, anything else is a secret sealed under a lost key.
    fn open(enc: &str) -> Option<String> {
        enc.strip_prefix("sealed:").map(String::from)
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    fn record(id: &str, secret: Option<&str>, hd: Option<&str>) -> Value {
        json!({ "clientId": id, "clientSecretEnc": secret.map(|s| format!("sealed:{s}")), "hd": hd })
    }

    fn signin_record(id: &str, secret: Option<&str>) -> Value {
        json!({ "clientId": id, "clientSecretEnc": secret.map(|s| format!("sealed:{s}")) })
    }

    fn signin(
        signin: &Value,
        conn: &Value,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Option<GoogleClient> {
        pick_signin(signin, conn, &open, env)
    }

    #[test]
    fn a_complete_signin_record_wins_and_keeps_the_connections_domain() {
        let conn = record("conn-a", Some("sa"), Some("radhires.com"));
        let env = env_of(&[
            ("AUTH_GOOGLE_SIGNIN_CLIENT_ID", "signin-env"),
            ("AUTH_GOOGLE_SIGNIN_CLIENT_SECRET", "se"),
        ]);
        let c = signin(&signin_record("signin-b", Some("sb")), &conn, &env).unwrap();
        assert_eq!(c.client_id, "signin-b");
        assert_eq!(c.client_secret, "sb");
        assert_eq!(c.source, "admin");
        assert_eq!(c.role, ClientRole::SignIn);
        assert_eq!(c.hd.as_deref(), Some("radhires.com"));
    }

    #[test]
    fn a_half_saved_signin_record_does_not_take_over() {
        let conn = record("conn-a", Some("sa"), None);
        let none = env_of(&[]);
        // Id without a secret, and a secret sealed under a lost key: neither
        // is a client, so login stays on the connections client.
        for half in [
            signin_record("signin-b", None),
            json!({ "clientId": "signin-b", "clientSecretEnc": "v1:lost-key" }),
            json!({ "clientId": "", "clientSecretEnc": "sealed:sb" }),
        ] {
            let c = signin(&half, &conn, &none).unwrap();
            assert_eq!(c.client_id, "conn-a");
            assert_eq!(c.role, ClientRole::Connections);
        }
        // ...and a half-saved record still lets the env twin through.
        let env = env_of(&[
            ("AUTH_GOOGLE_SIGNIN_CLIENT_ID", "signin-env"),
            ("AUTH_GOOGLE_SIGNIN_CLIENT_SECRET", "se"),
        ]);
        let c = signin(&signin_record("signin-b", None), &conn, &env).unwrap();
        assert_eq!((c.client_id.as_str(), c.source), ("signin-env", "env"));
    }

    #[test]
    fn the_signin_env_twin_needs_both_halves() {
        let conn = record("conn-a", Some("sa"), None);
        let env = env_of(&[
            ("AUTH_GOOGLE_SIGNIN_CLIENT_ID", "signin-env"),
            ("AUTH_GOOGLE_SIGNIN_CLIENT_SECRET", "se"),
        ]);
        let c = signin(&empty_object(), &conn, &env).unwrap();
        assert_eq!(c.client_id, "signin-env");
        assert_eq!(c.client_secret, "se");
        assert_eq!(c.source, "env");
        assert_eq!(c.role, ClientRole::SignIn);

        let id_only = env_of(&[("AUTH_GOOGLE_SIGNIN_CLIENT_ID", "signin-env")]);
        assert_eq!(
            signin(&empty_object(), &conn, &id_only).unwrap().client_id,
            "conn-a"
        );
    }

    #[test]
    fn an_empty_signin_slot_falls_back_to_the_connections_client() {
        // Admin record connections client.
        let c = signin(
            &empty_object(),
            &record("conn-a", Some("sa"), Some("x.io")),
            &env_of(&[]),
        )
        .unwrap();
        assert_eq!(
            (c.client_id.as_str(), c.source, c.role),
            ("conn-a", "admin", ClientRole::Connections)
        );
        assert_eq!(c.hd.as_deref(), Some("x.io"));
        // Env connections client.
        let env = env_of(&[
            ("AUTH_GOOGLE_CLIENT_ID", "conn-env"),
            ("AUTH_GOOGLE_CLIENT_SECRET", "ce"),
            ("AUTH_GOOGLE_HD", " y.io "),
        ]);
        let c = signin(&empty_object(), &empty_object(), &env).unwrap();
        assert_eq!((c.client_id.as_str(), c.source), ("conn-env", "env"));
        assert_eq!(c.hd.as_deref(), Some("y.io"));
        // Nothing anywhere: no login client.
        assert!(signin(&empty_object(), &empty_object(), &env_of(&[])).is_none());
    }

    #[test]
    fn the_signin_client_never_drops_the_connections_domain() {
        let b = signin_record("signin-b", Some("sb"));
        // Connections record complete: its hd.
        let c = signin(
            &b,
            &record("conn-a", Some("sa"), Some("radhires.com")),
            &env_of(&[]),
        )
        .unwrap();
        assert_eq!(c.hd.as_deref(), Some("radhires.com"));
        // Connections from env: AUTH_GOOGLE_HD.
        let env = env_of(&[
            ("AUTH_GOOGLE_CLIENT_ID", "conn-env"),
            ("AUTH_GOOGLE_CLIENT_SECRET", "ce"),
            ("AUTH_GOOGLE_HD", "radhires.com"),
        ]);
        let c = signin(&b, &empty_object(), &env).unwrap();
        assert_eq!(c.hd.as_deref(), Some("radhires.com"));
        // No connections client at all: a half-saved record's domain, else
        // AUTH_GOOGLE_HD — never "no restriction" while one is written down.
        let c = signin(
            &b,
            &record("conn-a", None, Some("radhires.com")),
            &env_of(&[]),
        )
        .unwrap();
        assert_eq!(c.hd.as_deref(), Some("radhires.com"));
        let c = signin(
            &b,
            &empty_object(),
            &env_of(&[("AUTH_GOOGLE_HD", "radhires.com")]),
        )
        .unwrap();
        assert_eq!(c.hd.as_deref(), Some("radhires.com"));
        // A domain only in the sign-in record is not a domain: that slot has none.
        let with_hd =
            json!({ "clientId": "signin-b", "clientSecretEnc": "sealed:sb", "hd": "other.io" });
        let c = signin(&with_hd, &record("conn-a", Some("sa"), None), &env_of(&[])).unwrap();
        assert_eq!(c.hd, None);
        // A connections record that says no domain means none, as before the
        // split — AUTH_GOOGLE_HD does not reach past a complete record.
        let c = signin(
            &b,
            &record("conn-a", Some("sa"), None),
            &env_of(&[("AUTH_GOOGLE_HD", "env.io")]),
        )
        .unwrap();
        assert_eq!(c.hd, None);
    }

    #[test]
    fn the_connections_resolver_is_unchanged() {
        let none = env_of(&[]);
        let full_env = env_of(&[
            ("AUTH_GOOGLE_CLIENT_ID", "conn-env"),
            ("AUTH_GOOGLE_CLIENT_SECRET", "ce"),
            ("AUTH_GOOGLE_HD", "env.io"),
            // The sign-in pair never feeds the connections client.
            ("AUTH_GOOGLE_SIGNIN_CLIENT_ID", "signin-env"),
            ("AUTH_GOOGLE_SIGNIN_CLIENT_SECRET", "se"),
        ]);
        // Complete record wins over env, with the record's hd (absent → None).
        let c = pick_connections(
            &record("conn-a", Some("sa"), Some("rec.io")),
            &open,
            &full_env,
        )
        .unwrap();
        assert_eq!(
            (c.client_id.as_str(), c.client_secret.as_str(), c.source),
            ("conn-a", "sa", "admin")
        );
        assert_eq!(c.hd.as_deref(), Some("rec.io"));
        assert_eq!(c.role, ClientRole::Connections);
        let c = pick_connections(
            &json!({ "clientId": "conn-a", "clientSecretEnc": "sealed:sa" }),
            &open,
            &full_env,
        )
        .unwrap();
        assert_eq!(c.hd, None);
        // A half-saved record, or one sealed under a lost key, yields to env.
        for half in [
            record("conn-a", None, Some("rec.io")),
            json!({ "clientId": "conn-a", "clientSecretEnc": "v1:lost" }),
        ] {
            let c = pick_connections(&half, &open, &full_env).unwrap();
            assert_eq!((c.client_id.as_str(), c.source), ("conn-env", "env"));
            assert_eq!(c.hd.as_deref(), Some("env.io"));
        }
        // Nothing usable anywhere → None, even with a sign-in client set.
        assert!(pick_connections(&empty_object(), &open, &none).is_none());
        let signin_only = env_of(&[
            ("AUTH_GOOGLE_SIGNIN_CLIENT_ID", "signin-env"),
            ("AUTH_GOOGLE_SIGNIN_CLIENT_SECRET", "se"),
        ]);
        assert!(pick_connections(&empty_object(), &open, &signin_only).is_none());
    }

    #[test]
    fn roles_parse_from_the_query() {
        assert_eq!(ClientRole::parse(None), Some(ClientRole::Connections));
        assert_eq!(
            ClientRole::parse(Some("connections")),
            Some(ClientRole::Connections)
        );
        assert_eq!(ClientRole::parse(Some("signin")), Some(ClientRole::SignIn));
        assert_eq!(ClientRole::parse(Some("agent")), None);
        assert_eq!(ClientRole::parse(Some("")), None);
        for role in ClientRole::ALL {
            assert_eq!(ClientRole::parse(Some(role.as_str())), Some(role));
        }
    }

    #[test]
    fn declarations_are_keyed_by_client_id_and_env_pins_win() {
        let evidence = json!({
            "conn-a": { "declaredAgentic": true },
            "signin-b": { "declaredAgentic": false },
            "other": { "declaredAgentic": null },
        });
        let none = env_of(&[]);
        let d =
            |id: &str, env: &dyn Fn(&str) -> Option<String>| declaration_with(&evidence, id, env);
        assert_eq!(
            d("conn-a", &none),
            AgenticDeclaration {
                declared: Some(true),
                pinned_by: None
            }
        );
        assert_eq!(d("signin-b", &none).declared, Some(false));
        assert_eq!(d("other", &none).declared, None);
        assert_eq!(d("never-seen", &none), AgenticDeclaration::default());
        assert_eq!(d("", &none), AgenticDeclaration::default());

        // The pin describes the client its own env id names.
        let env = env_of(&[
            ("AUTH_GOOGLE_CLIENT_ID", "conn-a"),
            ("AUTH_GOOGLE_CLIENT_AGENTIC", "0"),
            ("AUTH_GOOGLE_SIGNIN_CLIENT_ID", "signin-env"),
            ("AUTH_GOOGLE_SIGNIN_CLIENT_AGENTIC", "true"),
        ]);
        assert_eq!(
            d("conn-a", &env),
            AgenticDeclaration {
                declared: Some(false),
                pinned_by: Some("AUTH_GOOGLE_CLIENT_AGENTIC")
            }
        );
        assert_eq!(
            d("signin-env", &env),
            AgenticDeclaration {
                declared: Some(true),
                pinned_by: Some("AUTH_GOOGLE_SIGNIN_CLIENT_AGENTIC")
            }
        );
        // ...and nothing else: an Admin-UI client is not the env's.
        assert_eq!(
            d("signin-b", &env),
            AgenticDeclaration {
                declared: Some(false),
                pinned_by: None
            }
        );
        // A pin that is not an exact flag pins nothing.
        let fuzzy = env_of(&[
            ("AUTH_GOOGLE_CLIENT_ID", "conn-a"),
            ("AUTH_GOOGLE_CLIENT_AGENTIC", "yes"),
        ]);
        assert_eq!(
            d("conn-a", &fuzzy),
            AgenticDeclaration {
                declared: Some(true),
                pinned_by: None
            }
        );
    }

    #[test]
    fn statuses_per_role() {
        let conn = record("conn-a", Some("sa"), Some("radhires.com"));
        let evidence = json!({ "conn-a": { "declaredAgentic": true }, "signin-b": { "declaredAgentic": false } });
        let none = env_of(&[]);

        let s = status_with(
            ClientRole::Connections,
            &conn,
            &conn,
            &evidence,
            &open,
            &none,
        );
        assert!(s.configured);
        assert_eq!(
            (s.source, s.client_id.as_str(), s.secret_set),
            (Some("admin"), "conn-a", true)
        );
        assert_eq!(s.hd.as_deref(), Some("radhires.com"));
        assert_eq!(s.agentic.declared, Some(true));

        // An empty sign-in slot: not configured, no id, the inherited domain.
        let s = status_with(
            ClientRole::SignIn,
            &empty_object(),
            &conn,
            &evidence,
            &open,
            &none,
        );
        assert!(!s.configured);
        assert_eq!(
            (s.source, s.client_id.as_str(), s.secret_set),
            (None, "", false)
        );
        assert_eq!(s.hd.as_deref(), Some("radhires.com"));
        assert_eq!(s.agentic, AgenticDeclaration::default());

        // A half-saved sign-in record shows its id for the panel to finish.
        let s = status_with(
            ClientRole::SignIn,
            &signin_record("signin-b", None),
            &conn,
            &evidence,
            &open,
            &none,
        );
        assert!(!s.configured);
        assert_eq!((s.client_id.as_str(), s.secret_set), ("signin-b", false));
        assert_eq!(s.agentic.declared, Some(false));

        // A complete one, with the connections domain.
        let s = status_with(
            ClientRole::SignIn,
            &signin_record("signin-b", Some("sb")),
            &conn,
            &evidence,
            &open,
            &none,
        );
        assert!(s.configured);
        assert_eq!(
            (s.source, s.client_id.as_str()),
            (Some("admin"), "signin-b")
        );
        assert_eq!(s.hd.as_deref(), Some("radhires.com"));

        // The wire, order pinned.
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"configured":true,"source":"admin","clientId":"signin-b","secretSet":true,"hd":"radhires.com","agentic":{"declared":false,"pinnedBy":null}}"#
        );
    }

    fn st(configured: bool, declared: Option<bool>) -> GoogleClientStatus {
        GoogleClientStatus {
            configured,
            source: configured.then_some("admin"),
            client_id: if configured {
                "c".into()
            } else {
                String::new()
            },
            secret_set: configured,
            hd: None,
            agentic: AgenticDeclaration {
                declared,
                pinned_by: None,
            },
        }
    }

    #[test]
    fn warnings_name_an_agentic_client_on_sign_in_and_never_anything_else() {
        let codes = |conn: &GoogleClientStatus, signin: &GoogleClientStatus, login: bool| {
            client_warnings(conn, signin, login)
                .into_iter()
                .map(|w| w.code)
                .collect::<Vec<_>>()
        };
        // The sign-in client declared agentic: warned whether login is on or not.
        for login in [true, false] {
            assert_eq!(
                codes(&st(true, None), &st(true, Some(true)), login),
                vec!["signin_client_agentic"]
            );
        }
        // Login on, no sign-in client, connections declared agentic.
        assert_eq!(
            codes(&st(true, Some(true)), &st(false, None), true),
            vec!["login_on_agentic_client"]
        );
        // ...not while login is off, and not once a sign-in client is set.
        assert!(codes(&st(true, Some(true)), &st(false, None), false).is_empty());
        assert!(codes(&st(true, Some(true)), &st(true, Some(false)), true).is_empty());
        assert!(codes(&st(true, Some(true)), &st(true, None), true).is_empty());
        // Unknown or non-agentic: nothing to say.
        for d in [None, Some(false)] {
            assert!(codes(&st(true, d), &st(false, None), true).is_empty());
            assert!(codes(&st(true, None), &st(true, d), true).is_empty());
        }
        // A half-saved sign-in record serves nothing, so it warns nothing.
        assert!(codes(&st(true, None), &st(false, Some(true)), true).is_empty());
        // Both at once.
        assert_eq!(
            codes(&st(true, Some(true)), &st(true, Some(true)), true),
            vec!["signin_client_agentic"]
        );
        // Every message quotes Google's checkbox or guidance, not a paraphrase.
        for w in client_warnings(&st(true, Some(true)), &st(true, Some(true)), true)
            .into_iter()
            .chain(client_warnings(
                &st(true, Some(true)),
                &st(false, None),
                true,
            ))
        {
            assert!(
                w.message
                    .contains("Keep the existing client for your non-agent features.")
            );
        }
    }

    #[test]
    fn pins_parse_exact_strings_only() {
        assert_eq!(parse_pin(Some("1".into())), Some(true));
        assert_eq!(parse_pin(Some("true".into())), Some(true));
        assert_eq!(parse_pin(Some("0".into())), Some(false));
        assert_eq!(parse_pin(Some("false".into())), Some(false));
        assert_eq!(parse_pin(Some("TRUE".into())), None);
        assert_eq!(parse_pin(Some("".into())), None);
        assert_eq!(parse_pin(None), None);
    }
}
