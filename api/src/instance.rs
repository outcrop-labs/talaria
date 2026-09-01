// The instance's HOSTING domain — port of ui/src/server/instance.ts. Where
// this Talaria deployment lives (talaria.example.com), as opposed to the
// email sign-up domains people authenticate with. Verification is a
// SELF-FETCH: the server requests its own identity beacon through the
// candidate domain and checks the instance id that comes back — proof that
// DNS, routing, and TLS all actually land on THIS deployment. The beacon
// route itself (/api/well-known/talaria-instance) still serves from TS until
// its group flips; the verify hop goes through the domain, not this process.

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::settings::{get_setting, set_setting};
use sqlx::PgPool;
use std::time::Duration;

const ID_KEY: &str = "instance_id";
const DOMAIN_KEY: &str = "instance_domain";

/// The stored config (InstanceDomain) — stored shape = wire shape; the GET
/// passes the raw jsonb through, so this struct only exists for the verify
/// round-trip's rewrite.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDomain {
    pub domain: String,
    pub verified: bool,
    #[serde(rename = "verifiedAt")]
    pub verified_at: Option<String>,
}

/// Stable per-deployment identity, minted on first ask.
pub async fn get_instance_id(pg: &PgPool) -> Result<String, sqlx::Error> {
    let existing = get_setting(pg, ID_KEY, serde_json::Value::Null).await;
    if let Some(id) = existing.as_str() {
        return Ok(id.to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    set_setting(pg, ID_KEY, &serde_json::Value::String(id.clone())).await?;
    Ok(id)
}

/// The raw stored config — passthrough jsonb, null when unset.
pub async fn get_instance_domain(pg: &PgPool) -> serde_json::Value {
    get_setting(pg, DOMAIN_KEY, serde_json::Value::Null).await
}

/// Canonical base URL when a verified hosting domain exists; else None (the
/// caller falls back to the request origin).
pub async fn instance_base_url(pg: &PgPool) -> Option<String> {
    let cfg = get_instance_domain(pg).await;
    let domain = cfg.get("domain").and_then(|d| d.as_str())?;
    if !cfg
        .get("verified")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    Some(format!("https://{domain}"))
}

/// Hand-rolled `/^(?!-)[a-z0-9-]{1,63}(?<!-)(\.(?!-)[a-z0-9-]{1,63}(?<!-))+(:\d+)?$/`
/// — labels of 1–63 [a-z0-9-] with no leading/trailing dash, two or more of
/// them (a bare host is not a hosting domain), optional numeric port.
fn domain_ok(d: &str) -> bool {
    let host = match d.rsplit_once(':') {
        // Only when what follows the last colon is all digits is it a port —
        // else the colon is part of the (already failing) host.
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => d,
    };
    let label_ok = |l: &str| {
        (1..=63).contains(&l.len())
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let mut labels = host.split('.');
    // The regex needs TWO or more labels, and every one of them valid: one
    // label, then a `+` run of dot-prefixed labels ("a..bb" fails on the
    // empty one).
    match labels.next() {
        Some(first) if label_ok(first) => {
            let rest: Vec<&str> = labels.collect();
            !rest.is_empty() && rest.iter().all(|l| label_ok(l))
        }
        _ => false,
    }
}

/// trim → lowercase → strip ONE leading scheme → everything from the first
/// slash dies — the exact normalization chain in setInstanceDomain.
fn normalize_domain(raw: &str) -> String {
    let lowered = raw.trim().to_lowercase();
    let schemeless = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
        .unwrap_or(&lowered);
    schemeless.split('/').next().unwrap_or_default().to_string()
}

/// Set (normalize + validate) or clear. `Err` is the route's 400 sentence.
///
/// RECORDED DIVERGENCE — the clear path works here and cannot on TS:
/// postgres.js turns `sql.json(null)` into SQL NULL, so TS's
/// setSetting('instance_domain', null) violates app_settings.value's NOT
/// NULL constraint, the route's validation catch leaks the raw Postgres
/// sentence as a 400, and the domain can never be cleared at all. sqlx
/// encodes a real jsonb null, so the upsert lands: 200, `{instance: null}`,
/// and every reader (both runtimes' getSetting) sees null. The TS try/catch
/// was built for the validation throw; its DB-failure leak is the bug this
/// port does not reproduce (RUST-MIGRATION.md, divergences).
pub async fn set_instance_domain(
    pg: &PgPool,
    domain: Option<&str>,
) -> Result<Option<InstanceDomain>, String> {
    let Some(raw) = domain else {
        set_setting(pg, DOMAIN_KEY, &serde_json::Value::Null)
            .await
            .map_err(|e| format!("{e}"))?;
        return Ok(None);
    };
    let d = normalize_domain(raw);
    if !domain_ok(&d) {
        return Err("that does not look like a domain".into());
    }
    let next = InstanceDomain {
        domain: d,
        verified: false,
        verified_at: None,
    };
    set_setting(
        pg,
        DOMAIN_KEY,
        &serde_json::to_value(&next).map_err(|e| format!("{e}"))?,
    )
    .await
    .map_err(|e| format!("{e}"))?;
    Ok(Some(next))
}

/// The verify result's wire shape: `{verified: true}` or
/// `{verified: false, error}` — `error` is ABSENT on success (skip_serializing).
#[derive(serde::Serialize)]
pub struct VerifyResult {
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Round-trip proof: fetch our own well-known through the domain, https
/// first, http as the fallback. An OK answer naming a DIFFERENT instance is
/// terminal (wrong box — say so); a failed hop falls through to the next
/// scheme. The 8s bound is per fetch, like AbortSignal.timeout.
pub async fn verify_instance_domain(pg: &PgPool) -> VerifyResult {
    let cfg = get_instance_domain(pg).await;
    let domain = match serde_json::from_value::<InstanceDomain>(cfg.clone()) {
        // null (never set) and a corrupt value both land on the same sentence.
        Err(_) => {
            return VerifyResult {
                verified: false,
                error: Some("no domain configured".into()),
            };
        }
        Ok(d) => d,
    };
    let id = match get_instance_id(pg).await {
        Ok(id) => id,
        Err(e) => {
            return VerifyResult {
                verified: false,
                error: Some(format!("{e}")),
            };
        }
    };
    // redirect: 'follow' with a real cap; a verify hop that bounces forever
    // is a misconfigured proxy, and 20 is where browsers and undici call it.
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(8_000))
        .redirect(reqwest::redirect::Policy::limited(20))
        .build()
    else {
        return VerifyResult {
            verified: false,
            error: Some(format!(
                "{} is not reachable from this server (or does not serve Talaria yet)",
                domain.domain
            )),
        };
    };
    for scheme in ["https", "http"] {
        let url = format!(
            "{scheme}://{}/api/well-known/talaria-instance",
            domain.domain
        );
        let Ok(res) = client.get(&url).send().await else {
            continue; // try the next scheme
        };
        if !res.status().is_success() {
            continue;
        }
        let Ok(j) = res.json::<serde_json::Value>().await else {
            continue;
        };
        if j.get("instance").and_then(|v| v.as_str()) == Some(id.as_str()) {
            let verified = InstanceDomain {
                domain: domain.domain.clone(),
                verified: true,
                verified_at: Some(epoch_ms_to_iso(now_ms())),
            };
            // Serializing this struct cannot fail; a failed WRITE must not
            // report the proof failed — the error is logged and dropped.
            let value = serde_json::to_value(&verified).unwrap_or_default();
            if set_setting(pg, DOMAIN_KEY, &value).await.is_err() {
                tracing::error!("[instance] could not persist verified domain");
            }
            return VerifyResult {
                verified: true,
                error: None,
            };
        }
        return VerifyResult {
            verified: false,
            error: Some(format!(
                "{} answers, but as a DIFFERENT Talaria instance — check your DNS/proxy target",
                domain.domain
            )),
        };
    }
    VerifyResult {
        verified: false,
        error: Some(format!(
            "{} is not reachable from this server (or does not serve Talaria yet)",
            domain.domain
        )),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_truth_table_matches_the_ts_regex() {
        let ok = [
            "talaria.example.com",
            "a.bb",
            "a-b.c-d.ee",
            "x1.yy",
            "host.example.com:8443",
            "a.bb:80",
        ];
        for d in ok {
            assert!(domain_ok(d), "should accept {d:?}");
        }
        // Length edges: a 63-char label is legal, 64 is not — in either slot.
        assert!(domain_ok(&format!("{}.bb", "a".repeat(63))));
        assert!(!domain_ok(&format!("{}.bb", "a".repeat(64))));
        assert!(domain_ok(&format!("aa.{}", "b".repeat(63))));
        assert!(!domain_ok(&format!("aa.{}", "b".repeat(64))));
        let bad = [
            "localhost",
            "-a.bb",
            "a-.bb",
            "a.-bb",
            "a.b-",
            "a..bb",
            "a.bb:xy",
            "a.bb:",
            "BAD.bb",
            "a.b_c",
            "a.b_c:80",
            "",
            "a",
        ];
        for d in bad {
            assert!(!domain_ok(d), "should reject {d:?}");
        }
    }

    #[test]
    fn normalization_strips_scheme_path_and_case() {
        // The route normalizes before the regex sees it; these all land on
        // the same hosting domain.
        for raw in ["HTTPS://Talaria.Example.COM/x/y", "talaria.example.com"] {
            assert_eq!(normalize_domain(raw), "talaria.example.com");
        }
        // Exactly one scheme strip (the TS regex is anchored, not global).
        assert_eq!(normalize_domain("https://http://x.y"), "http:");
    }
}
