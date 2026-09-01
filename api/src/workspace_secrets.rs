// THE CREDENTIALS AN AGENT MAY USE WITHOUT EVER READING — port of
// ui/src/server/workspace-secrets.ts. `secret-vault.ts` stops a credential in
// context from reaching a provider; this is the other half: a way for an agent
// to USE one anyway — push with a PAT, pull from a private registry — while the
// value stays on this side of the wire. The agent is given a NAME and the
// platform substitutes the VALUE at the boundary that spends it.
//
// A DOC, NOT A ROW, because that is how credentials arrive: a deploy needs a
// PAT, a registry password and a signing key together. TWO LIFETIMES: a `vault`
// doc persists; a `relay` is one-shot, consumed on first resolve — which is the
// whole reason `uses_remaining` exists.
//
// NOT CARRIED HERE (they cross with the surfaces that call them):
// `spendHandlesInToolCall` and the handle briefing/soul strings ride with the
// mcp and agent-plane families; `handlesHeldBy` with the admin tail.
//
// WHAT THIS DELIBERATELY DOES NOT DO: hand a value to anything that will show
// it to a model. `resolve_handles` is for OUTBOUND boundaries, and its result
// must never be written back into a transcript, a tool result, or a record.

use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::Row;

use crate::agent_auth::epoch_ms_to_iso;
use crate::audit::{AuditEntry, log_audit};
use crate::secretbox::SecretBox;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// How long an unspent relay lives. An hour is long enough for a queued turn
/// and short enough that a changed mind costs nothing; anything that needs to
/// outlive the conversation is a vault doc and a deliberate trip through the
/// admin panel.
pub const RELAY_TTL_MS: i64 = 60 * 60 * 1000;

/// `«secret:doc»` or `«secret:doc.entry»`. The doc-only form resolves when the
/// doc holds exactly one entry, which is what makes a single secret feel like a
/// single secret while sharing the bundle's machinery.
const HANDLE: &str = r#"«secret:([a-z0-9][a-z0-9_-]*)(?:\.([a-z0-9][a-z0-9_-]*))?»"#;

fn handle_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!("(?i){HANDLE}")).expect("handle regex"))
}

pub fn handle_for(doc: &str, entry: Option<&str>) -> String {
    match entry {
        Some(e) => format!("«secret:{doc}.{e}»"),
        None => format!("«secret:{doc}»"),
    }
}

/// Does this text name a handle at all? Cheap enough to ask on every turn.
pub fn mentions_handle(text: &str) -> bool {
    handle_re().is_match(text)
}

pub struct SecretEntryInput {
    pub key: String,
    /// What KIND of credential — shown to humans, never derived from the value.
    pub label: String,
    pub value: String,
}

/// The listing wire shape — KEYS AND LABELS ONLY. There is no field here that
/// carries a value: every listing surface and log line is built from this, so
/// the value has nowhere to escape to by accident. Field order is the TS
/// interface's, which is the SQL select order plus the appended collections.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretDoc {
    pub id: String,
    pub name: String,
    pub title: String,
    pub kind: String,
    pub note: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub uses_remaining: Option<i64>,
    pub last_used_at: Option<String>,
    /// Hosts this credential may be spent against. Empty = unrestricted, which
    /// is what every secret predating the check has.
    pub allowed_hosts: Vec<String>,
    /// May a PERSON read this back? False for every agent credential, and no
    /// path flips it after creation.
    pub revealable: bool,
    /// Whose working secret this is. Null for workspace credentials.
    pub owner_user_id: Option<String>,
    /// Which SECRET folder it sits in. Null = loose.
    pub folder_id: Option<String>,
    pub entries: Vec<EntryLabel>,
    pub grants: Vec<String>,
    pub readers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryLabel {
    pub key: String,
    pub label: String,
}

// Row decode — the house plain-cell helpers (uuid columns ::text, timestamps
// read as truncated epoch ms the way the TS driver's Date does).
fn cell_str(r: &sqlx::postgres::PgRow, col: &str) -> String {
    r.try_get(col).unwrap_or_default()
}
fn cell_opt_str(r: &sqlx::postgres::PgRow, col: &str) -> Option<String> {
    r.try_get(col).unwrap_or(None)
}
fn cell_bool(r: &sqlx::postgres::PgRow, col: &str) -> bool {
    r.try_get(col).unwrap_or(false)
}
fn cell_opt_i64(r: &sqlx::postgres::PgRow, col: &str) -> Option<i64> {
    r.try_get(col).unwrap_or(None)
}
fn cell_hosts(r: &sqlx::postgres::PgRow, col: &str) -> Vec<String> {
    r.try_get(col).unwrap_or_default()
}

// ── WHERE IS THIS CREDENTIAL ABOUT TO GO? ────────────────────────────────────
//
// The substitution is blind to destination unless something reads one out of
// the text being substituted into — which is arbitrary tool arguments.
// Deliberately narrow, because the cost of a false POSITIVE is a refused
// legitimate call: URLs and git/ssh remotes where the host is unambiguous, and
// a BARE token standing alone whose last label is a plausible TLD and not a
// file extension. What is NOT extractable is handled by refusing — see
// `resolve_handles`.
const URL_HOST: &str =
    r#"(?:https?|ssh|git)://(?:[^@/\s]*@)?([a-z0-9][a-z0-9.-]*[a-z0-9])(?::\d+)?"#;
const SCP_HOST: &str = r#"(?:^|[\s"'`])(?:[a-z0-9_.-]+@)([a-z0-9][a-z0-9.-]*[a-z0-9]):"#;
// TS carries a lookahead `(?=[\s"'`/:,]|$)` here, which the regex crate cannot
// express. The candidates are found without it and verified by hand below —
// equivalent because every char a candidate spans past its prefix is
// `[a-z0-9.-]`, none of which can start a new match, so no match is lost by
// continuing the scan from the candidate's end.
const BARE_HOST: &str = r#"(?:^|[\s"'`=])((?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,24})"#;

fn url_host_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!("(?i){URL_HOST}")).expect("url host regex"))
}
fn scp_host_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!("(?i){SCP_HOST}")).expect("scp host regex"))
}
fn bare_host_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!("(?i){BARE_HOST}")).expect("bare host regex"))
}

/// Extensions a bare `something.ext` token is far likelier to be than a host.
/// Not exhaustive and does not need to be: everything it misses fails CLOSED.
fn file_ext_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)^(?:json|jsonc|ts|tsx|js|jsx|mjs|cjs|md|mdx|ya?ml|txt|lock|toml|sh|bash|zsh|py|rb|go|rs|java|c|h|cpp|env|gz|tgz|tar|zip|png|jpe?g|gif|svg|ico|html?|css|scss|sql|xml|csv|tsv|log|cfg|ini|conf|pem|key|crt|pdf|so|dll|exe)$",
        )
        .expect("file ext regex")
    })
}

