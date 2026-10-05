// Coding accounts — a Developer Agent's own sign-ins to coding-agent
// subscriptions (Claude Pro/Max, ChatGPT Codex, GitHub Copilot, Gemini, …),
// one account per service per agent.
//
// WHAT THIS CHANGES FOR A JOB. The workbench harness normally reaches models
// through Talaria's gateway on the workbench credential. An agent with a
// coding account signed in runs its harness on THAT account instead, which is
// the whole point: the person's existing subscription does the coding work.
// The persona driving the harness is untouched — it keeps the model its agent
// def configures, through the gateway, metered as always. One consequence
// worth stating plainly because nothing else will say it: harness spend on a
// coding account does NOT pass through the gateway, so it does not land in
// Talaria's ledger. The provider's own dashboard is the record.
//
// WHO HOLDS THE CREDENTIAL. This instance does. The whole credential — refresh
// token included — seals with secretbox into `agent_coding_accounts`, and the
// agent's omp reads it over omp's auth-broker protocol, which hands clients an
// access token and a `__remote__` sentinel where the refresh token would be.
// That makes Talaria the canonical refresher, so the credential cannot drift
// out from under us and a volume reset cannot lose it. Performing the flows
// and the refreshes is `talaria-omp-auth`'s job (it runs omp's own engine);
// this crate is the custody, the policy and the resolution.
//
// WHICH PLAN RUNS A JOB. A plan is either one of the agent's coding accounts
// or the Talaria gateway itself, and the gateway is a CHOICE rather than only
// a fallback: an agent with subscriptions signed in can still be told to run
// some or all of its coding work through the gateway on models the org picked.
// Three layers decide, narrowest first, because a subscription runs out
// mid-ticket and the fix has to be local:
//   1. the TICKET's pin — this ticket on that plan, optionally naming one
//      model, outranking everything;
//   2. the agent's PRIMARY account — or the gateway, when no account is
//      primary, which is how an agent keeps its coding work on the gateway
//      while holding credentials for later;
//   3. nothing configured — the org's Workbench model roles, exactly as
//      before this feature existed.
// Within the chosen plan, a role the plan does not fill inherits its
// `default`, and a plan that names nothing at all leaves the org's roles
// standing. Role picks hang off the PLAN rather than the agent for that
// reason: swapping plans has to swap a coherent set of models with it, not
// leave `smol` on the subscription that just ran out.
//
// THE ADMIN GATE. Two settings, both read here so no caller invents its own
// answer: the feature is off until an admin turns it on, and the services an
// org permits are an explicit allowlist. An account already signed in to a
// service that later leaves the allowlist stops being served to the harness
// but is NOT deleted — revoking a policy should not silently throw away a
// credential a person will want back when it is re-permitted.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::Row as _;

use talaria_secretbox::SecretBox;

/// omp's model roles, in the order the UI shows them. `default` rides
/// `--model` on the invocation line; the rest ride env vars. Keep in step with
/// `talaria_workbench_harnesses::OmpRoles`.
pub const ROLES: [&str; 4] = ["default", "smol", "slow", "plan"];

/// Whether the feature is available at all.
pub const SETTING_ENABLED: &str = "coding_accounts.enabled";
/// Which services an org permits — a list of omp store-provider ids. Absent
/// means "none permitted yet", which is not the same as "all": an admin turns
/// the feature on and then says what it may reach.
pub const SETTING_SERVICES: &str = "coding_accounts.services";

// ── Policy ──────────────────────────────────────────────────────────────────

pub async fn enabled(pg: &PgPool) -> bool {
    talaria_settings::get_setting(pg, SETTING_ENABLED, json!(false))
        .await
        .as_bool()
        .unwrap_or(false)
}

/// The permitted service ids. Order is the admin's; duplicates and blanks are
/// dropped so a hand-edited setting cannot produce a doubled row in the UI.
pub async fn permitted_services(pg: &PgPool) -> Vec<String> {
    let raw = talaria_settings::get_setting(pg, SETTING_SERVICES, json!([])).await;
    let mut out: Vec<String> = Vec::new();
    for v in raw.as_array().into_iter().flatten() {
        let Some(s) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        if !out.iter().any(|existing| existing == s) {
            out.push(s.to_string());
        }
    }
    out
}

/// The gate every write and every login start goes through.
pub async fn service_permitted(pg: &PgPool, service: &str) -> bool {
    enabled(pg).await && permitted_services(pg).await.contains(&service.to_string())
}

pub async fn set_enabled(pg: &PgPool, on: bool) -> Result<(), sqlx::Error> {
    talaria_settings::set_setting(pg, SETTING_ENABLED, &json!(on)).await
}

