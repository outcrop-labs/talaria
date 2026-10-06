// SHADOW MODE — measure the port before trusting it.
//
// THE RULE THIS MODULE EXISTS TO ENFORCE: a site does not switch over to a
// decision model because the model looks good in a fixture. It switches over
// because, on this install's own traffic, somebody read a table of agreements
// and disagreements and chose a threshold from it. Until then the existing
// code decides exactly as it always did and the port is asked the same
// question with nobody listening.
//
// So `compare` takes the answer the site ALREADY produced and never gives one
// back. There is no return value a caller could accidentally act on, which is
// the whole design: shadow mode that can influence the decision is not shadow
// mode. It also never blocks — the comparison runs detached, so a slow or
// unreachable provider costs the request nothing — and it never surfaces an
// error, because a failed measurement must not become a failed feature.
//
// WHY THE LEDGER TAKES TEXT. `baseline` and `port_answer` are strings so one
// table serves a yes/no gate, a pick-one router and a how-much score. Each
// site renders its own primitive and decides what agreement means for it,
// because only the site knows: "true vs false" is disagreement for a gate,
// while two adjacent score levels may be agreement for a ranking.
//
// WHAT IS NOT RECORDED: the text that was judged. A ticket message is
// somebody's words, and a comparison ledger is the wrong place to accumulate
// them. `subject_ref` points at the row an operator can open under the
// permissions that row already has.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::Row;
use talaria_state::AppState;

use crate::{Answer, Ask, Judgment, decide};

/// How a site renders its own answers for the ledger, and what it counts as
/// agreement. Declared per call rather than inferred, because the primitive
/// does not settle it: a gate disagrees the moment the booleans differ, and a
/// ranking may not.
pub struct Compare {
    /// Which wired site this is — the ledger's grouping key. Use the harness
    /// or feature id, so a reader can line the rows up against `harness_runs`.
    pub site: &'static str,
    /// The row an operator can go and read: a ticket ref, a conversation id.
    /// None when the subject has no addressable row.
    pub subject_ref: Option<String>,
    /// The answer the site ALREADY produced and is acting on, as text.
    pub baseline: String,
    /// Did the port agree with the baseline? Given the port's answer; the
    /// site decides what "agree" means.
    pub agrees: fn(&Answer, &str) -> bool,
}

/// The standard yes/no agreement: the port's probability crosses 0.5 the same
/// way the baseline's boolean points. The obvious reading for a gate, and
/// deliberately NOT a tunable here — the whole point of shadow mode is to
/// choose the threshold afterwards, from the recorded probabilities, rather
/// than to bake one in before there is any data.
pub fn noul_agrees(answer: &Answer, baseline: &str) -> bool {
    let Some(p) = answer.probability() else {
        return false;
    };
    (p >= 0.5) == (baseline == "true")
}

/// Ask the port the question the site just answered, and record both. Returns
/// immediately and answers nothing: the measurement is detached, and no caller
/// can act on it.
///
/// A no-op when the port is off, which is the default — so wiring a site for
/// shadow costs an unconfigured install one `get_decide_config` read and
/// nothing else.
pub fn compare(state: &AppState, cmp: Compare, ask: Ask) {
    let state = state.clone();
    tokio::spawn(async move {
        let http = talaria_retrieval_http::real_http();
        let judgment = decide(&state, &http, &ask)
            .await
            .and_then(|m| m.into_values().next());
        record(&state.pg, &cmp, judgment.as_ref()).await;
    });
}

