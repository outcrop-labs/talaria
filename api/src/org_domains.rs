// Org sign-up EMAIL domains — port of ui/src/server/org-domains.ts. An admin
// adds an email domain and proves ownership with a DNS TXT record; from then
// on anyone who authenticates through a provider that VERIFIES email (Google)
// with an address on that domain joins automatically as a member.
//
// These are the domains after the @ in people's email addresses, wholly
// independent of wherever this Talaria instance is HOSTED
// (talaria.example.com hosting ≠ example.com emails). DNS proof is required
// before a domain admits anyone — an admin can't (typo or otherwise) claim
// gmail.com.

use hickory_resolver::TokioResolver;
use sqlx::{PgPool, Row};
use std::sync::OnceLock;

/// Self-join gate: a provider-verified email on a VERIFIED org domain may
/// create an account. Exact-domain match only — subdomains are added
/// individually, on purpose. (`email.toLowerCase().split('@')[1]`, the
/// segment after the first @, untrimmed — the domain column is normalized
/// at write time.)
pub async fn self_join_allowed(pg: &PgPool, email: Option<&str>) -> Result<bool, sqlx::Error> {
    let Some(email) = email else { return Ok(false) };
    let lower = email.to_lowercase();
    let Some(domain) = lower.split('@').nth(1) else {
        return Ok(false);
    };
    if domain.is_empty() {
        return Ok(false);
    }
    let row: Option<(i32,)> =
        sqlx::query_as("select 1 from org_domains where domain = $1 and verified")
            .bind(domain)
            .fetch_optional(pg)
            .await?;
    Ok(row.is_some())
}

// ── The admin console (listOrgDomains / addOrgDomain / removeOrgDomain) ──────

/// normalize(): trim, lowercase, strip a leading scheme, everything after the
/// first /, and a leading @ — so a pasted `https://mail.example.com/login`
/// and `@example.com` both land on bare hostnames.
fn normalize(d: &str) -> String {
    let mut s = d.trim().to_lowercase();
    if let Some(rest) = s.strip_prefix("https://") {
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix("http://") {
        s = rest.to_string();
    }
    if let Some(slash) = s.find('/') {
        s.truncate(slash);
    }
    s.strip_prefix('@').unwrap_or(&s).to_string()
}

/// DOMAIN_RE — one dot-separated run of labels, each 1–63 chars of
/// [a-z0-9-], neither edge a dash, and at least two labels (a bare TLD is
/// not an email domain).
fn domain_ok(d: &str) -> bool {
    let labels: Vec<&str> = d.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            let b = l.as_bytes();
            !b.is_empty()
                && b.len() <= 63
                && b.first().is_some_and(|c| c.is_ascii_alphanumeric())
                && b.last().is_some_and(|c| c.is_ascii_alphanumeric())
                && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
        })
}

/// The domain row in the TS ROW's wire order.
fn domain_json(r: &sqlx::postgres::PgRow) -> Result<serde_json::Value, sqlx::Error> {
    let iso = crate::agent_auth::epoch_ms_to_iso;
    Ok(serde_json::json!({
        "id": r.try_get::<String, _>("id")?,
        "domain": r.try_get::<String, _>("domain")?,
        "verified": r.try_get::<bool, _>("verified")?,
        "verificationToken": r.try_get::<String, _>("verification_token")?,
        "addedBy": r.try_get::<Option<String>, _>("added_by")?,
        "createdAt": iso(r.try_get::<i64, _>("created_ms")?),
        "verifiedAt": r.try_get::<Option<i64>, _>("verified_ms")?.map(iso),
    }))
}

const DOMAIN_COLS: &str = "id::text, domain, verified, verification_token, added_by, \
     (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
     (trunc(extract(epoch from verified_at) * 1000))::bigint as verified_ms";

pub async fn list_org_domains(pg: &PgPool) -> Result<Vec<serde_json::Value>, String> {
    // AssertSqlSafe: the interpolated text is the DOMAIN_COLS constant.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select {DOMAIN_COLS} from org_domains order by domain"
    )))
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    rows.iter()
        .map(domain_json)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Add (or re-add) a domain, minting a fresh verification token. Re-adding a