pub async fn set_permitted_services(pg: &PgPool, services: &[String]) -> Result<(), sqlx::Error> {
    talaria_settings::set_setting(pg, SETTING_SERVICES, &json!(services)).await
}

// ── Rows ────────────────────────────────────────────────────────────────────

/// One signed-in account as the UI reads it. No credential material: the
/// identity columns are lifted out at login precisely so a list render never
/// unseals anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: i64,
    /// The store id — the service. `openai-codex`, never `openai-codex-device`.
    pub provider: String,
    /// The door the person signed in through, which may differ from `provider`.
    pub login_provider: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub org_id: Option<String>,
    pub org_name: Option<String>,
    /// The agent's default plan — what a job that pins nothing runs on.
    pub primary: bool,
    /// Whether this service is still on the org's allowlist. A `false` here is
    /// why an account can be signed in and yet drive nothing.
    pub permitted: bool,
    pub expires_at: Option<String>,
    pub authorized_at: Option<String>,
    pub disabled_at: Option<String>,
    pub disabled_cause: Option<String>,
    pub created_at: Option<String>,
    /// role → model, this account's own picks.
    pub roles: Value,
}

/// The identity slice a finished login reports, lifted out of the credential.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub org_id: Option<String>,
    pub org_name: Option<String>,
    pub identity_key: Option<String>,
    /// Epoch ms, from the credential; `None` for a credential that never expires.
    pub expires: Option<i64>,
    pub authorized_at: Option<i64>,
}

impl Identity {
    /// Read the display slice straight off a credential the bridge returned.
    /// `expires` is dropped when it is omp's never-expires sentinel territory
    /// (anything beyond ~year 2286 in epoch ms), so the UI says "no expiry"
    /// instead of printing a nonsense date.
    pub fn from_credential(credential: &Value, identity_key: Option<&str>) -> Self {
        let s = |k: &str| {
            credential
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let expires = credential.get("expires").and_then(Value::as_i64);
        Self {
            email: s("email"),
            account_id: s("accountId"),
            org_id: s("orgId"),
            org_name: s("orgName"),
            identity_key: identity_key
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            expires: expires.filter(|ms| *ms > 0 && *ms < 10_000_000_000_000),
            authorized_at: credential.get("authorizedAt").and_then(Value::as_i64),
        }
    }
}

fn iso(ms: Option<i64>) -> Option<String> {
    ms.map(talaria_agent_auth::epoch_ms_to_iso)
}

/// Every account on one agent, service order, each with its role picks.
pub async fn list_accounts(pg: &PgPool, agent_id: &str) -> Result<Vec<Account>, sqlx::Error> {
    let permitted = permitted_services(pg).await;
    let rows = sqlx::query(
        "select a.id, a.provider, a.login_provider, a.email, a.account_id, a.org_id, \
                a.org_name, a.is_primary, a.disabled_cause, \
                (extract(epoch from a.expires_at) * 1000)::bigint as expires_ms, \
                (extract(epoch from a.authorized_at) * 1000)::bigint as authorized_ms, \
                (extract(epoch from a.disabled_at) * 1000)::bigint as disabled_ms, \
                (extract(epoch from a.created_at) * 1000)::bigint as created_ms, \
                coalesce( \
                  (select jsonb_object_agg(r.role, r.model) \
                     from agent_coding_roles r where r.account_id = a.id), \
                  '{}'::jsonb) as roles \
         from agent_coding_accounts a where a.agent_id::text = $1 order by a.provider",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let provider: String = r.get("provider");
            Account {
                id: r.get("id"),
                permitted: permitted.contains(&provider),
                provider,
                login_provider: r.get("login_provider"),
                email: r.get("email"),
                account_id: r.get("account_id"),
                org_id: r.get("org_id"),
                org_name: r.get("org_name"),
                primary: r.get("is_primary"),
                expires_at: iso(r.get("expires_ms")),
                authorized_at: iso(r.get("authorized_ms")),
                disabled_at: iso(r.get("disabled_ms")),
                disabled_cause: r.get("disabled_cause"),
                created_at: iso(r.get("created_ms")),
                roles: r.get("roles"),
            }
        })
        .collect())
}