/// Write one comparison row. Never returns an error: a measurement that could
/// break the thing it measures is worse than no measurement.
pub async fn record(pg: &PgPool, cmp: &Compare, judgment: Option<&Judgment>) {
    let (port_answer, probability, certainty, calibrated, provider, model, latency, agreed) =
        match judgment {
            Some(j) => (
                Some(render(&j.answer)),
                j.answer.probability(),
                Some(j.certainty()),
                j.calibrated,
                j.provider.clone(),
                j.model.clone(),
                Some(j.latency_ms as i32),
                Some((cmp.agrees)(&j.answer, &cmp.baseline)),
            ),
            // THE PORT ANSWERED NOTHING, AND THAT IS A MEASUREMENT. A site
            // reading only the rows where a judgment arrived would compute an
            // agreement rate over the calls that worked and call it the
            // provider's accuracy. The silences belong in the denominator.
            None => (None, None, None, false, String::new(), None, None, None),
        };
    let res = sqlx::query(
        "insert into decide_shadow \
           (site, subject_ref, baseline, port_answer, probability, certainty, \
            calibrated, provider, model, latency_ms, agreed) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(cmp.site)
    .bind(cmp.subject_ref.as_deref())
    .bind(&cmp.baseline)
    .bind(port_answer)
    .bind(probability.map(|p| p as f32))
    .bind(certainty.map(|c| c as f32))
    .bind(calibrated)
    .bind(provider)
    .bind(model)
    .bind(latency)
    .bind(agreed)
    .execute(pg)
    .await;
    if let Err(e) = res {
        tracing::warn!("[decide] shadow row not recorded for {}: {e}", cmp.site);
    }
}

/// An answer as the ledger stores it. A noul renders as the boolean it would
/// gate on, so a gate's two columns are directly comparable; a choice renders
/// as the chosen id; a score as its position.
fn render(answer: &Answer) -> String {
    match answer {
        Answer::Noul(p) => (*p >= 0.5).to_string(),
        Answer::Choice { id, .. } => id.clone(),
        Answer::Score { position, .. } => format!("{position:.3}"),
    }
}

// ── Reading the numbers back ─────────────────────────────────────────────────

/// One site's shadow record, as the admin panel shows it. The fields a person
/// needs to decide whether to switch a site over, and no others.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SiteShadow {
    pub site: String,
    /// Every comparison attempted, including the ones where the port said
    /// nothing.
    pub compared: i64,
    /// Comparisons where the port actually answered.
    pub answered: i64,
    /// Of those that answered, how many matched the baseline.
    pub agreed: i64,
    /// Of those that answered, how many carried a real distribution.
    pub calibrated: i64,
    /// Median and 99th-percentile latency over the answered calls, ms.
    pub p50_ms: Option<i64>,
    pub p99_ms: Option<i64>,
    /// THE NUMBER A THRESHOLD IS CHOSEN FROM: mean certainty on the
    /// comparisons where the port DISAGREED with the baseline. A provider
    /// whose disagreements are its least certain answers is one a threshold
    /// can filter; a provider that disagrees confidently is one to look at
    /// case by case before trusting.
    pub mean_certainty_when_disagreed: Option<f64>,
    pub provider: Option<String>,
    pub newest: Option<String>,
}

/// Every site with shadow rows, newest activity first. Empty on an install
/// that has never run a comparison, which is every install by default.
pub async fn shadow_report(pg: &PgPool) -> Vec<SiteShadow> {
    let rows = sqlx::query(
        "select site, \
                count(*)::bigint as compared, \
                count(port_answer)::bigint as answered, \
                count(*) filter (where agreed)::bigint as agreed, \
                count(*) filter (where calibrated)::bigint as calibrated, \
                percentile_disc(0.5) within group (order by latency_ms) \
                  filter (where latency_ms is not null) as p50, \
                percentile_disc(0.99) within group (order by latency_ms) \
                  filter (where latency_ms is not null) as p99, \
                avg(certainty) filter (where agreed = false) as disagree_certainty, \
                max(provider) filter (where provider <> '') as provider, \
                max(created_at)::text as newest \
           from decide_shadow \
          group by site \
          order by max(created_at) desc",
    )
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| SiteShadow {
            site: r.try_get("site").unwrap_or_default(),
            compared: r.try_get("compared").unwrap_or(0),
            answered: r.try_get("answered").unwrap_or(0),
            agreed: r.try_get("agreed").unwrap_or(0),
            calibrated: r.try_get("calibrated").unwrap_or(0),
            p50_ms: r
                .try_get::<Option<i32>, _>("p50")
                .ok()
                .flatten()
                .map(i64::from),
            p99_ms: r
                .try_get::<Option<i32>, _>("p99")
                .ok()
                .flatten()
                .map(i64::from),
            mean_certainty_when_disagreed: r
                .try_get::<Option<f64>, _>("disagree_certainty")
                .ok()
                .flatten(),
            provider: r.try_get::<Option<String>, _>("provider").ok().flatten(),
            // Cast to text IN SQL rather than decoded here: this workspace's
            // sqlx has no date feature enabled, and a leaf crate is the wrong
            // place to add one for a display field.
            newest: r.try_get::<Option<String>, _>("newest").ok().flatten(),
        })
        .collect()
}