/// known domain keeps its verified state — only the added_by attribution
/// moves (the TS on-conflict updates exactly that one column).
pub async fn add_org_domain(
    pg: &PgPool,
    domain: &str,
    added_by: &str,
) -> Result<serde_json::Value, String> {
    let d = normalize(domain);
    if !domain_ok(&d) {
        return Err("that does not look like a domain".into());
    }
    // `talaria-verify=` + 18 random bytes hex — node's randomBytes(18).toString('hex').
    let mut raw = [0u8; 18];
    getrandom::fill(&mut raw).map_err(|e| e.to_string())?;
    let token = format!(
        "talaria-verify={}",
        raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "insert into org_domains (domain, verification_token, added_by) values ($1, $2, $3) \
         on conflict (domain) do update set added_by = excluded.added_by \
         returning {DOMAIN_COLS}"
    )))
    .bind(&d)
    .bind(&token)
    .bind(added_by)
    .fetch_one(pg)
    .await
    .map_err(|e| e.to_string())?;
    domain_json(&row).map_err(|e| e.to_string())
}

pub async fn remove_org_domain(pg: &PgPool, id: &str) -> Result<(), String> {
    sqlx::query("delete from org_domains where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── DNS verification (verifyOrgDomain) ───────────────────────────────────────

fn resolver() -> &'static TokioResolver {
    static R: OnceLock<TokioResolver> = OnceLock::new();
    R.get_or_init(|| {
        // The system resolver (/etc/resolv.conf), like node:dns — read once
        // per process. A host with no resolv.conf falls to the empty config,
        // which fails every lookup: the honest answer for that operator.
        match TokioResolver::builder_tokio() {
            Ok(b) => b.build(),
            Err(_) => {
                TokioResolver::builder_with_config(Default::default(), Default::default()).build()
            }
        }
    })
}

/// All TXT strings for one host, or an empty vec on any lookup failure —
/// node's `resolveTxt(host).catch(() => [])`. Each TXT record may hold
/// several character-strings; node surfaces them as one array of chunks, and
/// the caller joins before comparing.
async fn resolve_txt(host: &str) -> Vec<String> {
    let name = match host.parse::<hickory_resolver::proto::rr::Name>() {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    match resolver().txt_lookup(name).await {
        Ok(lookup) => lookup
            .iter()
            // One record = one array of chunks, joined with '' (node's
            // `chunks.join('')`).
            .map(|txt| {
                txt.txt_data()
                    .iter()
                    .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                    .collect::<String>()
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Check DNS for the verification TXT record (verifyOrgDomain). Canonical
/// placement is `_talaria-verify.<domain>` — deliberately NOT
/// `talaria.<domain>`-shaped, since many orgs HOST Talaria on a subdomain
/// like talaria.example.com and the email domain being verified here is a
/// separate concern entirely. Legacy/root placements still pass.
pub async fn verify_org_domain(pg: &PgPool, id: &str) -> Result<serde_json::Value, String> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select {DOMAIN_COLS} from org_domains where id = $1::uuid"
    )))
    .bind(id)
    .fetch_optional(pg)
    .await
    .map_err(|e| e.to_string())?;
    let Some(row) = row else {
        return Ok(serde_json::json!({ "verified": false, "error": "not found" }));
    };
    let domain: String = row.try_get("domain").map_err(|e| e.to_string())?;
    let token: String = row
        .try_get("verification_token")
        .map_err(|e| e.to_string())?;
    let hosts = [
        format!("_talaria-verify.{domain}"),
        format!("_talaria.{domain}"),
        domain.clone(),
    ];
    for host in &hosts {
        let records = resolve_txt(host).await;
        if records.iter().any(|r| r.trim() == token) {
            let _ = sqlx::query(
                "update org_domains set verified = true, verified_at = now() where id = $1::uuid",
            )
            .bind(id)
            .execute(pg)
            .await;
            return Ok(serde_json::json!({ "verified": true }));
        }
    }
    Ok(serde_json::json!({
        "verified": false,
        "error": format!(
            "TXT record not found. Add \"{token}\" as a TXT record on _talaria-verify.{domain} (or on {domain} itself) and try again"
        ),
    }))
}