/// Store a finished login. Upserting on (agent, service) is what makes "one
/// account per service, per agent" true rather than aspirational: signing in
/// again — even through a different door, even as a different person — replaces
/// the account instead of adding a second one the harness would have to choose
/// between. The row's id survives, so role picks and ticket pins that name it
/// keep naming it.
///
/// The first account an agent gets becomes its primary, because an agent with
/// exactly one plan and no default would be a configuration step that exists
/// only to be annoying.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_account(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    provider: &str,
    login_provider: &str,
    credential: &Value,
    identity: &Identity,
    created_by: Option<&str>,
) -> Result<i64, String> {
    let sealed = sb
        .seal(&credential.to_string())
        .map_err(|e| format!("coding account seal failed: {e}"))?;
    let id: i64 = sqlx::query_scalar(
        "insert into agent_coding_accounts \
           (agent_id, provider, login_provider, credential_enc, identity_key, email, \
            account_id, org_id, org_name, is_primary, expires_at, authorized_at, created_by) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7, $8, $9, \
                 not exists (select 1 from agent_coding_accounts where agent_id = $1::uuid), \
                 to_timestamp($10::double precision / 1000), \
                 to_timestamp($11::double precision / 1000), $12::uuid) \
         on conflict (agent_id, provider) do update set \
           login_provider = excluded.login_provider, \
           credential_enc = excluded.credential_enc, \
           identity_key = excluded.identity_key, \
           email = excluded.email, \
           account_id = excluded.account_id, \
           org_id = excluded.org_id, \
           org_name = excluded.org_name, \
           expires_at = excluded.expires_at, \
           authorized_at = excluded.authorized_at, \
           disabled_at = null, \
           disabled_cause = null, \
           updated_at = now() \
         returning id",
    )
    .bind(agent_id)
    .bind(provider)
    .bind(login_provider)
    .bind(&sealed)
    .bind(identity.identity_key.as_deref())
    .bind(identity.email.as_deref())
    .bind(identity.account_id.as_deref())
    .bind(identity.org_id.as_deref())
    .bind(identity.org_name.as_deref())
    .bind(identity.expires)
    .bind(identity.authorized_at)
    .bind(created_by)
    .fetch_one(pg)
    .await
    .map_err(|e| format!("coding account write failed: {e}"))?;
    Ok(id)
}

/// Point the agent's default at one of its accounts. Clearing the old primary
/// and setting the new one happen in one transaction, because the schema's
/// partial unique index would refuse the overlap — which is the index doing
/// its job.
pub async fn set_primary(pg: &PgPool, agent_id: &str, id: i64) -> Result<bool, sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query(
        "update agent_coding_accounts set is_primary = false, updated_at = now() \
         where agent_id::text = $1 and is_primary",
    )
    .bind(agent_id)
    .execute(&mut *tx)
    .await?;
    let done = sqlx::query(
        "update agent_coding_accounts set is_primary = true, updated_at = now() \
         where agent_id::text = $1 and id = $2",
    )
    .bind(agent_id)
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(done > 0)
}