/// The shadow block the admin GET carries. Separate from the report so an
/// empty install serializes as an empty list rather than as null.
pub fn shadow_value(sites: Vec<SiteShadow>) -> Value {
    serde_json::to_value(sites).unwrap_or_else(|_| Value::Array(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_noul_renders_as_the_boolean_a_gate_would_act_on() {
        assert_eq!(render(&Answer::Noul(0.9)), "true");
        assert_eq!(render(&Answer::Noul(0.1)), "false");
        // THE BOUNDARY IS INCLUSIVE AND MATCHES `noul_agrees`, because a row
        // whose rendering and whose `agreed` disagreed about p=0.5 would be
        // unreadable.
        assert_eq!(render(&Answer::Noul(0.5)), "true");
        assert!(noul_agrees(&Answer::Noul(0.5), "true"));
    }

    #[test]
    fn noul_agreement_compares_the_side_of_the_coin_flip_not_the_magnitude() {
        // A weak yes still agrees with a baseline of true: shadow mode records
        // the disagreement RATE, and the threshold that might reject a weak
        // answer is chosen later, from `certainty`.
        assert!(noul_agrees(&Answer::Noul(0.51), "true"));
        assert!(!noul_agrees(&Answer::Noul(0.49), "true"));
        assert!(noul_agrees(&Answer::Noul(0.49), "false"));
        assert!(!noul_agrees(&Answer::Noul(0.99), "false"));
    }

    #[test]
    fn a_non_noul_answer_never_agrees_through_the_noul_comparator() {
        // Guards against a site wiring a choice question with the gate's
        // comparator and silently recording every row as a disagreement
        // without anybody noticing it was the wrong function.
        let c = Answer::Choice {
            id: "a".into(),
            probabilities: BTreeMap::new(),
            confidence: 0.9,
        };
        assert!(!noul_agrees(&c, "a"));
    }

    #[test]
    fn a_choice_renders_as_the_chosen_id_and_a_score_as_its_position() {
        let c = Answer::Choice {
            id: "technical".into(),
            probabilities: BTreeMap::new(),
            confidence: 0.8,
        };
        assert_eq!(render(&c), "technical");
        let s = Answer::Score {
            position: 1.25,
            probabilities: vec![0.0, 0.75, 0.25],
            confidence: 0.75,
        };
        assert_eq!(render(&s), "1.250");
    }

    #[test]
    fn an_empty_report_serializes_as_a_list_so_a_panel_can_iterate_it() {
        assert_eq!(shadow_value(Vec::new()), serde_json::json!([]));
    }

    #[test]
    fn the_report_shape_names_both_denominators_because_silences_count() {
        // `compared` vs `answered` is the distinction that stops an agreement
        // rate being computed over only the calls that worked.
        let v = shadow_value(vec![SiteShadow {
            site: "ticket-relevance".into(),
            compared: 10,
            answered: 7,
            agreed: 6,
            calibrated: 7,
            p50_ms: Some(120),
            p99_ms: Some(480),
            mean_certainty_when_disagreed: Some(0.21),
            provider: Some("jev".into()),
            newest: None,
        }]);
        assert_eq!(v[0]["compared"], serde_json::json!(10));
        assert_eq!(v[0]["answered"], serde_json::json!(7));
        assert_eq!(v[0]["meanCertaintyWhenDisagreed"], serde_json::json!(0.21));
    }
}
