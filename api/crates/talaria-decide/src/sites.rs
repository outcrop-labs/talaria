// THE SITE CENSUS AND THE SWITCH — every place in Talaria that asks a decision
// model anything, what it does with the answer, where it is turned on, and the
// threshold it acts at.
//
// WHY THIS EXISTS. Shadow mode's whole argument is that a site switches over
// when its own numbers say so, not when the port works. But until this module
// the switch was a `const f64` in whichever crate held the call site —
// `SEMANTIC_FLOOR` in talaria-workflows, `ALIGN_FLOOR` in talaria-gaps — so
// reading the ledger and acting on it were separated by a pull request. An
// operator who can see that a site agreed with the existing code on 94% of 300
// answers and cannot do anything about it has been given a dashboard, not a
// feature.
//
// So: the floors move here, behind one sparse `app_settings` row, beside the
// ledger that justifies them. The constants become DEFAULTS — each with the
// cost asymmetry that chose it written down next to it, because that asymmetry
// is the only honest way to pick a threshold before there are numbers, and
// because an operator moving one should be able to see what they are trading.
//
// EVERY SITE IS LISTED, including the ones this panel cannot switch. A site
// whose switch lives on another surface (the guardrail rule toggle, the rerank
// provider picker) says so and names it; a site that only measures says that.
// The census is the point: "where is a decision model being asked something"
// must have one complete answer, or the answer is "nobody is sure".

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_gateway::settings::{get_setting_hot, set_setting};

use crate::Judgment;

// ── The census ───────────────────────────────────────────────────────────────

pub struct SiteDef {
    /// The ledger's grouping key, and the stored row's field name. Matches
    /// `shadow::Compare::site` exactly where the site records comparisons.
    pub id: &'static str,
    pub label: &'static str,
    /// Which primitive this site asks for — `noul`, `choice` or `score`. A
    /// provider that does not serve it refuses before a request is built, so
    /// this is also what tells an operator why a site is quiet.
    pub primitive: &'static str,
    /// What changes when this site acts. One sentence, read before flipping it.
    pub acts: &'static str,
    /// Does this site act with no operator decision at all? `false` means it
    /// measures until somebody turns it on.
    pub default_on: bool,
    /// Can this panel switch it? `None` means yes. `Some(where)` names the
    /// surface that owns the switch instead — a second spelling of the same
    /// switch is how the two come to disagree.
    pub switch_lives_at: Option<&'static str>,
    pub default_floor: f64,
    /// WHAT THE FLOOR IS MEASURED AGAINST, and the two are different
    /// questions — conflating them is a silent behaviour change in both
    /// directions at once.
    ///
    ///   `FLOOR_LEAN`       the probability of YES must clear it. A
    ///                      one-directional question, where a confident NO
    ///                      means "do nothing", not "do the opposite".
    ///   `FLOOR_CERTAINTY`  distance from the middle must clear it. A
    ///                      two-sided question, where a confident no is as
    ///                      actionable as a confident yes — and the only
    ///                      reading a Choice or a Score has, since neither
    ///                      has a "probability of yes" at all.
    pub reads: &'static str,
    /// WHY that floor. The cost asymmetry, in the site's own terms.
    pub floor_why: &'static str,
}

/// The probability of yes must clear the floor.
pub const FLOOR_LEAN: &str = "lean";
/// Distance from the middle must clear the floor.
pub const FLOOR_CERTAINTY: &str = "certainty";