/// Every host this text would send something to, lowercased and deduped.
pub fn hosts_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = text.to_lowercase();
    let push = |h: &str, out: &mut Vec<String>| {
        if !h.is_empty() && !out.iter().any(|x| x == h) {
            out.push(h.to_string());
        }
    };
    for re in [url_host_re(), scp_host_re()] {
        for caps in re.captures_iter(&lower) {
            if let Some(m) = caps.get(1) {
                push(m.as_str(), &mut out);
            }
        }
    }
    // The hand-rolled half of BARE_HOST's lookahead: the char after the
    // candidate must be one the JS pattern would have accepted.
    for caps in bare_host_re().captures_iter(&lower) {
        let Some(m) = caps.get(1) else { continue };
        let host = m.as_str();
        let after = &lower[m.end()..];
        let boundary = after.is_empty()
            || after.chars().next().is_some_and(|c| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '/' | ':' | ',')
            });
        if !boundary {
            continue;
        }
        let ext = host.rsplit('.').next().unwrap_or_default();
        if file_ext_re().is_match(ext) {
            continue;
        }
        push(host, &mut out);
    }
    out
}

/// Is `host` covered by an allowlist entry? Exact, or a subdomain of one —
/// `github.com` covers `api.github.com` but never `github.com.evil.net`, which
/// is why this is not `ends_with` alone.
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|a| {
        let entry = a.trim().to_lowercase();
        let entry = entry.trim_start_matches('.');
        !entry.is_empty() && (host == entry || host.ends_with(&format!(".{entry}")))
    })
}

/// Why a handle did not resolve — reported to the operator, never the model: a
/// caller that learns which names exist has been handed a map of the
/// workspace's credentials.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedHandle {
    pub handle: String,
    pub reason: &'static str,
    /// For `destination`: where it was about to be spent. Named so an operator
    /// reading the log learns which host to add — or which attack just failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsedEntry {
    pub name: String,
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Resolution {
    pub text: String,
    /// Docs actually spent, for the audit line. Names, never values.
    pub used: Vec<UsedEntry>,
    pub unresolved: Vec<UnresolvedHandle>,
    /// Every destination this text would reach, recorded on the spend whether
    /// or not the secret restricts them — "where did our deploy key go" is a
    /// question an operator asks AFTER something happened.
    pub hosts: Vec<String>,
}

/// SUBSTITUTE HANDLES FOR VALUES, at an outbound boundary and nowhere else.
/// `caller` is the agent model asking; a handle it has not been granted does
/// not resolve, and the difference between "no such secret" and "not yours"
/// goes to the operator, not the model.
///
/// A RELAY IS SPENT HERE: `uses_remaining` decrements atomically in the same
/// statement that reads it, so two concurrent tool calls cannot both spend the
/// last use of a one-shot. The destination check runs BEFORE the spend — a
/// refusal must not consume a use; an attacker who could burn a relay by naming
/// a bad host would have a denial-of-service on every credential in the
/// workspace. And when a non-empty allowlist cannot see ANY host, that refuses
/// too: an unverifiable destination is not a verified one.
pub async fn resolve_handles(
    pg: &PgPool,
    sb: &SecretBox,
    text: &str,
    caller: &str,
) -> Result<Resolution, String> {
    let found: Vec<(String, String, Option<String>)> = handle_re()
        .captures_iter(text)
        .map(|c| {
            (
                c.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
                c.get(1)
                    .map(|m| m.as_str().to_lowercase())
                    .unwrap_or_default(),
                c.get(2).map(|m| m.as_str().to_lowercase()),
            )
        })
        .collect();
    if found.is_empty() {
        return Ok(Resolution {
            text: text.to_string(),
            used: Vec::new(),
            unresolved: Vec::new(),
            hosts: Vec::new(),
        });
    }

    let mut used: Vec<UsedEntry> = Vec::new();
    let mut unresolved: Vec<UnresolvedHandle> = Vec::new();
    // Read ONCE, off the text the model wrote — before any substitution, so a
    // credential that happens to contain a dot cannot invent a destination.
    let hosts = hosts_in(text);
    let mut out = text.to_string();

    // `granted` is the count of the caller's DIRECT grant plus its grant
    // through the folder the doc sits in — resolved at read time so a
    // credential added to a shared folder next week is covered without anybody
    // re-sharing it, which is the step everybody forgets.
    let resolve_sql = "select s.id::text, s.name, s.kind, \
        (trunc(extract(epoch from s.expires_at) * 1000))::bigint as expires_ms, \
        s.uses_remaining::int8 as uses_remaining, \
        coalesce(s.allowed_hosts, '{}') as allowed_hosts, \
        ((select count(*) from workspace_secret_grants g where g.secret_id = s.id and g.agent_model = $1) \
         + (select count(*) from secret_folder_grants fg where fg.folder_id = s.secret_folder_id and fg.agent_model = $1)) as granted \
       from workspace_secrets s where lower(s.name) = $2";

    for (handle, doc_name, entry_key) in found {
        let rows = sqlx::query(resolve_sql)
            .bind(caller)
            .bind(&doc_name)
            .fetch_all(pg)
            .await
            .map_err(|e| format!("secrets resolve read: {e}"))?;
        let Some(doc) = rows.first() else {
            unresolved.push(UnresolvedHandle {
                handle,
                reason: "unknown",
                host: None,
            });
            continue;
        };
        let doc_id = cell_str(doc, "id");
        let doc_name = cell_str(doc, "name");
        let doc_kind = cell_str(doc, "kind");
        if cell_opt_i64(doc, "granted").unwrap_or(0) == 0 {
            unresolved.push(UnresolvedHandle {
                handle,
                reason: "not-granted",
                host: None,
            });
            continue;
        }
        // TS: `new Date(expiresAt).getTime() <= Date.now()` — an unparseable
        // value is NaN and therefore NOT expired; None here plays that role.
        if cell_opt_i64(doc, "expires_ms").is_some_and(|t| t <= now_ms()) {
            unresolved.push(UnresolvedHandle {
                handle,
                reason: "expired",
                host: None,
            });
            continue;
        }

        let allowed = cell_hosts(doc, "allowed_hosts");
        if !allowed.is_empty() {
            if let Some(bad) = hosts.iter().find(|h| !host_allowed(h, &allowed)) {
                unresolved.push(UnresolvedHandle {
                    handle,
                    reason: "destination",
                    host: Some(bad.clone()),
                });
                continue;
            }
            if hosts.is_empty() {
                unresolved.push(UnresolvedHandle {
                    handle,
                    reason: "destination",
                    host: None,
                });
                continue;
            }
        }

        let entries = cipher_entries(pg, &doc_id).await?;
        // The doc-only form is only unambiguous when there is one entry.
        // Guessing which of four credentials was meant is how the wrong one
        // gets spent.
        let entry = match &entry_key {
            Some(k) => entries.iter().find(|e| e.key.to_lowercase() == *k),
            None => {
                if entries.len() == 1 {
                    entries.first()
                } else {
                    None
                }
            }
        };
        let Some(entry) = entry else {
            unresolved.push(UnresolvedHandle {
                handle,
                reason: if entry_key.is_some() {
                    "unknown"
                } else {
                    "ambiguous"
                },
                host: None,
            });
            continue;
        };
        let uses_remaining = cell_opt_i64(doc, "uses_remaining");

        // SPEND IT IN THE SAME STATEMENT THAT CHECKS IT.
        let exhausted;
        if uses_remaining.is_some() {
            let spent = sqlx::query(
                "update workspace_secrets set uses_remaining = uses_remaining - 1, last_used_at = now() \
                  where id = $1::uuid and uses_remaining > 0 returning id::text",
            )
            .bind(&doc_id)
            .fetch_all(pg)
            .await
            .map_err(|e| format!("secrets spend: {e}"))?;
            if spent.is_empty() {
                unresolved.push(UnresolvedHandle {
                    handle,
                    reason: "spent",
                    host: None,
                });
                continue;
            }
            exhausted = uses_remaining.unwrap_or(0) - 1 <= 0;
        } else {
            sqlx::query("update workspace_secrets set last_used_at = now() where id = $1::uuid")
                .bind(&doc_id)
                .execute(pg)
                .await
                .map_err(|e| format!("secrets touch: {e}"))?;
            exhausted = false;
        }

        // Key material changed under us: refusing is the only safe answer.
        let value = match sb.open(&entry.cipher) {
            Ok(v) => v,
            Err(_) => {
                unresolved.push(UnresolvedHandle {
                    handle,
                    reason: "unknown",
                    host: None,
                });
                continue;
            }
        };
        out = out.replace(&handle, &value);

        // A credential with no uses left is DESTROYED, not retained — emptied
        // rather than deleted so the audit trail survives with the row. A
        // one-shot somebody pasted into chat this morning should not still be
        // recoverable from a database dump tonight.
        if exhausted {
            sqlx::query(
                "update workspace_secret_entries set value_cipher = '' where secret_id = $1::uuid",
            )
            .bind(&doc_id)
            .execute(pg)
            .await
            .map_err(|e| format!("secrets destroy: {e}"))?;
        }
        used.push(UsedEntry {
            name: doc_name.clone(),
            key: entry.key.clone(),
            label: entry.label.clone(),
        });

        // THE ONE EVENT WORTH AUDITING — the spend is the only moment a
        // credential actually MOVES, and it is recorded here rather than at the
        // call sites so the boundaries cannot drift. Names, kinds and
        // destinations; never a value, and never the text it was substituted
        // into, which by definition now contains one.
        audit_spend(
            pg,
            caller,
            &doc_name,
            &entry.label,
            serde_json::json!({
                "key": entry.key,
                "hosts": hosts,
                "kind": doc_kind,
                "exhausted": exhausted,
                "restricted": !allowed.is_empty(),
            }),
        );
    }

    Ok(Resolution {
        text: out,
        used,
        unresolved,
        hosts,
    })
}