/// Sign an account out. Its role picks and any ticket pins naming it cascade
/// away, which is the right answer: a pin pointing at a credential that no
/// longer exists would strand a job. If it was the primary, the agent's
/// longest-standing remaining account takes over — leaving an agent with
/// accounts but no default would make the next job's model depend on row order.
pub async fn delete_account(pg: &PgPool, agent_id: &str, id: i64) -> Result<bool, sqlx::Error> {
    let mut tx = pg.begin().await?;
    let done =
        sqlx::query("delete from agent_coding_accounts where agent_id::text = $1 and id = $2")
            .bind(agent_id)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if done > 0 {
        sqlx::query(
            "update agent_coding_accounts set is_primary = true, updated_at = now() \
             where id = ( \
               select id from agent_coding_accounts \
               where agent_id::text = $1 \
                 and not exists (select 1 from agent_coding_accounts \
                                 where agent_id::text = $1 and is_primary) \
               order by created_at, id limit 1)",
        )
        .bind(agent_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(done > 0)
}

// ── Credentials (broker-side) ───────────────────────────────────────────────

/// One account with its credential open — the broker's read, and the only
/// place plaintext exists outside the bridge.
pub struct OpenAccount {
    pub id: i64,
    pub provider: String,
    pub identity_key: Option<String>,
    pub credential: Value,
    pub disabled: bool,
}

/// Every *servable* account for an agent: signed in, not disabled, and still
/// on the org's allowlist. A credential that fails to open is SKIPPED rather
/// than failing the whole snapshot — one unreadable row (a restore without the
/// root key) must not take a working agent's other accounts down with it.
pub async fn open_accounts(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
) -> Result<Vec<OpenAccount>, sqlx::Error> {
    let permitted = permitted_services(pg).await;
    // `disabled_at is not null` rather than the timestamp itself: the caller
    // only asks "is this servable", and selecting the raw `timestamptz` into
    // an `Option<i64>` decoded fine for every NULL and panicked on the first
    // row that was actually disabled. Returning the boolean the caller wants
    // makes the Rust type right by construction instead of by discipline.
    let rows = sqlx::query(
        "select id, provider, identity_key, credential_enc, \
                (disabled_at is not null) as disabled \
         from agent_coding_accounts where agent_id::text = $1 order by id",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let provider: String = r.get("provider");
        if !permitted.contains(&provider) {
            continue;
        }
        let sealed: String = r.get("credential_enc");
        let Ok(plain) = sb.open(&sealed) else {
            continue;
        };
        let Ok(credential) = serde_json::from_str::<Value>(&plain) else {
            continue;
        };
        out.push(OpenAccount {
            id: r.get("id"),
            provider,
            identity_key: r.get("identity_key"),
            credential,
            disabled: r.get("disabled"),
        });
    }
    Ok(out)
}

/// One account by id with its credential open — the refresh path's read,
/// routed through `open_accounts` so it inherits the same allowlist and
/// unsealable-row rules rather than growing a second answer.
pub async fn open_account(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    id: i64,
) -> Result<Option<OpenAccount>, sqlx::Error> {
    Ok(open_accounts(pg, sb, agent_id)
        .await?
        .into_iter()
        .find(|a| a.id == id))
}

/// Replace one account's credential after a refresh. Keyed by id alone: the
/// refresher already resolved the agent, and a refresh must not depend on the
/// allowlist that may have changed under it mid-flight.
pub async fn store_refreshed(
    pg: &PgPool,
    sb: &SecretBox,
    id: i64,
    credential: &Value,
) -> Result<(), String> {
    let sealed = sb
        .seal(&credential.to_string())
        .map_err(|e| format!("coding account seal failed: {e}"))?;
    let expires = credential
        .get("expires")
        .and_then(Value::as_i64)
        .filter(|ms| *ms > 0 && *ms < 10_000_000_000_000);
    sqlx::query(
        "update agent_coding_accounts set credential_enc = $1, \
           expires_at = to_timestamp($2::double precision / 1000), updated_at = now() \
         where id = $3",
    )
    .bind(&sealed)
    .bind(expires)
    .bind(id)
    .execute(pg)
    .await
    .map_err(|e| format!("coding account refresh write failed: {e}"))?;
    Ok(())
}

/// Mark an account the harness could not use. omp calls this when a provider
/// answers `invalid_grant` — the credential is dead and only a re-login fixes
/// it, so the row stays (with the reason) and stops being served.
pub async fn disable_account(pg: &PgPool, id: i64, cause: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update agent_coding_accounts set disabled_at = now(), disabled_cause = $1, \
           updated_at = now() where id = $2 and disabled_at is null",
    )
    .bind(cause)
    .bind(id)
    .execute(pg)
    .await?;
    Ok(())
}

// ── Rate-limit blocks (the harness's own cache) ─────────────────────────────

pub async fn blocks_for(pg: &PgPool, account_id: i64) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(
        "select provider_key, block_scope, blocked_until_ms, updated_at_ms \
         from agent_coding_account_blocks where account_id = $1 \
         order by provider_key, block_scope",
    )
    .bind(account_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut o = serde_json::Map::new();
            o.insert(
                "providerKey".into(),
                json!(r.get::<String, _>("provider_key")),
            );
            o.insert(
                "blockScope".into(),
                json!(r.get::<String, _>("block_scope")),
            );
            o.insert(
                "blockedUntilMs".into(),
                json!(r.get::<i64, _>("blocked_until_ms")),
            );
            if let Some(ms) = r.get::<Option<i64>, _>("updated_at_ms") {
                o.insert("updatedAtMs".into(), json!(ms));
            }
            Value::Object(o)
        })
        .collect())
}

pub async fn put_block(
    pg: &PgPool,
    account_id: i64,
    provider_key: &str,
    block_scope: &str,
    blocked_until_ms: i64,
    updated_at_ms: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into agent_coding_account_blocks \
           (account_id, provider_key, block_scope, blocked_until_ms, updated_at_ms) \
         values ($1, $2, $3, $4, $5) \
         on conflict (account_id, provider_key, block_scope) do update set \
           blocked_until_ms = excluded.blocked_until_ms, \
           updated_at_ms = excluded.updated_at_ms",
    )
    .bind(account_id)
    .bind(provider_key)
    .bind(block_scope)
    .bind(blocked_until_ms)
    .bind(updated_at_ms)
    .execute(pg)
    .await?;
    Ok(())
}