pub const DECIDE_SITES: &[SiteDef] = &[
    // THE CASCADE, and the one site whose switch changes what gets spent. With
    // it off the LLM gate runs and the port is asked the same question with
    // nobody listening — double cost, which is the price of the measurement
    // and a reason not to leave it there forever. With it on, a CONFIDENT
    // answer is taken and the harness turn is skipped; an uncertain one falls
    // through to the harness, which is exactly where an expensive judge earns
    // its keep.
    SiteDef {
        id: "ticket-relevance",
        label: "Ticket-thread gate",
        primitive: "noul",
        acts: "Decides whether the assigned agent answers a message in a ticket room. When it answers confidently the LLM gate turn is skipped entirely; when it does not, the harness still runs.",
        default_on: false,
        switch_lives_at: None,
        default_floor: 0.70,
        reads: FLOOR_CERTAINTY,
        // 0.70 certainty is p ≥ 0.85 or p ≤ 0.15 — the gate acts only at the
        // ends and falls open through the whole middle, which preserves the
        // pre-port behaviour exactly ("when genuinely uncertain, answer true").
        floor_why: "The gate fails open by construction: an unanswered human message costs more than an unneeded reply. So it acts only at the ends of the range and leaves the uncertain middle to the harness — raising this spends more on the harness, lowering it lets a weaker judgment silence a message.",
    },
    SiteDef {
        id: "workflow-match",
        label: "Workflow matching",
        primitive: "noul",
        acts: "Adds workflows the keyword match missed to a ticket's delivery. Add-only — a keyword match is never dropped, so the worst case is an agent handed a skill it did not need.",
        default_on: true,
        switch_lives_at: None,
        default_floor: 0.75,
        reads: FLOOR_LEAN,
        floor_why: "No asymmetry to lean on: a workflow carries skills and toolkits into an agent's session, so a weak yes is not worth acting on. Lower this and agents collect workflows they did not need; raise it and near-miss tickets keep missing.",
    },
    SiteDef {
        id: "gap-align",
        label: "Capability-gap dedup",
        primitive: "choice",
        acts: "Merges a differently-worded report of a gap already filed into that row, bumping its seen_count instead of opening a duplicate. An exact slug hit never asks anything.",
        default_on: true,
        switch_lives_at: None,
        default_floor: 0.85,
        reads: FLOOR_CERTAINTY,
        floor_why: "A wrong merge HIDES a real gap behind someone else's row, and seen_count is what ranks the Suggested queue — so this is the least forgiving site on the list. A wrong split is merely a duplicate an admin can see.",
    },
    // Measured only, and the measurement has its own consent switch because
    // it costs a question per offered tool on every turn.
    SiteDef {
        id: "tool-prune",
        label: "Tool-offer pruning",
        primitive: "noul",
        acts: "Nothing yet. Judges the tools each agent turn was offered against the ones it called, so the ledger can answer whether pruning would have broken the turn. No request has ever been pruned.",
        default_on: false,
        switch_lives_at: Some("the \"Measure tool-offer pruning\" switch above"),
        default_floor: 0.15,
        reads: FLOOR_LEAN,
        floor_why: "A keep floor, not an act floor, and deliberately low: dropping a tool the turn needed breaks it, while keeping one it did not costs a few tokens of prompt. Read the ledger before trusting any number here.",
    },
    // Declared so the census is complete. Its switch is the guard's own
    // per-rule toggle, and its record is `guard_findings` grouped by
    // `check_type` rather than the shadow ledger — a finding IS the output, so
    // there is no baseline to compare against.
    SiteDef {
        id: "guard-semantic",
        label: "Semantic guardrails",
        primitive: "noul",
        acts: "Files a guard finding with the model's own calibrated probability as its confidence — the field min_confidence has been comparing against developer-typed constants since it shipped. It cannot annotate or redact: the pass runs after the completion has gone back to the caller.",
        default_on: false,
        switch_lives_at: Some("the per-rule toggles on Guardrails, plus that panel's mode ladder"),
        default_floor: 0.50,
        reads: FLOOR_LEAN,
        floor_why: "The guard's own min_confidence, floored at 0.5. Findings land in guard_findings whatever the mode; Observe does not disclose them to the reader, which is what makes the rule safe to run before anyone knows its false-positive rate.",
    },
    SiteDef {
        id: "rerank",
        label: "Retrieval reranking",
        primitive: "score",
        acts: "Reorders retrieval candidates by judged relevance to the query, as the ninth provider in the rerank registry. A failure falls back to vector order and never breaks search.",
        default_on: false,
        switch_lives_at: Some("the reranker provider picker on Retrieval — choose \"decide\""),
        default_floor: 0.0,
        reads: FLOOR_LEAN,
        floor_why: "No threshold: a Score is consumed as a position, not gated as a verdict, so there is nothing to be confident enough about. The ordering is the answer.",
    },
];

pub fn def_of(id: &str) -> Option<&'static SiteDef> {
    DECIDE_SITES.iter().find(|s| s.id == id)
}

// ── The stored row ───────────────────────────────────────────────────────────

const KEY: &str = "decide_sites";

/// What a site does right now: whether it acts, and the certainty it acts at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteGate {
    pub on: bool,
    pub floor: f64,
}