/// One entry row with its cipher — shared by the resolve and git-credential
/// paths, both of which need the sealed value in place.
struct CipherEntry {
    key: String,
    label: String,
    cipher: String,
}

async fn cipher_entries(pg: &PgPool, secret_id: &str) -> Result<Vec<CipherEntry>, String> {
    let rows = sqlx::query(
        "select key, label, value_cipher as cipher from workspace_secret_entries \
          where secret_id = $1::uuid order by key",
    )
    .bind(secret_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets entries read: {e}"))?;
    Ok(rows
        .iter()
        .map(|r| CipherEntry {
            key: cell_str(r, "key"),
            label: cell_str(r, "label"),
            cipher: cell_str(r, "cipher"),
        })
        .collect())
}

/// The `secrets.spend` audit row, fire-and-forget like every audit write. The
/// `after` JSON is built by the caller because the two boundaries record
/// different shapes — the substitution names the kind and the burn, the git
/// helper names `via`.
fn audit_spend(pg: &PgPool, actor: &str, name: &str, label: &str, after: serde_json::Value) {
    let pg = pg.clone();
    let actor = actor.to_string();
    let name = name.to_string();
    let label = label.to_string();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "secrets.spend",
                target_type: "secret",
                target_id: Some(&name),
                target_label: Some(&label),
                before: None,
                after: Some(after),
            },
        )
        .await;
    });
}

// ── The tool-call spend boundary ─────────────────────────────────────────────

/// The audit's view of one spent entry — `used` in the gateway's spend report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpendEntry {
    pub name: String,
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SpendOutcome {
    /// Whether `rpc.params.arguments` was rewritten in place with the
    /// substituted values.
    pub changed: bool,
    pub used: Vec<SpendEntry>,
    pub unresolved: Vec<UnresolvedHandle>,
}

/// Resolve handle mentions INSIDE a gateway `tools/call` arguments object,
/// rewriting `rpc.params.arguments` in place (spendHandlesInToolCall). The
/// gateway's one spend boundary: a credential is spent exactly when it rides
/// an outbound tool call, and the report (`used`, names never values) is what
/// the relay route logs.
///
/// Non-`tools/call` rpcs and rpcs without an `arguments` object are returned
/// untouched — a notification or a tools/list cannot spend anything. A value
/// that breaks the JSON round trip forwards the call UNRESOLVED (the tool will
/// refuse the raw handle) but still reports the spend, because it happened.
pub async fn spend_handles_in_tool_call(
    pg: &PgPool,
    sb: &SecretBox,
    rpc: &mut Value,
    caller: &str,
) -> Result<SpendOutcome, String> {
    if rpc.get("method").and_then(Value::as_str) != Some("tools/call") {
        return Ok(SpendOutcome::default());
    }
    // `!rpc.params?.arguments` — JS truthiness: absent, null, false, 0 and ""
    // don't spend; anything else (object, array, non-empty string) does.
    let js_truthy = |v: &Value| match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        _ => true,
    };
    let Some(arguments) = rpc
        .get_mut("params")
        .and_then(|p| p.get_mut("arguments"))
        .filter(|a| js_truthy(a))
    else {
        return Ok(SpendOutcome::default());
    };
    let payload = arguments.to_string();
    let resolved = resolve_handles(pg, sb, &payload, caller).await?;
    let used = resolved
        .used
        .iter()
        .map(|u| SpendEntry {
            name: u.name.clone(),
            key: u.key.clone(),
            label: u.label.clone(),
        })
        .collect();
    if resolved.used.is_empty() {
        return Ok(SpendOutcome {
            changed: false,
            used,
            unresolved: resolved.unresolved,
        });
    }
    // `rpc.params.arguments = JSON.parse(resolved.text)` — the assignment is
    // whatever parses back, not necessarily an object.
    match serde_json::from_str::<Value>(&resolved.text) {
        Ok(next) => {
            *arguments = next;
            Ok(SpendOutcome {
                changed: true,
                used,
                unresolved: resolved.unresolved,
            })
        }
        Err(_) => Ok(SpendOutcome {
            changed: false,
            used,
            unresolved: resolved.unresolved,
        }),
    }
}

// ── WORKING SECRETS: read, share, unshare ────────────────────────────────────
//
// THE ONE PLACE A VALUE COMES BACK. Everything else in this file moves a
// credential OUTWARD; `reveal_entry` does the opposite, so every guard it
// carries is load-bearing rather than decorative.