/// Clear blocks. `scope` of `None` clears every scope for the account (the
/// client's `DELETE …/blocks`); `Some(("provider", "scope"))` clears the one
/// row (`DELETE …/block`, where an empty scope targets the provider-wide row).
pub async fn clear_blocks(
    pg: &PgPool,
    account_id: i64,
    scope: Option<(&str, &str)>,
) -> Result<(), sqlx::Error> {
    match scope {
        Some((provider_key, block_scope)) => {
            sqlx::query(
                "delete from agent_coding_account_blocks \
                 where account_id = $1 and provider_key = $2 and block_scope = $3",
            )
            .bind(account_id)
            .bind(provider_key)
            .bind(block_scope)
            .execute(pg)
            .await?;
        }
        None => {
            sqlx::query("delete from agent_coding_account_blocks where account_id = $1")
                .bind(account_id)
                .execute(pg)
                .await?;
        }
    }
    Ok(())
}

// ── Role picks, per plan ────────────────────────────────────────────────────

/// Which plan a role pick, a ticket pin or a resolution is about. The gateway
/// is a plan: this is the type that keeps "run it on the org's gateway" a
/// first-class answer instead of something you can only get by configuring
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// The org's Talaria gateway — metered, on models the org configured.
    Gateway,
    /// One of the agent's signed-in coding accounts.
    Account(i64),
}

impl Plan {
    pub fn account_id(self) -> Option<i64> {
        match self {
            Plan::Gateway => None,
            Plan::Account(id) => Some(id),
        }
    }

    /// `None` (no account named) is the gateway — the wire spelling the routes
    /// and the UI use, so neither has to invent a sentinel.
    pub fn from_account_id(id: Option<i64>) -> Self {
        match id {
            Some(id) => Plan::Account(id),
            None => Plan::Gateway,
        }
    }
}

/// Replace one plan's role picks. An absent role is a role that falls back, so
/// clearing one is how you hand it back — there is no separate "unset" verb to
/// get out of step with this one. An account plan is scoped to the agent, so a
/// pick cannot be written onto another agent's account by id.
pub async fn set_role_picks(
    pg: &PgPool,
    agent_id: &str,
    plan: Plan,
    picks: &[(String, String)],
) -> Result<bool, sqlx::Error> {
    if let Plan::Account(account_id) = plan {
        let owned: Option<(i64,)> = sqlx::query_as(
            "select id from agent_coding_accounts where id = $1 and agent_id::text = $2",
        )
        .bind(account_id)
        .bind(agent_id)
        .fetch_optional(pg)
        .await?;
        if owned.is_none() {
            return Ok(false);
        }
    }
    let account_id = plan.account_id();
    let mut tx = pg.begin().await?;
    sqlx::query(
        "delete from agent_coding_roles \
         where agent_id::text = $1 and account_id is not distinct from $2",
    )
    .bind(agent_id)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    for (role, model) in picks {
        sqlx::query(
            "insert into agent_coding_roles (agent_id, account_id, role, model) \
             values ($1::uuid, $2, $3, $4)",
        )
        .bind(agent_id)
        .bind(account_id)
        .bind(role)
        .bind(model)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// One plan's role picks, as the UI reads them.
pub async fn role_picks(
    pg: &PgPool,
    agent_id: &str,
    plan: Plan,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "select role, model from agent_coding_roles \
         where agent_id::text = $1 and account_id is not distinct from $2",
    )
    .bind(agent_id)
    .bind(plan.account_id())
    .fetch_all(pg)
    .await
}

// ── The per-ticket pin ──────────────────────────────────────────────────────

/// A ticket's pinned plan, as the ticket strip and `start_job` read it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPin {
    /// Null for the Talaria gateway; otherwise the account.
    pub account_id: Option<i64>,
    /// `talaria` for the gateway, else the account's service.
    pub provider: String,
    pub email: Option<String>,
    /// The one model this ticket names, overriding the plan's `default` pick
    /// on the invocation line. Null means the plan's own default.
    pub model: Option<String>,
    pub pinned_at: Option<String>,
}

pub async fn task_pin(pg: &PgPool, task_id: &str) -> Result<Option<TaskPin>, sqlx::Error> {
    let row = sqlx::query(
        "select p.account_id, a.provider, a.email, p.model, \
                (extract(epoch from p.pinned_at) * 1000)::bigint as pinned_ms \
         from task_coding_pins p \
         left join agent_coding_accounts a on a.id = p.account_id \
         where p.task_id = $1::uuid",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|r| TaskPin {
        account_id: r.get("account_id"),
        provider: r
            .get::<Option<String>, _>("provider")
            .unwrap_or_else(|| GATEWAY_PROVIDER.to_string()),
        email: r.get("email"),
        model: r.get("model"),
        pinned_at: iso(r.get("pinned_ms")),
    }))
}