/// A SPARSE OVERRIDE, not a mirror of the census. An absent site — and an
/// absent field on a present site — takes the registered default, so a site
/// added later arrives with its own reasoning rather than with whatever the
/// last write happened to contain.
fn gate_from(stored: &Value, def: &SiteDef) -> SiteGate {
    let row = stored.get(def.id);
    SiteGate {
        on: row
            .and_then(|r| r.get("on"))
            .and_then(Value::as_bool)
            .unwrap_or(def.default_on),
        floor: row
            .and_then(|r| r.get("floor"))
            .and_then(Value::as_f64)
            .filter(|f| f.is_finite() && (0.0..=1.0).contains(f))
            .unwrap_or(def.default_floor),
    }
}

async fn stored(pg: &PgPool) -> Value {
    // Hot-read: the ticket gate reads this per message and the tool pass per
    // turn. Legal under that window's law — this key is written ONLY by
    // `set_site` below, through `set_setting`, which drops the cached key.
    get_setting_hot(pg, KEY, json!({})).await
}

pub async fn gate_of(pg: &PgPool, site: &str) -> SiteGate {
    match def_of(site) {
        // An id with no entry in the census cannot be switched on by a typo in
        // the stored row.
        None => SiteGate {
            on: false,
            floor: 1.0,
        },
        Some(def) => gate_from(&stored(pg).await, def),
    }
}

/// THE CALL-SITE DOOR. The judgment this site may act on, or `None` — which
/// means "run your own path", the same thing `decide` returning `None` means.
///
/// Every site records to the ledger BEFORE calling this, including the answers
/// below the floor, because an agreement rate computed over only the acted-on
/// half is not one.
pub async fn acted(pg: &PgPool, site: &str, j: Option<Judgment>) -> Option<Judgment> {
    let def = def_of(site)?;
    acts_on(def, &gate_of(pg, site).await, j)
}

/// The pure predicate, so the boundary is testable without a database.
///
/// Three conditions in every case: the site is switched on, a real
/// distribution stood behind the answer, and the number clears the floor. The
/// THIRD one is read differently per site, and that difference is load-bearing
/// — `workflow-match` asked at `FLOOR_CERTAINTY` would both raise its bar from
/// p ≥ 0.75 to p ≥ 0.875 *and* start pulling a workflow in on a confident NO,
/// because distance from the middle cannot tell the two ends apart. Four of
/// the six sites lean; only the two-sided ones measure certainty.
pub fn acts_on(def: &SiteDef, gate: &SiteGate, j: Option<Judgment>) -> Option<Judgment> {
    if !gate.on {
        return None;
    }
    let j = j?;
    // Uncalibrated never acts, whichever way the floor is read: a fabricated
    // confidence is worse than no confidence, because it looks like evidence.
    if !j.calibrated {
        return None;
    }
    let clears = match def.reads {
        FLOOR_LEAN => j.answer.probability().is_some_and(|p| p >= gate.floor),
        FLOOR_CERTAINTY => j.certainty() >= gate.floor,
        // An unknown reading acts on nothing rather than picking one.
        _ => false,
    };
    clears.then_some(j)
}

/// One site as the admin panel shows it: the census entry, flattened together
/// with what is actually set.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SitePublic {
    pub id: &'static str,
    pub label: &'static str,
    pub primitive: &'static str,
    pub acts: &'static str,
    pub on: bool,
    pub floor: f64,
    pub default_on: bool,
    pub default_floor: f64,
    pub reads: &'static str,
    pub floor_why: &'static str,
    /// `None` when this panel owns the switch.
    pub switch_lives_at: Option<&'static str>,
}

pub async fn sites_public(pg: &PgPool) -> Vec<SitePublic> {
    let stored = stored(pg).await;
    DECIDE_SITES
        .iter()
        .map(|def| {
            let gate = gate_from(&stored, def);
            SitePublic {
                id: def.id,
                label: def.label,
                primitive: def.primitive,
                acts: def.acts,
                on: gate.on,
                floor: gate.floor,
                default_on: def.default_on,
                default_floor: def.default_floor,
                reads: def.reads,
                floor_why: def.floor_why,
                switch_lives_at: def.switch_lives_at,
            }
        })
        .collect()
}