/// BEING AN ADMIN IS NOT ENOUGH. Reveal is a grant, not a role: an admin can
/// see that a working secret exists and can delete it, but reading somebody's
/// staging key because you administer the workspace is the behaviour that
/// sends people back to pasting keys into Slack.
pub struct RevealOutcome {
    pub value: Option<String>,
    /// The refusal noun; empty exactly when `value` is set.
    pub refusal: &'static str,
}

impl RevealOutcome {
    fn refused(reason: &'static str) -> Self {
        RevealOutcome {
            value: None,
            refusal: reason,
        }
    }
}

/// REVEAL ONE ENTRY, for one person, once. Four things have to be true —
/// exists, `revealable`, owner-or-shared, unexpired — and `revealable` is the
/// one that matters: there is no route, permission or admin override that
/// flips it. ONE ENTRY AT A TIME, by key, so a bundle cannot be drained by one
/// call and the audit line names the credential actually looked at.
pub async fn reveal_entry(
    pg: &PgPool,
    sb: &SecretBox,
    name: &str,
    entry_key: &str,
    user_id: &str,
    actor_label: Option<&str>,
) -> Result<RevealOutcome, String> {
    // `shared` is the same union as the agent side: shared with them directly,
    // or through the folder.
    let rows = sqlx::query(
        "select s.id::text, s.name, s.title, s.revealable, \
                (trunc(extract(epoch from s.expires_at) * 1000))::bigint as expires_ms, \
                s.owner_user_id::text as owner_user_id, \
                ((select count(*) from workspace_secret_readers r where r.secret_id = s.id and r.user_id = $1::uuid) \
                 + (select count(*) from secret_folder_readers fr where fr.folder_id = s.secret_folder_id and fr.user_id = $1::uuid)) as shared \
           from workspace_secrets s where s.name = $2",
    )
    .bind(user_id)
    .bind(name)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets reveal read: {e}"))?;
    let Some(doc) = rows.first() else {
        return Ok(RevealOutcome::refused("unknown"));
    };
    // THE GUARANTEE, ENFORCED FIRST — an agent credential is not readable by
    // anybody, including whoever created it.
    if !cell_bool(doc, "revealable") {
        return Ok(RevealOutcome::refused("not-revealable"));
    }
    if cell_opt_str(doc, "owner_user_id").as_deref() != Some(user_id)
        && cell_opt_i64(doc, "shared").unwrap_or(0) == 0
    {
        return Ok(RevealOutcome::refused("not-shared"));
    }
    if cell_opt_i64(doc, "expires_ms").is_some_and(|t| t <= now_ms()) {
        return Ok(RevealOutcome::refused("expired"));
    }

    let entries = sqlx::query(
        "select key, label, value_cipher as cipher from workspace_secret_entries \
          where secret_id = $1::uuid and key = $2",
    )
    .bind(cell_str(doc, "id"))
    .bind(entry_key)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets reveal entry read: {e}"))?;
    let Some(entry) = entries.first() else {
        return Ok(RevealOutcome::refused("no-such-entry"));
    };
    // A spent one-shot has had its ciphertext emptied. Say so plainly rather
    // than handing back an empty string that reads as a credential.
    let cipher = cell_str(entry, "cipher");
    if cipher.is_empty() {
        return Ok(RevealOutcome::refused("destroyed"));
    }
    let value = match sb.open(&cipher) {
        Ok(v) => v,
        Err(_) => return Ok(RevealOutcome::refused("destroyed")),
    };

    // EVERY LOOK IS WRITTEN DOWN — that seeing it leaves a mark is the whole
    // difference between a shared vault and a key in a Slack thread. Names and
    // labels; the value is the one thing that must never be in the row
    // recording that somebody read the value. The actor is the LABEL falling
    // back to the id: access is decided by `user_id` and nothing else.
    let actor = actor_label
        .map(|a| a.to_string())
        .unwrap_or_else(|| user_id.to_string());
    let after =
        serde_json::json!({ "key": cell_str(entry, "key"), "title": cell_str(doc, "title") });
    let pg_audit = pg.clone();
    let target_id = cell_str(doc, "name");
    let target_label = cell_str(entry, "label");
    tokio::spawn(async move {
        log_audit(
            &pg_audit,
            AuditEntry {
                actor: &actor,
                action: "secrets.reveal",
                target_type: "secret",
                target_id: Some(&target_id),
                target_label: Some(&target_label),
                before: None,
                after: Some(after),
            },
        )
        .await;
    });
    sqlx::query("update workspace_secrets set last_used_at = now() where id = $1::uuid")
        .bind(cell_str(doc, "id"))
        .execute(pg)
        .await
        .map_err(|e| format!("secrets reveal touch: {e}"))?;
    Ok(RevealOutcome {
        value: Some(value),
        refusal: "",
    })
}

/// Let somebody else read it. Owner-only — a reader cannot widen the circle
/// they were let into, which is the difference between sharing and forwarding.
pub async fn share_secret_with(
    pg: &PgPool,
    name: &str,
    user_id: &str,
    acting_user_id: &str,
) -> Result<bool, String> {
    let rows = sqlx::query(
        "select id::text, owner_user_id::text as owner_user_id, revealable \
           from workspace_secrets where name = $1",
    )
    .bind(name)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets share read: {e}"))?;
    let Some(doc) = rows.first() else {
        return Ok(false);
    };
    if !cell_bool(doc, "revealable")
        || cell_opt_str(doc, "owner_user_id").as_deref() != Some(acting_user_id)
    {
        return Ok(false);
    }
    sqlx::query(
        "insert into workspace_secret_readers (secret_id, user_id, granted_by) \
         values ($1::uuid, $2::uuid, $3) on conflict do nothing",
    )
    .bind(cell_str(doc, "id"))
    .bind(user_id)
    .bind(acting_user_id)
    .execute(pg)
    .await
    .map_err(|e| format!("secrets share write: {e}"))?;
    Ok(true)
}