/// Pin a ticket to one of the agent's plans, optionally naming the model.
/// An account plan must belong to `agent_id` — the caller resolved which agent
/// the ticket is on, and a pin naming another agent's account would be a
/// credential reach dressed up as a preference; such a pin writes nothing and
/// answers `false`. Pinning the gateway always works: there is no credential
/// to reach for.
pub async fn set_task_pin(
    pg: &PgPool,
    task_id: &str,
    agent_id: &str,
    plan: Plan,
    model: Option<&str>,
    pinned_by: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let done = match plan {
        Plan::Gateway => sqlx::query(
            "insert into task_coding_pins (task_id, account_id, model, pinned_by) \
             values ($1::uuid, null, $2, $3::uuid) \
             on conflict (task_id) do update set \
               account_id = null, model = excluded.model, \
               pinned_by = excluded.pinned_by, pinned_at = now()",
        )
        .bind(task_id)
        .bind(model)
        .bind(pinned_by)
        .execute(pg)
        .await?
        .rows_affected(),
        Plan::Account(account_id) => sqlx::query(
            "insert into task_coding_pins (task_id, account_id, model, pinned_by) \
             select $1::uuid, a.id, $4, $5::uuid from agent_coding_accounts a \
             where a.id = $3 and a.agent_id::text = $2 \
             on conflict (task_id) do update set \
               account_id = excluded.account_id, model = excluded.model, \
               pinned_by = excluded.pinned_by, pinned_at = now()",
        )
        .bind(task_id)
        .bind(agent_id)
        .bind(account_id)
        .bind(model)
        .bind(pinned_by)
        .execute(pg)
        .await?
        .rows_affected(),
    };
    Ok(done > 0)
}

pub async fn clear_task_pin(pg: &PgPool, task_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from task_coding_pins where task_id = $1::uuid")
        .bind(task_id)
        .execute(pg)
        .await?;
    Ok(())
}

// ── Resolution: which plan, which models ────────────────────────────────────

/// The provider id omp knows the Talaria gateway by — the one
/// `talaria-workbench-harnesses` declares in each agent's `models.json`.
pub const GATEWAY_PROVIDER: &str = "talaria";

/// The plan a job will actually run on, with its models resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub plan: Plan,
    /// `talaria` for the gateway, else the account's service.
    pub provider: String,
    pub email: Option<String>,
    /// Why this plan: `"ticket"` (pinned) or `"primary"` (the agent's default).
    pub source: &'static str,
    /// role → model, for the roles this plan fills. A role missing here falls
    /// back to the org's Workbench gateway role.
    pub roles: Vec<(String, String)>,
}

impl Resolved {
    /// Whether this resolution is the gateway — in which case the harness is
    /// metered as it always was, and nothing bills a subscription.
    pub fn is_gateway(&self) -> bool {
        self.plan == Plan::Gateway
    }

    pub fn model_for(&self, role: &str) -> Option<&str> {
        self.roles
            .iter()
            .find(|(r, _)| r == role)
            .map(|(_, m)| m.as_str())
    }

    /// `<provider>/<model>` — how omp's CLI and role env name a model. The
    /// gateway spells itself the same way it already did (`talaria/<id>`), so
    /// a gateway plan and an unconfigured agent produce identical lines.
    pub fn model_arg(&self, role: &str) -> Option<String> {
        self.model_for(role)
            .map(|m| format!("{}/{}", self.provider, m))
    }
}

/// Decide which models a role set resolves to for one plan. Split out from the
/// query so the fall-down is testable without a database: the ticket's model
/// wins for `default`, then the plan's own picks, then every unfilled role
/// inherits whatever `default` ended up as — so pinning a plan that named only
/// a default still moves the whole job onto that plan instead of splitting it
/// across two providers.
fn resolve_roles(picks: &[(String, String)], pin_model: Option<String>) -> Vec<(String, String)> {
    let default_model = pin_model.or_else(|| {
        picks
            .iter()
            .find(|(r, _)| r == "default")
            .map(|(_, m)| m.clone())
    });
    let mut roles = Vec::new();
    for role in ROLES {
        let model = if role == "default" {
            default_model.clone()
        } else {
            picks
                .iter()
                .find(|(r, _)| r == role)
                .map(|(_, m)| m.clone())
                .or_else(|| default_model.clone())
        };
        if let Some(model) = model {
            roles.push((role.to_string(), model));
        }
    }
    roles
}

