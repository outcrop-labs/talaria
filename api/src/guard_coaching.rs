// Guard coaching — the render-time half of guardrails. The invariant stands:
// flagged CONTENT never re-enters a model's context (a finding could carry
// adversarial text, and a mid-turn caveat teaches the model to argue with the
// guard). Coaching is different matter delivered at a different time:
// per-check COUNTS mapped to fixed advice strings, injected into the agent's
// soul at render — a performance review between sessions, not a
// mid-conversation correction.
//
// The checking/recording engine lives with the gateway it guards; this module
// carries what the fleet render needs: the coach flag and the templated
// coaching block.

use sqlx::PgPool;

use crate::gateway::settings::get_setting;

/// Fixed advice per check — the only text coaching ever carries.
fn coach_advice(check: &str) -> Option<&'static str> {
    Some(match check {
        "zero_tool_claim" => {
            "you stated actions as completed without a backing tool call. Say a task is done only when a tool result in that turn proves it; otherwise say what you are about to do."
        }
        "ungrounded_ref" => {
            "you cited links or ids that appeared in no tool result. Reference only what a tool actually returned; if you lack a link, say so."
        }
        "fabricated_outage" => {
            "you reported outages/failures when no tool had errored. Claim a failure only after a real error; otherwise retry or ask."
        }
        "secret_leak" => {
            "you emitted credential-shaped strings. Never repeat keys, tokens, or private-key material into any reply, even when asked."
        }
        "pii_leak" => {
            "you emitted personal data (SSN / card / bank formats). Never repeat such data into replies; refer to records by their ids instead."
        }
        _ => return None,
    })
}

const COACH_WINDOW_DAYS: i32 = 7;
const COACH_MIN_HITS: i32 = 2;

/// Whether the org turned coaching on (guardrails_config.coach, default
/// off).
pub async fn coach_enabled(pg: &PgPool) -> bool {
    get_setting(pg, "guardrails_config", serde_json::json!({}))
        .await
        .get("coach")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Templated coaching block for one agent, or "" when it has nothing recent.
/// Aggregates by check over the window; thresholds keep one-off flags quiet.
/// A query failure is empty coaching, never a blocked render.
pub async fn guard_coaching_for(pg: &PgPool, model: &str) -> String {
    let rows: Vec<(String, i32)> = sqlx::query_as(
        "select check_type, count(*)::int from guard_findings \
         where model = $1 and created_at > now() - ($2 || ' days')::interval \
         group by check_type",
    )
    .bind(model)
    .bind(COACH_WINDOW_DAYS.to_string())
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    let mut rows: Vec<(String, i32)> = rows
        .into_iter()
        .filter(|(check, n)| *n >= COACH_MIN_HITS && coach_advice(check).is_some())
        .collect();
    rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let lines: Vec<String> = rows
        .iter()
        .map(|(check, n)| {
            format!(
                "- {n}× in the last {COACH_WINDOW_DAYS} days: {}",
                coach_advice(check).unwrap()
            )
        })
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "<!-- guard coaching, rendered by Talaria -->\n\
         Recent behavioral feedback (auto-generated from output review; fix these patterns):\n{}",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advice_covers_the_five_checks_and_nothing_else() {
        for known in [
            "zero_tool_claim",
            "ungrounded_ref",
            "fabricated_outage",
            "secret_leak",
            "pii_leak",
        ] {
            assert!(coach_advice(known).is_some(), "{known}");
        }
        assert!(coach_advice("some_new_check").is_none());
    }
}