pub async fn unshare_secret_from(
    pg: &PgPool,
    name: &str,
    user_id: &str,
    acting_user_id: &str,
) -> Result<bool, String> {
    let rows = sqlx::query(
        "select id::text, owner_user_id::text as owner_user_id \
           from workspace_secrets where name = $1",
    )
    .bind(name)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets unshare read: {e}"))?;
    let Some(doc) = rows.first() else {
        return Ok(false);
    };
    if cell_opt_str(doc, "owner_user_id").as_deref() != Some(acting_user_id) {
        return Ok(false);
    }
    sqlx::query(
        "delete from workspace_secret_readers where secret_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(cell_str(doc, "id"))
    .bind(user_id)
    .execute(pg)
    .await
    .map_err(|e| format!("secrets unshare write: {e}"))?;
    Ok(true)
}

pub async fn grant_secret(
    pg: &PgPool,
    name: &str,
    agent_model: &str,
    granted_by: Option<&str>,
) -> Result<(), String> {
    sqlx::query(
        "insert into workspace_secret_grants (secret_id, agent_model, granted_by) \
         select id, $2, $3 from workspace_secrets where name = $1 on conflict do nothing",
    )
    .bind(name)
    .bind(agent_model)
    .bind(granted_by)
    .execute(pg)
    .await
    .map_err(|e| format!("secrets grant: {e}"))?;
    Ok(())
}

pub async fn revoke_secret(pg: &PgPool, name: &str, agent_model: &str) -> Result<(), String> {
    sqlx::query(
        "delete from workspace_secret_grants \
          where agent_model = $2 \
            and secret_id in (select id from workspace_secrets where name = $1)",
    )
    .bind(name)
    .bind(agent_model)
    .execute(pg)
    .await
    .map_err(|e| format!("secrets revoke: {e}"))?;
    Ok(())
}

pub async fn delete_secret_doc(pg: &PgPool, name: &str) -> Result<(), String> {
    sqlx::query("delete from workspace_secrets where name = $1")
        .bind(name)
        .execute(pg)
        .await
        .map_err(|e| format!("secrets delete: {e}"))?;
    Ok(())
}

// ── THE DOC READ/WRITE HALF ──────────────────────────────────────────────────

/// One doc, keys and labels only.
pub async fn get_secret_doc(pg: &PgPool, name: &str) -> Result<Option<SecretDoc>, String> {
    let rows = sqlx::query(
        "select id::text, name, title, kind, note, created_by, \
                (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
                (trunc(extract(epoch from expires_at) * 1000))::bigint as expires_ms, \
                uses_remaining::int8 as uses_remaining, \
                (trunc(extract(epoch from last_used_at) * 1000))::bigint as last_used_ms, \
                coalesce(allowed_hosts, '{}') as allowed_hosts, \
                revealable, owner_user_id::text as owner_user_id, \
                secret_folder_id::text as folder_id \
           from workspace_secrets where name = $1",
    )
    .bind(name)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets doc read: {e}"))?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let id = cell_str(row, "id");
    let entries = sqlx::query(
        "select key, label from workspace_secret_entries where secret_id = $1::uuid order by key",
    )
    .bind(&id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets entries read: {e}"))?;
    let grants =
        sqlx::query("select agent_model from workspace_secret_grants where secret_id = $1::uuid")
            .bind(&id)
            .fetch_all(pg)
            .await
            .map_err(|e| format!("secrets grants read: {e}"))?;
    let readers = sqlx::query(
        "select user_id::text from workspace_secret_readers where secret_id = $1::uuid",
    )
    .bind(&id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets readers read: {e}"))?;
    Ok(Some(SecretDoc {
        id,
        name: cell_str(row, "name"),
        title: cell_str(row, "title"),
        kind: cell_str(row, "kind"),
        note: cell_opt_str(row, "note"),
        created_by: cell_opt_str(row, "created_by"),
        created_at: epoch_ms_to_iso(cell_opt_i64(row, "created_ms").unwrap_or(0)),
        expires_at: cell_opt_i64(row, "expires_ms").map(epoch_ms_to_iso),
        uses_remaining: cell_opt_i64(row, "uses_remaining"),
        last_used_at: cell_opt_i64(row, "last_used_ms").map(epoch_ms_to_iso),
        allowed_hosts: cell_hosts(row, "allowed_hosts"),
        revealable: cell_bool(row, "revealable"),
        owner_user_id: cell_opt_str(row, "owner_user_id"),
        folder_id: cell_opt_str(row, "folder_id"),
        entries: entries
            .iter()
            .map(|r| EntryLabel {
                key: cell_str(r, "key"),
                label: cell_str(r, "label"),
            })
            .collect(),
        grants: grants.iter().map(|r| cell_str(r, "agent_model")).collect(),
        readers: readers.iter().map(|r| cell_str(r, "user_id")).collect(),
    }))
}

/// Every doc, newest first (listSecretDocs) — the admin console's inventory.
/// SecretDoc already serializes in the TS wire shape; this is the same data
/// the reading routes serve, so console and agent see one truth.
pub async fn list_secret_docs(pg: &PgPool) -> Result<Vec<SecretDoc>, String> {
    let names: Vec<(String,)> =
        sqlx::query_as("select name from workspace_secrets order by created_at desc")
            .fetch_all(pg)
            .await
            .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(names.len());
    for (name,) in names {
        if let Some(doc) = get_secret_doc(pg, &name).await? {
            out.push(doc);
        }
    }
    Ok(out)
}

/// Which handles does one agent hold, and how (handlesHeldBy) — direct
/// grants plus folder grants, gated on expiry and remaining uses. The
/// admin console's per-agent security view.
pub async fn handles_held_by(pg: &PgPool, agent_model: &str) -> Result<Vec<Value>, String> {
    let rows = sqlx::query(
        "select s.name, s.title, e.key, e.label, \
                (g.agent_model is not null) as \"direct\", \
                f.name as folder \
           from workspace_secrets s \
           join workspace_secret_entries e on e.secret_id = s.id \
           left join workspace_secret_grants g \
                  on g.secret_id = s.id and g.agent_model = $1 \
           left join secret_folder_grants fg \
                  on fg.folder_id = s.secret_folder_id and fg.agent_model = $1 \
           left join secret_folders f on f.id = s.secret_folder_id \
          where (g.agent_model is not null or fg.agent_model is not null) \
            and (s.expires_at is null or s.expires_at > now()) \
            and (s.uses_remaining is null or s.uses_remaining > 0) \
          order by s.title, e.key",
    )
    .bind(agent_model)
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": cell_str(r, "name"),
                "title": cell_str(r, "title"),
                "key": cell_str(r, "key"),
                "label": cell_str(r, "label"),
                "via": if cell_bool(r, "direct") { "direct" } else { "folder" },
                "folder": cell_opt_str(r, "folder"),
            })
        })
        .collect())
}

/// Create a doc. `relay` docs default to a single use, which is what makes
/// them relays rather than secrets somebody forgot to delete.
pub struct CreateSecret {
    pub name: String,
    pub title: String,
    pub entries: Vec<SecretEntryInput>,
    pub kind: Option<&'static str>,
    pub note: Option<String>,
    pub created_by: Option<String>,
    pub expires_at: Option<String>,
    pub uses: Option<i64>,
    pub grant_to: Option<Vec<String>>,
    pub allowed_hosts: Option<Vec<String>>,
    /// TRUE makes it a WORKING secret — a person can read it back. False keeps
    /// every agent credential unreadable forever.
    pub revealable: bool,
    pub owner_user_id: Option<String>,
    pub folder_id: Option<String>,
    pub readers: Option<Vec<String>>,
}