/// Resolve the plan for a job: the ticket's pin when it has one, else the
/// agent's primary account, else the gateway. Returns `None` when nothing is
/// configured, and the caller keeps the org's Workbench roles untouched —
/// which is also what a disabled feature, a revoked allowlist entry and a
/// disabled account all produce, because a policy change should slow work down
/// rather than stop it.
pub async fn resolve(
    pg: &PgPool,
    agent_id: &str,
    task_id: Option<&str>,
) -> Result<Option<Resolved>, sqlx::Error> {
    if !enabled(pg).await {
        return Ok(None);
    }
    let permitted = permitted_services(pg).await;

    // The pin outranks the primary, and carries its own model for `default`.
    // A pin whose account is gone or disabled is treated as no pin at all, so
    // the ticket falls back to the agent's default rather than stalling.
    let pinned = match task_id {
        Some(t) => {
            sqlx::query(
                "select p.account_id, a.provider, a.email, p.model \
                 from task_coding_pins p \
                 left join agent_coding_accounts a \
                   on a.id = p.account_id and a.agent_id::text = $2 \
                      and a.disabled_at is null \
                 where p.task_id = $1::uuid \
                   and (p.account_id is null or a.id is not null)",
            )
            .bind(t)
            .bind(agent_id)
            .fetch_optional(pg)
            .await?
        }
        None => None,
    };

    let (plan, provider, email, source, pin_model) = match pinned {
        Some(r) => {
            let account_id: Option<i64> = r.get("account_id");
            let model: Option<String> = r.get("model");
            (
                Plan::from_account_id(account_id),
                r.get::<Option<String>, _>("provider")
                    .unwrap_or_else(|| GATEWAY_PROVIDER.to_string()),
                r.get::<Option<String>, _>("email"),
                "ticket",
                model,
            )
        }
        None => {
            let primary = sqlx::query(
                "select a.id, a.provider, a.email from agent_coding_accounts a \
                 where a.agent_id::text = $1 and a.is_primary and a.disabled_at is null",
            )
            .bind(agent_id)
            .fetch_optional(pg)
            .await?;
            match primary {
                Some(r) => (
                    Plan::Account(r.get("id")),
                    r.get("provider"),
                    r.get("email"),
                    "primary",
                    None,
                ),
                // No primary account is not "nothing configured": it is the
                // gateway, and the agent may still have gateway role picks.
                None => (
                    Plan::Gateway,
                    GATEWAY_PROVIDER.to_string(),
                    None,
                    "primary",
                    None,
                ),
            }
        }
    };

    // An account whose service left the allowlist stops driving jobs; the
    // gateway is never gated this way, because it is not a third-party plan.
    if let Plan::Account(_) = plan
        && !permitted.contains(&provider)
    {
        return Ok(None);
    }

    let picks = role_picks(pg, agent_id, plan).await?;
    let roles = resolve_roles(&picks, pin_model);
    if roles.is_empty() {
        // Signed in (or on the gateway) but no model named anywhere: nothing
        // to override with, so the org's Workbench roles stand rather than an
        // empty `--model`.
        return Ok(None);
    }
    Ok(Some(Resolved {
        plan,
        provider,
        email,
        source,
        roles,
    }))
}

/// Which agents hold at least one servable coding account — the fleet render's
/// question, asked once instead of per agent. Gateway-only agents are absent:
/// they need no broker and no new container env.
pub async fn agents_with_accounts(pg: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    if !enabled(pg).await {
        return Ok(Vec::new());
    }
    let permitted = permitted_services(pg).await;
    if permitted.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String,)> = sqlx::query_as(
        "select distinct agent_id::text from agent_coding_accounts \
         where disabled_at is null and provider = any($1)",
    )
    .bind(&permitted)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Hand an agent's coding work back to the gateway by default, without