/// Set one site's switch and/or floor. Absent means leave alone; the row stays
/// sparse, so a field set back to its default is REMOVED rather than written —
/// otherwise changing a default later would silently not reach the installs
/// that had accepted it.
pub async fn set_site(
    pg: &PgPool,
    site: &str,
    on: Option<bool>,
    floor: Option<f64>,
) -> Result<Value, String> {
    let def = def_of(site).ok_or_else(|| format!("unknown site \"{site}\""))?;
    if def.switch_lives_at.is_some() {
        return Err(format!(
            "\"{}\" is switched {} — setting it here would be a second spelling of the same switch.",
            def.label,
            def.switch_lives_at.unwrap_or_default()
        ));
    }
    let mut row = match stored(pg).await {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let mut entry = match row.get(site) {
        Some(Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    if let Some(on) = on {
        if on == def.default_on {
            entry.remove("on");
        } else {
            entry.insert("on".into(), json!(on));
        }
    }
    if let Some(f) = floor {
        if !f.is_finite() || !(0.0..=1.0).contains(&f) {
            return Err("a floor is a certainty between 0 and 1".into());
        }
        // Compared at the precision the panel sends, so clicking back to the
        // default clears the override rather than pinning the old number.
        if (f - def.default_floor).abs() < 1e-9 {
            entry.remove("floor");
        } else {
            entry.insert("floor".into(), json!(f));
        }
    }
    if entry.is_empty() {
        row.remove(site);
    } else {
        row.insert(site.into(), Value::Object(entry));
    }
    let v = Value::Object(row);
    set_setting(pg, KEY, &v).await.map_err(|e| e.to_string())?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Answer;

    fn judgment(certainty_from: f64, calibrated: bool) -> Judgment {
        Judgment {
            answer: Answer::Noul(certainty_from),
            calibrated,
            provider: "test".into(),
            model: None,
            latency_ms: 1,
        }
    }

    #[test]
    fn the_census_is_internally_consistent() {
        for s in DECIDE_SITES {
            assert!(!s.id.is_empty() && !s.label.is_empty(), "{}", s.id);
            assert!(
                ["noul", "choice", "score"].contains(&s.primitive),
                "{} asks for unknown primitive {}",
                s.id,
                s.primitive
            );
            assert!(
                (0.0..=1.0).contains(&s.default_floor),
                "{} has an impossible floor",
                s.id
            );
            // A floor with no reasoning beside it is a magic number, which is
            // the thing this module exists to delete.
            assert!(
                s.floor_why.len() > 40,
                "{} does not say why its floor is what it is",
                s.id
            );
            assert!(s.acts.len() > 40, "{} does not say what it does", s.id);
            assert!(
                [FLOOR_LEAN, FLOOR_CERTAINTY].contains(&s.reads),
                "{} does not say how its floor is read",
                s.id
            );
            // A Choice or a Score has no "probability of yes", so a lean floor
            // over one would read `None` and act on nothing — silently.
            if s.primitive != "noul" && s.default_floor > 0.0 {
                assert_eq!(
                    s.reads, FLOOR_CERTAINTY,
                    "{} asks for a {} and cannot lean",
                    s.id, s.primitive
                );
            }
        }
        let ids: Vec<&str> = DECIDE_SITES.iter().map(|s| s.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "two sites share an id");
    }

    #[test]
    fn a_site_switched_elsewhere_cannot_also_be_switched_here() {
        // Two spellings of one switch is how they come to disagree. The
        // census lists those sites so a person can find out where the switch
        // IS; it does not offer a second one.
        let elsewhere: Vec<&str> = DECIDE_SITES
            .iter()
            .filter(|s| s.switch_lives_at.is_some())
            .map(|s| s.id)
            .collect();
        assert!(
            elsewhere.contains(&"rerank") && elsewhere.contains(&"guard-semantic"),
            "{elsewhere:?}"
        );
        for s in DECIDE_SITES.iter().filter(|s| s.switch_lives_at.is_some()) {
            assert!(
                !s.switch_lives_at.unwrap_or_default().is_empty(),
                "{} says its switch is elsewhere without saying where",
                s.id
            );
        }
    }

    #[test]
    fn an_absent_row_and_an_absent_field_both_take_the_registered_default() {
        let def = def_of("workflow-match").expect("in the census");
        // Nothing stored at all.
        assert_eq!(
            gate_from(&json!({}), def),
            SiteGate {
                on: true,
                floor: 0.75
            }
        );
        // Present, but only one field set — the other takes the default.
        assert_eq!(
            gate_from(&json!({"workflow-match": {"on": false}}), def),
            SiteGate {
                on: false,
                floor: 0.75
            }
        );
        assert_eq!(
            gate_from(&json!({"workflow-match": {"floor": 0.9}}), def),
            SiteGate {
                on: true,
                floor: 0.9
            }
        );
        // A nonsense floor is not a floor — it takes the default rather than
        // gating on NaN, which compares false against everything and would
        // silently switch the site off.
        for bad in [json!(1.5), json!(-0.1), json!("high"), json!(null)] {
            assert_eq!(
                gate_from(&json!({"workflow-match": {"floor": bad}}), def).floor,
                0.75,
                "{bad} should not become a floor"
            );
        }
    }

    #[test]
    fn an_unknown_site_id_can_never_be_switched_on_by_a_typo_in_the_row() {
        // `gate_of` resolves through the census, so a stored key naming a site
        // that does not exist reaches nothing — and the fallback is off with
        // an unreachable floor rather than on.
        let def = def_of("workflow-match").expect("in the census");
        assert_eq!(
            acts_on(
                def,
                &SiteGate {
                    on: false,
                    floor: 1.0
                },
                Some(judgment(0.99, true))
            ),
            None
        );
        assert!(def_of("ticket-relavence").is_none(), "a plausible typo");
    }

    #[test]
    fn a_lean_floor_reads_the_probability_of_yes_and_a_confident_no_does_not_act() {
        // THE ONE THAT MATTERS. `workflow-match` is one-directional: a
        // confident NO means "do not pull this workflow in", not "pull in the
        // opposite". Read as certainty, p = 0.01 would clear a 0.75 floor —
        // distance from the middle cannot tell the two ends apart — and the
        // call site, which only checks for Some, would deliver the workflow.
        let def = def_of("workflow-match").expect("in the census");
        assert_eq!(def.reads, FLOOR_LEAN);
        let gate = SiteGate {
            on: true,
            floor: 0.75,
        };
        assert!(
            acts_on(def, &gate, Some(judgment(0.75, true))).is_some(),
            "at the floor"
        );
        assert!(acts_on(def, &gate, Some(judgment(0.99, true))).is_some());
        assert!(
            acts_on(def, &gate, Some(judgment(0.74, true))).is_none(),
            "just under"
        );
        assert!(
            acts_on(def, &gate, Some(judgment(0.01, true))).is_none(),
            "a confident NO must not act on a one-directional question"
        );
        assert!(
            acts_on(def, &gate, Some(judgment(0.99, false))).is_none(),
            "uncalibrated"
        );
    }

    #[test]
    fn a_certainty_floor_acts_at_both_ends_because_a_confident_no_is_the_point() {
        // The ticket gate is two-sided: a confident no is what lets it skip
        // the harness turn, and a confident yes says the same thing the
        // fail-open default already said.
        let def = def_of("ticket-relevance").expect("in the census");
        assert_eq!(def.reads, FLOOR_CERTAINTY);
        let gate = SiteGate {
            on: true,
            floor: 0.70,
        };
        // certainty = 2·|p − 0.5|, so the floor sits at p = 0.85 and p = 0.15.
        assert!(
            acts_on(def, &gate, Some(judgment(0.85, true))).is_some(),
            "confident yes"
        );
        assert!(
            acts_on(def, &gate, Some(judgment(0.15, true))).is_some(),
            "confident no"
        );
        assert!(acts_on(def, &gate, Some(judgment(0.84, true))).is_none());
        assert!(acts_on(def, &gate, Some(judgment(0.16, true))).is_none());
        assert!(
            acts_on(def, &gate, Some(judgment(0.5, true))).is_none(),
            "the middle is where the harness still runs"
        );
        assert!(acts_on(def, &gate, None).is_none());
    }

    #[test]
    fn every_site_in_the_census_keeps_the_floor_semantics_its_call_site_had() {
        // Pinned per site rather than spot-checked, because getting one wrong
        // is invisible: the site keeps answering, just at a different bar or
        // in the wrong direction.
        let expect = [
            ("ticket-relevance", FLOOR_CERTAINTY, 0.70),
            ("workflow-match", FLOOR_LEAN, 0.75),
            ("gap-align", FLOOR_CERTAINTY, 0.85),
            ("tool-prune", FLOOR_LEAN, 0.15),
            ("guard-semantic", FLOOR_LEAN, 0.50),
        ];
        for (id, reads, floor) in expect {
            let d = def_of(id).unwrap_or_else(|| panic!("{id} left the census"));
            assert_eq!(d.reads, reads, "{id} changed how its floor is read");
            assert!(
                (d.default_floor - floor).abs() < 1e-9,
                "{id} default floor moved to {}",
                d.default_floor
            );
        }
        // And the census covers every site the ledger can group by.
        assert_eq!(
            DECIDE_SITES.len(),
            6,
            "a site was added without a census entry"
        );
    }
}