pub async fn create_secret_doc(
    pg: &PgPool,
    sb: &SecretBox,
    input: &CreateSecret,
) -> Result<SecretDoc, String> {
    if input.entries.is_empty() {
        return Err("a secret needs at least one entry".to_string());
    }
    let kind = input.kind.unwrap_or("vault");
    let uses = match input.uses {
        Some(u) => Some(u),
        None if kind == "relay" => Some(1),
        None => None,
    };
    let allowed_hosts: Vec<String> = input
        .allowed_hosts
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|h| h.trim().to_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    let id = sqlx::query_scalar::<_, String>(
        "insert into workspace_secrets (name, title, kind, note, created_by, expires_at, uses_remaining, allowed_hosts, \
                                        revealable, owner_user_id, secret_folder_id) \
         values ($1, $2, $3, $4, $5, $6::timestamptz, $7::int8, $8, $9, $10::uuid, $11::uuid) returning id::text",
    )
    .bind(&input.name)
    .bind(&input.title)
    .bind(kind)
    .bind(input.note.as_deref())
    .bind(input.created_by.as_deref())
    .bind(input.expires_at.as_deref())
    .bind(uses)
    .bind(&allowed_hosts)
    .bind(input.revealable)
    .bind(input.owner_user_id.as_deref())
    .bind(input.folder_id.as_deref())
    .fetch_one(pg)
    .await
    .map_err(|e| format!("secrets create: {e}"))?;
    for e in &input.entries {
        let cipher = sb
            .seal(&e.value)
            .map_err(|e| format!("secrets seal: {e}"))?;
        sqlx::query(
            "insert into workspace_secret_entries (secret_id, key, label, value_cipher) \
             values ($1::uuid, $2, $3, $4)",
        )
        .bind(&id)
        .bind(&e.key)
        .bind(&e.label)
        .bind(cipher)
        .execute(pg)
        .await
        .map_err(|e| format!("secrets entry write: {e}"))?;
    }
    for reader in input.readers.clone().unwrap_or_default() {
        sqlx::query(
            "insert into workspace_secret_readers (secret_id, user_id, granted_by) \
             values ($1::uuid, $2::uuid, $3) on conflict do nothing",
        )
        .bind(&id)
        .bind(reader)
        .bind(input.created_by.as_deref())
        .execute(pg)
        .await
        .map_err(|e| format!("secrets reader write: {e}"))?;
    }
    for agent in input.grant_to.clone().unwrap_or_default() {
        sqlx::query(
            "insert into workspace_secret_grants (secret_id, agent_model, granted_by) \
             values ($1::uuid, $2, $3) on conflict do nothing",
        )
        .bind(&id)
        .bind(agent)
        .bind(input.created_by.as_deref())
        .execute(pg)
        .await
        .map_err(|e| format!("secrets grant write: {e}"))?;
    }
    get_secret_doc(pg, &input.name)
        .await?
        .ok_or_else(|| "secret was created but could not be read back".to_string())
}

/// MINT A ONE-SHOT FROM A CONVERSATION. The whole point is the asymmetry: the
/// value arrives over one request and what goes back is a NAME. Granted to
/// exactly one agent, spendable exactly once, expiring within the hour — three
/// separate bounds, and a relay has to clear all of them.
pub struct MintedRelay {
    pub name: String,
    pub handle: String,
    pub label: String,
    pub expires_at: String,
    pub allowed_hosts: Vec<String>,
}