/// signing anything out. The credentials stay, their role picks stay, and a
/// ticket can still pin one — this only moves what a job that pins nothing
/// runs on.
pub async fn use_gateway_by_default(pg: &PgPool, agent_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update agent_coding_accounts set is_primary = false, updated_at = now() \
         where agent_id::text = $1 and is_primary",
    )
    .bind(agent_id)
    .execute(pg)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roles_are_omps_four_in_ui_order() {
        assert_eq!(ROLES, ["default", "smol", "slow", "plan"]);
    }

    #[test]
    fn identity_reads_the_display_slice_off_a_credential() {
        let cred = json!({
            "type": "oauth",
            "access": "at",
            "refresh": "rt",
            "expires": 1_700_000_000_000i64,
            "email": "dev@example.com",
            "orgName": " Acme ",
            "authorizedAt": 1_699_000_000_000i64,
        });
        let id = Identity::from_credential(&cred, Some(" key "));
        assert_eq!(id.email.as_deref(), Some("dev@example.com"));
        assert_eq!(id.org_name.as_deref(), Some("Acme"), "trimmed");
        assert_eq!(id.identity_key.as_deref(), Some("key"));
        assert_eq!(id.expires, Some(1_700_000_000_000));
        assert_eq!(id.authorized_at, Some(1_699_000_000_000));
        assert!(id.account_id.is_none(), "absent stays absent");
    }

    #[test]
    fn a_never_expires_sentinel_reads_as_no_expiry() {
        // omp's NEVER_EXPIRES is far past any real date; storing it would
        // render a nonsense expiry in the UI.
        let cred = json!({ "expires": 8_640_000_000_000_000i64 });
        assert!(Identity::from_credential(&cred, None).expires.is_none());
        let cred = json!({ "expires": 0 });
        assert!(Identity::from_credential(&cred, None).expires.is_none());
    }

    #[test]
    fn a_resolved_plan_names_models_the_omp_way() {
        let r = Resolved {
            plan: Plan::Account(7),
            provider: "anthropic".into(),
            email: None,
            source: "ticket",
            roles: vec![
                ("default".into(), "claude-opus-4-5".into()),
                ("smol".into(), "claude-haiku-4-5".into()),
            ],
        };
        assert_eq!(
            r.model_arg("default").as_deref(),
            Some("anthropic/claude-opus-4-5")
        );
        assert_eq!(
            r.model_arg("smol").as_deref(),
            Some("anthropic/claude-haiku-4-5")
        );
        assert!(r.model_arg("plan").is_none(), "an unfilled role falls back");
    }

    #[test]
    fn the_gateway_is_a_plan_and_spells_models_the_way_it_always_did() {
        let r = Resolved {
            plan: Plan::Gateway,
            provider: GATEWAY_PROVIDER.into(),
            email: None,
            source: "primary",
            roles: vec![("default".into(), "pl-main".into())],
        };
        assert!(r.is_gateway());
        assert_eq!(r.model_arg("default").as_deref(), Some("talaria/pl-main"));
    }

    #[test]
    fn a_plan_round_trips_through_its_wire_spelling() {
        assert_eq!(Plan::from_account_id(None), Plan::Gateway);
        assert_eq!(Plan::from_account_id(Some(3)), Plan::Account(3));
        assert_eq!(
            Plan::Gateway.account_id(),
            None,
            "the gateway names no account"
        );
        assert_eq!(Plan::Account(3).account_id(), Some(3));
    }

    #[test]
    fn unfilled_roles_inherit_the_plans_default() {
        let picks = vec![("default".to_string(), "opus".to_string())];
        let roles = resolve_roles(&picks, None);
        assert_eq!(
            roles,
            vec![
                ("default".to_string(), "opus".to_string()),
                ("smol".to_string(), "opus".to_string()),
                ("slow".to_string(), "opus".to_string()),
                ("plan".to_string(), "opus".to_string()),
            ],
            "a plan that named only a default still takes the whole job"
        );
    }

    #[test]
    fn the_tickets_model_replaces_only_the_default_role() {
        let picks = vec![
            ("default".to_string(), "opus".to_string()),
            ("smol".to_string(), "haiku".to_string()),
        ];
        let roles = resolve_roles(&picks, Some("sonnet".to_string()));
        assert_eq!(roles[0], ("default".to_string(), "sonnet".to_string()));
        assert_eq!(
            roles[1],
            ("smol".to_string(), "haiku".to_string()),
            "the account's own smol pick survives the ticket's model"
        );
        assert_eq!(
            roles[2],
            ("slow".to_string(), "sonnet".to_string()),
            "and an unfilled role inherits the ticket's model, not the old default"
        );
    }

    #[test]
    fn an_account_that_names_nothing_resolves_to_nothing() {
        assert!(
            resolve_roles(&[], None).is_empty(),
            "nothing to override with leaves the org's gateway roles standing"
        );
    }

    #[test]
    fn a_pinned_model_alone_is_enough_to_move_a_job() {
        let roles = resolve_roles(&[], Some("gpt-5.6-sol".to_string()));
        assert_eq!(roles.len(), 4, "every role takes the pinned model");
        assert!(roles.iter().all(|(_, m)| m == "gpt-5.6-sol"));
    }

    #[test]
    fn an_empty_identity_string_is_not_an_identity() {
        let cred = json!({ "email": "", "accountId": "   " });
        let id = Identity::from_credential(&cred, Some("  "));
        assert!(id.email.is_none());
        assert!(id.account_id.is_none());
        assert!(id.identity_key.is_none());
    }
}
