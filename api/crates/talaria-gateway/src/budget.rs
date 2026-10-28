// Spend ceiling — the circuit breaker checked BEFORE an upstream call, at org
// and caller scope, with the key's own caps min-merged under the admin's
// ceiling. Off by default; a failed read must not become an outage.

use crate::settings::get_setting_hot;
use crate::usage::{SpendUnit, SpendWindow, spend_clears_at, spend_since};
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use talaria_agent_auth::now_ms;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BudgetLimits {
    /// Tokens allowed in the window. None/zero = unlimited.
    pub tokens: Option<f64>,
    /// Priced USD allowed in the window. None/zero = unlimited.
    pub usd: Option<f64>,
}

impl BudgetLimits {
    fn from_json(v: Option<&Value>) -> Self {
        let read = |k: &str| -> Option<f64> {
            v.and_then(|x| x.get(k))
                .and_then(|x| x.as_f64())
                .filter(|n| n.is_finite() && *n > 0.0)
        };
        BudgetLimits {
            tokens: read("tokens"),
            usd: read("usd"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BudgetPolicy {
    pub window_hours: i64,
    pub org: Option<BudgetLimits>,
    pub per_agent: Option<BudgetLimits>,
    pub agents: HashMap<String, BudgetLimits>,
}

impl BudgetPolicy {
    /// Off by default — an existing deployment must never start getting 429s.
    fn default_policy() -> Self {
        BudgetPolicy {
            window_hours: 24,
            org: None,
            per_agent: None,
            agents: HashMap::new(),
        }
    }

    fn from_json(v: &Value) -> Self {
        let mut p = Self::default_policy();
        if !v.is_object() {
            return p;
        }
        if let Some(h) = v.get("windowHours").and_then(|x| x.as_f64()) {
            p.window_hours = h.round() as i64;
        }
        p.org = Some(BudgetLimits::from_json(
            v.get("org").filter(|x| !x.is_null()),
        ));
        p.per_agent = Some(BudgetLimits::from_json(
            v.get("perAgent").filter(|x| !x.is_null()),
        ));
        if let Some(Value::Object(agents)) = v.get("agents") {
            for (name, limits) in agents {
                p.agents
                    .insert(name.clone(), BudgetLimits::from_json(Some(limits)));
            }
        }
        p
    }

    /// The admin's ceiling for a caller: its own override, else perAgent.
    fn caller_limits(&self, caller: &str) -> BudgetLimits {
        self.agents
            .get(caller)
            .copied()
            .unwrap_or_else(|| self.per_agent.unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct BudgetDenial {
    pub scope: &'static str, // "org" | "caller"
    pub subject: Option<String>,
    pub unit: &'static str, // "tokens" | "usd"
    pub limit: f64,
    pub used: f64,
    pub window_hours: i64,
    /// The `Retry-After` an operator's or an app's key is answered with. An
    /// agent's key gets none: see `resets_at`.
    pub retry_after_seconds: i64,
    /// When the window's spend falls back under the cap if nothing more is
    /// spent, in epoch seconds (`spend_clears_at`) — `None` when the read
    /// failed. It rides the refusal as `error.resets_at`, the field Hermes
    /// reads for a 429's reset: with no `Retry-After` beside it, an agent
    /// fails the turn after a few seconds of retries and names this time,
    /// instead of waiting out 300s twice and saying "resets in ~5 min" about
    /// a 24h window.
    pub resets_at: Option<i64>,
    /// Which side set the binding number — where the fix lives.
    pub via: &'static str, // "key" | "admin"
}

/// min-merge, remembering which side bound each unit so a denial can point
/// its reader at the surface that actually holds the number.
struct MergedCaps {
    tokens: Option<f64>,
    usd: Option<f64>,
    tokens_from_key: bool,
    usd_from_key: bool,
}

fn merge_caps(key: &BudgetLimits, admin: &BudgetLimits) -> MergedCaps {
    let pick = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (None, b) => (b, false),
        (a, None) => (a, true),
        (Some(a), Some(b)) => (Some(a.min(b)), a <= b),
    };
    let (tokens, tokens_from_key) = pick(key.tokens, admin.tokens);
    let (usd, usd_from_key) = pick(key.usd, admin.usd);
    MergedCaps {
        tokens,
        usd,
        tokens_from_key,
        usd_from_key,
    }
}

/// The `Retry-After` an operator's or an app's key hears. A rolling window
/// ages out continuously, so "try again shortly" is a client's cue to look
/// again, never longer than the window itself — the real resume time is
/// `resets_at`, beside it. An agent's key is not sent this (llm_chat.rs
/// `budget_refusal`): Hermes believes it, and holds the turn for it.
fn retry_after(window_hours: i64) -> i64 {
    (window_hours * 3600).clamp(30, 300)
}

/// en-US grouping for a token figure. Shared with the task-usage route,
/// whose activity line carries the same shape.
pub fn group(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The sentence a denial carries, pointing its reader at the surface
/// whoever-hit-it can actually act on.
pub fn budget_message(d: &BudgetDenial) -> String {
    let (used, cap) = if d.unit == "usd" {
        (format!("${:.2}", d.used), format!("${:.2}", d.limit))
    } else {
        (
            format!("{} tokens", group(d.used as i64)),
            format!("{} tokens", group(d.limit as i64)),
        )
    };
    let who = if d.scope == "org" {
        "This organization".to_string()
    } else {
        format!("\"{}\"", d.subject.clone().unwrap_or_default())
    };
    let fix = if d.via == "key" {
        "the key's owner can raise or remove the cap (Settings → API keys)."
    } else {
        "an admin can raise the limit (Admin → Settings → LLM budgets)."
    };
    let when = d
        .resets_at
        .map(|at| format!(" (around {}, if nothing more is spent)", utc_minute(at)))
        .unwrap_or_default();
    format!(
        "{who} has reached its LLM budget: {used} of {cap} in the last {h}h. Requests resume as spend ages out of the window{when}, or {fix}",
        h = d.window_hours
    )
}

/// `2026-10-28 14:21 UTC` for an epoch second — the minute a denial says the
/// window reopens, rounded up: it is not open any earlier.
fn utc_minute(epoch_s: i64) -> String {
    let minute = epoch_s.saturating_add(59).div_euclid(60).saturating_mul(60);
    let iso = talaria_agent_auth::epoch_ms_to_iso(minute.saturating_mul(1000));
    match (iso.get(0..10), iso.get(11..16)) {
        (Some(day), Some(minute)) => format!("{day} {minute} UTC"),
        _ => iso,
    }
}

// Spend is read per request, so it is cached briefly. The TTL collapses as
// the caller approaches its ceiling: cheap while there's headroom, exact at
// the edge, so the overshoot a cache could hide stays bounded.
struct CacheEntry {
    at: i64,
    ttl_ms: i64,
    value: SpendWindow,
}

fn spend_cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn cached_spend(
    pg: &PgPool,
    window_hours: i64,
    subject: Option<&str>,
    fraction_used: impl Fn(&SpendWindow) -> f64,
) -> Option<SpendWindow> {
    let key = format!("{window_hours}:{}", subject.unwrap_or("*"));
    if let Ok(cache) = spend_cache().lock()
        && let Some(hit) = cache.get(&key)
        && now_ms() - hit.at < hit.ttl_ms
    {
        return Some(hit.value);
    }
    let value = spend_since(pg, window_hours, subject).await.ok()?;
    // >80% spent: no caching at all. Otherwise 15s, bounding how much a burst
    // can slip past the check to roughly 15s of traffic.
    let ttl_ms = if fraction_used(&value) > 0.8 {
        0
    } else {
        15_000
    };
    if let Ok(mut cache) = spend_cache().lock() {
        cache.insert(
            key,
            CacheEntry {
                at: now_ms(),
                ttl_ms,
                value,
            },
        );
    }
    Some(value)
}

/// The circuit breaker. Returns a denial when `caller` (or the org) is over
/// budget, else None. Called BEFORE the upstream request — a refusal must
/// cost nothing.
pub async fn check_budget(
    pg: &PgPool,
    caller: &str,
    key_caps: BudgetLimits,
) -> Option<BudgetDenial> {
    let policy = BudgetPolicy::from_json(
        &get_setting_hot(
            pg,
            "llm_budgets",
            serde_json::json!({"windowHours": 24, "org": null, "perAgent": null, "agents": {}}),
        )
        .await,
    );
    let admin = policy.caller_limits(caller);
    let caps = merge_caps(&key_caps, &admin);
    let org = policy.org.unwrap_or_default();
    if caps.tokens.is_none() && caps.usd.is_none() && org.tokens.is_none() && org.usd.is_none() {
        return None;
    }
    let window_hours = policy.window_hours.clamp(1, 24 * 365);

    // Caller scope first — a key throttled by its own cap hears about its own
    // number, not the org's.
    struct Scope {
        scope: &'static str,
        subject: Option<String>,
        limits: BudgetLimits,
        tokens_from_key: bool,
        usd_from_key: bool,
    }
    let scopes = [
        Scope {
            scope: "caller",
            subject: Some(caller.to_string()),
            limits: BudgetLimits {
                tokens: caps.tokens,
                usd: caps.usd,
            },
            tokens_from_key: caps.tokens_from_key,
            usd_from_key: caps.usd_from_key,
        },
        Scope {
            scope: "org",
            subject: None,
            limits: org,
            tokens_from_key: false,
            usd_from_key: false,
        },
    ];

    for s in scopes {
        let (token_cap, usd_cap) = (s.limits.tokens, s.limits.usd);
        if token_cap.is_none() && usd_cap.is_none() {
            continue;
        }
        // A failed read must not become an outage — skip the scope.
        let Some(spend) = cached_spend(pg, window_hours, s.subject.as_deref(), |v| {
            (token_cap.map_or(0.0, |t| v.tokens as f64 / t))
                .max(usd_cap.map_or(0.0, |u| v.cost / u))
        })
        .await
        else {
            continue;
        };
        // Only a refusal pays for the resume-time read; a failed one leaves
        // the refusal without a time, never without the refusal.
        let clears_at = |unit, cap| {
            let subject = s.subject.clone();
            async move {
                spend_clears_at(pg, window_hours, subject.as_deref(), unit, cap)
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("[budget] resume time unreadable: {e}");
                        None
                    })
            }
        };
        if let Some(t) = token_cap
            && spend.tokens as f64 >= t
        {
            return Some(BudgetDenial {
                scope: s.scope,
                subject: s.subject.clone(),
                unit: "tokens",
                limit: t,
                used: spend.tokens as f64,
                window_hours,
                retry_after_seconds: retry_after(window_hours),
                resets_at: clears_at(SpendUnit::Tokens, t).await,
                via: if s.tokens_from_key { "key" } else { "admin" },
            });
        }
        if let Some(u) = usd_cap
            && spend.cost >= u
        {
            return Some(BudgetDenial {
                scope: s.scope,
                subject: s.subject.clone(),
                unit: "usd",
                limit: u,
                used: spend.cost,
                window_hours,
                retry_after_seconds: retry_after(window_hours),
                resets_at: clears_at(SpendUnit::Usd, u).await,
                via: if s.usd_from_key { "key" } else { "admin" },
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_picks_the_tighter_and_remembers_which_side() {
        // Owner throttles below the admin: key binds, and the denial says so.
        let m = merge_caps(
            &BudgetLimits {
                tokens: Some(1000.0),
                usd: None,
            },
            &BudgetLimits {
                tokens: Some(5000.0),
                usd: Some(2.0),
            },
        );
        assert_eq!(m.tokens, Some(1000.0));
        assert!(m.tokens_from_key);
        assert_eq!(m.usd, Some(2.0));
        assert!(!m.usd_from_key);
        // Admin tighter than key: admin binds.
        let m = merge_caps(
            &BudgetLimits {
                tokens: Some(9000.0),
                usd: None,
            },
            &BudgetLimits {
                tokens: Some(5000.0),
                usd: None,
            },
        );
        assert_eq!(m.tokens, Some(5000.0));
        assert!(!m.tokens_from_key);
    }

    #[test]
    fn zero_and_nonfinite_limits_are_unlimited() {
        let l = BudgetLimits::from_json(Some(&serde_json::json!({"tokens": 0, "usd": -5})));
        assert_eq!(l.tokens, None);
        assert_eq!(l.usd, None);
    }

    #[test]
    fn message_names_the_surface_that_holds_the_number() {
        let key_side = BudgetDenial {
            scope: "caller",
            subject: Some("api:my-key".into()),
            unit: "tokens",
            limit: 1000.0,
            used: 1000.0,
            window_hours: 24,
            retry_after_seconds: 300,
            resets_at: None,
            via: "key",
        };
        let m = budget_message(&key_side);
        assert!(m.contains("\"api:my-key\" has reached its LLM budget"));
        assert!(m.contains("1,000 tokens of 1,000 tokens"));
        assert!(m.contains("Settings → API keys"));
        assert!(m.ends_with("Requests resume as spend ages out of the window, or the key's owner can raise or remove the cap (Settings → API keys)."));
    }

    #[test]
    fn message_names_the_minute_the_window_reopens() {
        let agent = BudgetDenial {
            scope: "caller",
            subject: Some("engineer-engineering".into()),
            unit: "usd",
            limit: 2.0,
            used: 2.5,
            window_hours: 24,
            retry_after_seconds: 300,
            // 2026-10-28T14:20:59Z: named as the next whole minute.
            resets_at: Some(1_793_197_259),
            via: "admin",
        };
        assert_eq!(
            budget_message(&agent),
            "\"engineer-engineering\" has reached its LLM budget: $2.50 of $2.00 in the last 24h. \
             Requests resume as spend ages out of the window (around 2026-10-28 14:21 UTC, if \
             nothing more is spent), or an admin can raise the limit (Admin → Settings → LLM budgets)."
        );
    }

    #[test]
    fn the_reset_minute_rounds_up_and_a_whole_minute_stays() {
        assert_eq!(utc_minute(1_793_197_200), "2026-10-28 14:20 UTC");
        assert_eq!(utc_minute(1_793_197_201), "2026-10-28 14:21 UTC");
    }

    #[test]
    fn retry_after_is_bounded_by_the_window() {
        assert_eq!(retry_after(24), 300);
        assert_eq!(retry_after(24 * 365), 300);
    }
}