#[allow(clippy::too_many_arguments)] // the mint's inputs name the errand's own facts
pub async fn mint_relay(
    pg: &PgPool,
    sb: &SecretBox,
    label: &str,
    value: &str,
    agent_model: &str,
    created_by: Option<&str>,
    note: Option<String>,
    allowed_hosts: Option<Vec<String>>,
) -> Result<MintedRelay, String> {
    // Random, not derived from the label: a guessable name is one another
    // agent could ask for, and the grant check is the only thing that would
    // stop it.
    let name = format!("relay-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let expires_at = epoch_ms_to_iso(now_ms() + RELAY_TTL_MS);
    create_secret_doc(
        pg,
        sb,
        &CreateSecret {
            name: name.clone(),
            title: label.to_string(),
            kind: Some("relay"),
            // ONE ENTRY, so the bare `«secret:relay-…»` form resolves.
            entries: vec![SecretEntryInput {
                key: "value".into(),
                label: label.to_string(),
                value: value.to_string(),
            }],
            note,
            created_by: created_by.map(|c| c.to_string()),
            expires_at: Some(expires_at.clone()),
            uses: Some(1),
            grant_to: Some(vec![agent_model.to_string()]),
            allowed_hosts: allowed_hosts.clone().filter(|h| !h.is_empty()),
            revealable: false,
            owner_user_id: None,
            folder_id: None,
            readers: None,
        },
    )
    .await?;
    Ok(MintedRelay {
        handle: handle_for(&name, None),
        name,
        label: label.to_string(),
        expires_at,
        allowed_hosts: allowed_hosts.unwrap_or_default(),
    })
}

// ── SECRET FOLDERS ───────────────────────────────────────────────────────────
//
// The Secrets view's own organisation, and nothing to do with Files. Flat, on
// purpose: a person with thirty credentials wants six labelled piles, not a
// tree.

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretFolder {
    pub id: String,
    pub name: String,
    pub owner_user_id: Option<String>,
    pub created_at: String,
    /// People the whole folder is shared with.
    pub readers: Vec<String>,
    /// Agents that may SPEND everything in it — handle only, as ever.
    pub grants: Vec<String>,
    /// How many secrets are filed here.
    pub count: i64,
}

/// `workspace: true` asks for the ORG's folders instead of a person's — the
/// ones with no owner, which group agent credentials in Admin → Secrets.
pub async fn list_secret_folders(
    pg: &PgPool,
    user_id: &str,
    workspace: bool,
) -> Result<Vec<SecretFolder>, String> {
    let rows = if workspace {
        sqlx::query(
            "select f.id::text, f.name, f.owner_user_id::text as owner_user_id, \
                    (trunc(extract(epoch from f.created_at) * 1000))::bigint as created_ms \
               from secret_folders f where f.owner_user_id is null order by f.name",
        )
        .fetch_all(pg)
        .await
    } else {
        sqlx::query(
            "select distinct f.id::text, f.name, f.owner_user_id::text as owner_user_id, \
                    (trunc(extract(epoch from f.created_at) * 1000))::bigint as created_ms \
               from secret_folders f \
               left join secret_folder_readers fr on fr.folder_id = f.id and fr.user_id = $1::uuid \
              where f.owner_user_id = $1::uuid or fr.user_id is not null \
              order by f.name",
        )
        .bind(user_id)
        .fetch_all(pg)
        .await
    }
    .map_err(|e| format!("folders list: {e}"))?;
    let mut out = Vec::new();
    for f in &rows {
        let id = cell_str(f, "id");
        let readers = sqlx::query_scalar::<_, String>(
            "select user_id::text from secret_folder_readers where folder_id = $1::uuid",
        )
        .bind(&id)
        .fetch_all(pg)
        .await
        .map_err(|e| format!("folder readers read: {e}"))?;
        let grants = sqlx::query_scalar::<_, String>(
            "select agent_model from secret_folder_grants where folder_id = $1::uuid",
        )
        .bind(&id)
        .fetch_all(pg)
        .await
        .map_err(|e| format!("folder grants read: {e}"))?;
        let count = sqlx::query_scalar::<_, i64>(
            "select count(*)::int8 from workspace_secrets where secret_folder_id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(pg)
        .await
        .unwrap_or(0);
        out.push(SecretFolder {
            id,
            name: cell_str(f, "name"),
            owner_user_id: cell_opt_str(f, "owner_user_id"),
            created_at: epoch_ms_to_iso(cell_opt_i64(f, "created_ms").unwrap_or(0)),
            readers,
            grants,
            count,
        });
    }
    Ok(out)
}

/// `owner_user_id: None` makes a WORKSPACE folder — the org's, for grouping
/// agent credentials, so it does not vanish with the account that made it.
pub async fn create_secret_folder(
    pg: &PgPool,
    name: &str,
    owner_user_id: Option<&str>,
) -> Result<SecretFolder, String> {
    let rows = sqlx::query(
        "insert into secret_folders (name, owner_user_id) values ($1, $2::uuid) \
         returning id::text, name, owner_user_id::text as owner_user_id, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms",
    )
    .bind(name)
    .bind(owner_user_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("folder create: {e}"))?;
    let row = rows
        .first()
        .ok_or_else(|| "folder create: no row returned".to_string())?;
    Ok(SecretFolder {
        id: cell_str(row, "id"),
        name: cell_str(row, "name"),
        owner_user_id: cell_opt_str(row, "owner_user_id"),
        created_at: epoch_ms_to_iso(cell_opt_i64(row, "created_ms").unwrap_or(0)),
        readers: Vec::new(),
        grants: Vec::new(),
        count: 0,
    })
}

/// A folder is yours if you own it — or, for a WORKSPACE folder (owner null),
/// if you administer the workspace. Requiring an owner match on those would
/// make them permanently unmanageable.
async fn owns_folder(pg: &PgPool, id: &str, user_id: &str, is_admin: bool) -> Result<bool, String> {
    let rows = sqlx::query(
        "select owner_user_id::text as owner_user_id from secret_folders where id = $1::uuid",
    )
    .bind(id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("folder read: {e}"))?;
    let Some(row) = rows.first() else {
        return Ok(false);
    };
    Ok(match cell_opt_str(row, "owner_user_id") {
        None => is_admin,
        Some(owner) => owner == user_id,
    })
}

pub async fn rename_secret_folder(
    pg: &PgPool,
    id: &str,
    name: &str,
    user_id: &str,
    is_admin: bool,
) -> Result<bool, String> {
    if !owns_folder(pg, id, user_id, is_admin).await? {
        return Ok(false);
    }
    sqlx::query("update secret_folders set name = $2 where id = $1::uuid")
        .bind(id)
        .bind(name)
        .execute(pg)
        .await
        .map_err(|e| format!("folder rename: {e}"))?;
    Ok(true)
}

/// DELETING A FOLDER DOES NOT DELETE ITS CREDENTIALS — `on delete set null`
/// puts them back at the top level. Losing four working keys because somebody
/// tidied up a label would be an unforgivable way to lose them.
pub async fn delete_secret_folder(
    pg: &PgPool,
    id: &str,
    user_id: &str,
    is_admin: bool,
) -> Result<bool, String> {
    if !owns_folder(pg, id, user_id, is_admin).await? {
        return Ok(false);
    }
    sqlx::query("delete from secret_folders where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await
        .map_err(|e| format!("folder delete: {e}"))?;
    Ok(true)
}

/// Who a folder share names. An empty one answers `false` — the TS route
/// builds `{}` when the body named neither a person nor an agent.
#[derive(Default)]
pub struct FolderWho {
    pub user_id: Option<String>,
    pub agent_model: Option<String>,
}

/// Share the whole folder — with a person who may then reveal everything in
/// it, or with an agent that may spend everything in it and read none of it.
/// Sharing forward in time: access resolves at read as the union, so a
/// credential added next week is covered without anybody re-sharing it.
pub async fn share_secret_folder(
    pg: &PgPool,
    id: &str,
    who: &FolderWho,
    on: bool,
    acting_user_id: &str,
    is_admin: bool,
) -> Result<bool, String> {
    if !owns_folder(pg, id, acting_user_id, is_admin).await? {
        return Ok(false);
    }
    if let Some(user_id) = &who.user_id {
        if on {
            sqlx::query(
                "insert into secret_folder_readers (folder_id, user_id, granted_by) \
                 values ($1::uuid, $2::uuid, $3) on conflict do nothing",
            )
            .bind(id)
            .bind(user_id)
            .bind(acting_user_id)
            .execute(pg)
            .await
            .map_err(|e| format!("folder share: {e}"))?;
        } else {
            sqlx::query(
                "delete from secret_folder_readers where folder_id = $1::uuid and user_id = $2::uuid",
            )
            .bind(id)
            .bind(user_id)
            .execute(pg)
            .await
            .map_err(|e| format!("folder unshare: {e}"))?;
        }
        return Ok(true);
    }
    if let Some(agent_model) = &who.agent_model {
        if on {
            sqlx::query(
                "insert into secret_folder_grants (folder_id, agent_model, granted_by) \
                 values ($1::uuid, $2, $3) on conflict do nothing",
            )
            .bind(id)
            .bind(agent_model)
            .bind(acting_user_id)
            .execute(pg)
            .await
            .map_err(|e| format!("folder grant: {e}"))?;
        } else {
            sqlx::query(
                "delete from secret_folder_grants where folder_id = $1::uuid and agent_model = $2",
            )
            .bind(id)
            .bind(agent_model)
            .execute(pg)
            .await
            .map_err(|e| format!("folder revoke: {e}"))?;
        }
        return Ok(true);
    }
    Ok(false)
}

/// FILE IT SOMEWHERE. A WORKSPACE credential (no owner, not revealable) is
/// filed by an admin; a person's working secret is filed by that person. Both,
/// and nothing else — and only into a folder the same hand owns, since filing
/// into somebody else's would share your credential with everyone they shared
/// the folder with, which is not what "organise" means.
pub async fn move_secret_to_folder(
    pg: &PgPool,
    name: &str,
    folder_id: Option<&str>,
    acting_user_id: &str,
    is_admin: bool,
) -> Result<bool, String> {
    let rows = sqlx::query(
        "select id::text, owner_user_id::text as owner_user_id, revealable \
           from workspace_secrets where name = $1",
    )
    .bind(name)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets move read: {e}"))?;
    let Some(doc) = rows.first() else {
        return Ok(false);
    };
    let allowed = if cell_bool(doc, "revealable") {
        cell_opt_str(doc, "owner_user_id").as_deref() == Some(acting_user_id)
    } else {
        is_admin && cell_opt_str(doc, "owner_user_id").is_none()
    };
    if !allowed {
        return Ok(false);
    }
    if let Some(folder) = folder_id
        && !owns_folder(pg, folder, acting_user_id, is_admin).await?
    {
        return Ok(false);
    }
    sqlx::query("update workspace_secrets set secret_folder_id = $2::uuid where id = $1::uuid")
        .bind(cell_str(doc, "id"))
        .bind(folder_id)
        .execute(pg)
        .await
        .map_err(|e| format!("secrets move write: {e}"))?;
    Ok(true)
}

/// The working secrets this person can see: theirs, plus what was shared with
/// them — directly, through a shared folder, or by owning the folder itself.
/// Keys and labels only, like every other listing here.
pub async fn list_secrets_for_user(pg: &PgPool, user_id: &str) -> Result<Vec<SecretDoc>, String> {
    let names = sqlx::query_scalar::<_, String>(
        "select distinct s.name, s.created_at \
           from workspace_secrets s \
           left join workspace_secret_readers r on r.secret_id = s.id and r.user_id = $1::uuid \
           left join secret_folder_readers fr on fr.folder_id = s.secret_folder_id and fr.user_id = $1::uuid \
           left join secret_folders f on f.id = s.secret_folder_id \
          where s.revealable = true \
            and (s.owner_user_id = $1::uuid or r.user_id is not null or fr.user_id is not null or f.owner_user_id = $1::uuid) \
          order by s.created_at desc",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("secrets list: {e}"))?;
    let mut out = Vec::new();
    for name in names {
        if let Some(doc) = get_secret_doc(pg, &name).await? {
            out.push(doc);
        }
    }
    Ok(out)
}

// ── A CREDENTIAL FOR A HOST — the sandbox's way in ───────────────────────────
//
// A handle substitutes at the MCP gateway, which is every tool call an agent
// makes THROUGH Talaria. It is not the shell inside a workbench sandbox: git
// push runs with its own bash tool, the handle goes out as literal text, and
// pushing code is the main thing a workbench credential is for. So git asks
// US: a credential helper forwards the host, the answer keeps the value in
// process memory, and the model never knows a credential was involved.
//
// AN EMPTY ALLOWLIST IS NOT ELIGIBLE HERE. Everywhere else empty means
// "unrestricted" because a human wrote the handle into a specific command.
// Here nobody wrote anything: git names a host and we answer, so without the
// allowlist requirement one grant would hand every credential an agent holds
// to any host it could be pointed at.
pub struct HostCredential {
    pub username: String,
    pub password: String,
    pub name: String,
}

pub async fn credential_for_host(
    pg: &PgPool,
    sb: &SecretBox,
    agent_model: &str,
    host: &str,
) -> Result<Option<HostCredential>, String> {
    let lower = host.trim().to_lowercase();
    if lower.is_empty() {
        return Ok(None);
    }
    let rows = sqlx::query(
        "select s.id::text, s.name, s.title, coalesce(s.allowed_hosts, '{}') as allowed_hosts \
           from workspace_secrets s \
           left join workspace_secret_grants g on g.secret_id = s.id and g.agent_model = $1 \
           left join secret_folder_grants fg on fg.folder_id = s.secret_folder_id and fg.agent_model = $1 \
          where (g.agent_model is not null or fg.agent_model is not null) \
            and array_length(s.allowed_hosts, 1) > 0 \
            and (s.expires_at is null or s.expires_at > now()) \
            and (s.uses_remaining is null or s.uses_remaining > 0) \
          order by s.title",
    )
    .bind(agent_model)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("credential read: {e}"))?;
    let Some(doc) = rows
        .iter()
        .find(|r| host_allowed(&lower, &cell_hosts(r, "allowed_hosts")))
    else {
        return Ok(None);
    };

    let entries = cipher_entries(pg, &cell_str(doc, "id")).await?;

    // TWO SHAPES, because credentials arrive in two shapes. A doc carrying
    // `username` and `password` is a login; anything else is a token, and git
    // takes a token as the PASSWORD with a throwaway username — the convention
    // GitHub, GitLab and every registry already expect.
    let named = |k: &str| entries.iter().find(|e| e.key == k);
    let user_entry = named("username").or_else(|| named("user"));
    let pass_entry = named("password").or_else(|| named("token")).or_else(|| {
        entries
            .iter()
            .find(|e| user_entry.is_none_or(|u| u.key != e.key))
    });
    let Some(pass_entry) = pass_entry else {
        return Ok(None);
    };
    if pass_entry.cipher.is_empty() {
        return Ok(None);
    }

    let mut username = "x-access-token".to_string();
    let password = match sb.open(&pass_entry.cipher) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    if let Some(user_entry) = user_entry
        && !user_entry.cipher.is_empty()
    {
        match sb.open(&user_entry.cipher) {
            Ok(u) => username = u,
            Err(_) => return Ok(None),
        }
    }

    // SPEND IT, on the same terms as every other boundary — including burning
    // a one-shot, which is exactly right: a relay handed to a sandbox for one
    // push should not survive the push. The HOST goes into the text because
    // `resolve_handles` reads the destination out of what it is substituting
    // into — a bare handle carries no host and would refuse every credential
    // this path exists to serve; the allowlist is enforced twice, here and by
    // the substitution itself. And it is the QUALIFIED handle: a
    // username/password doc has two entries, and the bare form refuses as
    // AMBIGUOUS by design.
    let doc_name = cell_str(doc, "name");
    let spent = resolve_handles(
        pg,
        sb,
        &format!(
            "https://{lower}/ {}",
            handle_for(&doc_name, Some(&pass_entry.key))
        ),
        agent_model,
    )
    .await?;
    if spent.used.is_empty() {
        return Ok(None);
    }

    audit_spend(
        pg,
        agent_model,
        &doc_name,
        &pass_entry.label,
        serde_json::json!({
            "key": pass_entry.key,
            "hosts": [lower],
            "via": "git-credential",
            "restricted": true,
        }),
    );
    Ok(Some(HostCredential {
        username,
        password,
        name: doc_name,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_for_both_forms() {
        assert_eq!(handle_for("deploy", None), "«secret:deploy»");
        assert_eq!(handle_for("deploy", Some("pat")), "«secret:deploy.pat»");
    }

    #[test]
    fn hosts_in_reads_each_shape() {
        // URL and remote forms, lowercased and deduped.
        assert_eq!(
            hosts_in("curl https://API.Example.com/x and git@github.com:org/repo.git"),
            vec!["api.example.com".to_string(), "github.com".to_string()]
        );
        // A bare token standing alone is a host…
        assert_eq!(
            hosts_in("docker login registry.outcrop.dev"),
            vec!["registry.outcrop.dev".to_string()]
        );
        // …unless its last label is a file extension.
        assert!(hosts_in("cat package.json").is_empty());
        // The bare-host boundary: a candidate followed by a char the JS
        // lookahead would refuse (here a digit) is not a host, and the letters
        // it stopped before cannot start another one.
        assert!(hosts_in("x=a.b.com1").is_empty());
        // Port on a URL form is not part of the host.
        assert_eq!(
            hosts_in("ssh://git@src.example.com:2222/x"),
            vec!["src.example.com".to_string()]
        );
        // A URL host a bare candidate would also match: deduped to one.
        assert_eq!(hosts_in("https://x.dev/ x.dev"), vec!["x.dev".to_string()]);
    }

    #[test]
    fn host_allowed_is_suffix_dot_not_ends_with() {
        let allowed = vec!["github.com".to_string()];
        assert!(host_allowed("github.com", &allowed));
        assert!(host_allowed("api.github.com", &allowed));
        assert!(!host_allowed("github.com.evil.net", &allowed));
        // Leading dot and whitespace in the entry are tolerated.
        assert!(host_allowed("x.dev", &[" .x.dev ".to_string()]));
        // An empty entry covers nothing.
        assert!(!host_allowed("x.dev", &["".to_string()]));
    }

    #[test]
    fn mentions_handle_is_case_blind() {
        assert!(mentions_handle("push with «secret:deploy»"));
        assert!(mentions_handle("PUSH WITH «SECRET:DEPLOY»"));
        assert!(!mentions_handle("nothing to see"));
    }
}
