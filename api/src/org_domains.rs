// Org sign-up EMAIL domains — the self-join read of
// ui/src/server/org-domains.ts. An admin adds an email domain and proves
// ownership with a DNS TXT record; from then on anyone who authenticates
// through a provider that VERIFIES email (Google) with an address on that
// domain joins automatically as a member. The admin CRUD and the DNS
// verification round-trip port with the admin surfaces in batch 3.

use sqlx::PgPool;

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
