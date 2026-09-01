// HARNESS HEALTH — which of OUR fixtures are broken, read across every model.
//
// THE QUESTION THIS ANSWERS, and nothing else on the page can. A red cell tells
// an admin a model failed a fixture. It cannot tell them whether the fixture is
// right. Those are different questions with different owners, and for a month
// the only way to tell them apart was for somebody to notice the same assertion
// failing on a second model and go and read it — which is how every fixture
// rewritten this round was found, one at a time, by hand.
//
// A FIXTURE THAT FAILS ON EVERY MODEL IS OURS. Not certainly — a genuinely hard
// task can defeat a shortlist — but it is the strongest signal available without
// reading transcripts, and it is cheap: the archive already holds every case of
// every run. The evidence for it is on the record already:
//
//     work-session · hands a finished ticket to review    failed on gemma AND deepseek
//     work-session · ends with the status line            failed on gemma AND deepseek
//     workbench · runs the tests before it calls it done   gapped on gemma AND deepseek
//
// All three were the turn budget cutting the session off mid-work. Nothing on
// the fitness page said so; it took a hand-written database query.
//
// WHAT IT DELIBERATELY DOES NOT DO: judge. It counts, groups the reasons
// verbatim, and says how many models agreed. The reading — "this is a hard task"
// versus "this assertion is wrong" — is a human's, and the panel gives them the
// sentences to make it with.

use serde::Serialize;

use crate::fitness::evals::EvalCaseScore;
use crate::harness::define::EvalBand;

/// One archived run, reduced to what this file needs.
pub struct HealthInput<'a> {
    pub model: &'a str,
    pub cases: &'a [EvalCaseScore],
}

/// How suspicious a fixture is, in the only three readings the counts support —
/// plus the fourth that says the counts support nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Suspicion {
    /// Every model that ran it came back wrong. Read the fixture first.
    Ours,
    /// More than half, but not all. Could be a hard task, could be a fixture
    /// that only the strongest model satisfies — worth a look either way.
    Shared,
    /// One model of several. That is what a fitness suite is FOR.
    Model,
    /// Fewer than two models have run it, so nothing can be concluded. Said out
    /// loud rather than defaulting to 'model', because "no evidence" and "the
    /// model's fault" are the two readings this whole file exists to separate.
    Unknown,
}

impl Suspicion {
    /// Sort rank, worst reading first.
    fn rank(self) -> u8 {
        match self {
            Suspicion::Ours => 0,
            Suspicion::Shared => 1,
            Suspicion::Model => 2,
            Suspicion::Unknown => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureHealth {
    pub harness: String,
    pub case: String,
    pub band: EvalBand,
    /// Models whose archived run recorded a VERDICT on this fixture — a skip is
    /// not a verdict and is counted separately.
    pub tested: i64,
    pub failed: i64,
    /// OUR gap, already: the fixture said it could not fairly ask its question.
    pub gapped: i64,
    pub timed_out: i64,
    /// The provider never let us ask, or the candidate could not be tested.
    pub unmeasured: i64,
    pub suspicion: Suspicion,
    /// Distinct failure sentences, commonest first, with who saw each. The
    /// sentences are the point: two models failing for the SAME stated reason is
    /// a much stronger signal than two models failing.
    pub reasons: Vec<HealthReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReason {
    pub reason: String,
    pub models: Vec<String>,
}

/// A verdict was reached — the model answered and the fixture judged it.
fn verdict_reached(c: &EvalCaseScore) -> bool {
    c.skipped.is_none()
}

fn wrong(c: &EvalCaseScore) -> bool {
    c.skipped.is_none()
        && (!c.contract_held
            || c.task == crate::fitness::evals::TaskVerdict::Fail
            || c.timed_out
            || c.error.is_some())
}

/// THE VERDICT-CARRYING SENTENCE, truncated rather than paraphrased. Grouped
/// verbatim up to the cut — a paraphrase here would destroy the signal, which is
/// two models coming back with the SAME words.
fn sentence_of(s: &str) -> String {
    // 240 characters, measured the way the TS `slice` did: by unicode scalar,
    // never splitting a surrogate pair the way a byte cut would.
    s.chars().take(240).collect()
}

#[derive(Debug, Clone)]
struct Accum {
    health: FixtureHealth,
    reason_map: Vec<(String, Vec<String>)>,
}

impl Accum {
    fn note(&mut self, reason: String, model: &str) {
        // Appended, not deduped — the TS `set(reason, [...(get(reason) ?? []), run.model])`
        // spells the same thing, and the count is the signal.
        if let Some((_, models)) = self.reason_map.iter_mut().find(|(r, _)| *r == reason) {
            models.push(model.to_string());
        } else {
            self.reason_map.push((reason, vec![model.to_string()]));
        }
    }
}

/// Every fixture in the archive, with what happened to it across models.
///
/// PURE over the records it is given, so the panel and a test see the same
/// arithmetic. Sorted worst first: the fixtures every model failed, then the
/// ones most models failed, then the rest — which is the order somebody
/// debugging their own harness wants to read.
pub fn harness_health(runs: &[HealthInput<'_>]) -> Vec<FixtureHealth> {
    let mut by: Vec<(String, Accum)> = Vec::new();

    for run in runs {
        for c in run.cases {
            let key = format!("{}::{}", c.harness, c.case);
            let at = match by.iter_mut().find(|(k, _)| *k == key) {
                Some((_, a)) => a,
                None => {
                    by.push((
                        key.clone(),
                        Accum {
                            health: FixtureHealth {
                                harness: c.harness.clone(),
                                case: c.case.clone(),
                                band: c.band,
                                tested: 0,
                                failed: 0,
                                gapped: 0,
                                timed_out: 0,
                                unmeasured: 0,
                                suspicion: Suspicion::Unknown,
                                reasons: Vec::new(),
                            },
                            reason_map: Vec::new(),
                        },
                    ));
                    &mut by.last_mut().expect("just pushed").1
                }
            };

            if !verdict_reached(c) {
                at.health.unmeasured += 1;
                continue;
            }
            at.health.tested += 1;
            if c.timed_out {
                at.health.timed_out += 1;
            }
            // A GAP IS NOT A FAILURE, and counting it as one was making this view
            // report our own broken fixtures as models getting things wrong.
            // `failed` means THE MODEL GOT IT WRONG; `gapped` means the fixture
            // could not fairly ask. They are added together nowhere —
            // `suspicion` reads both. This is the same discipline score.rs
            // applies when it excludes gaps from `task_score`.
            if let Some(gap) = &c.gap {
                at.health.gapped += 1;
                at.note(sentence_of(gap), run.model);
                continue;
            }
            if wrong(c) {
                at.health.failed += 1;
                // The fixture's own sentence, or the runner's.
                let reason = c
                    .task_error
                    .clone()
                    .or_else(|| c.error.clone())
                    .unwrap_or_else(|| "no reason recorded".to_string());
                at.note(sentence_of(&reason), run.model);
            }
        }
    }

    let mut out: Vec<FixtureHealth> = by
        .into_iter()
        .map(|(_, mut a)| {
            // Commonest first — the sentence the most models used is the one to
            // read first.
            a.reason_map.sort_by(|x, y| y.1.len().cmp(&x.1.len()));
            a.health.reasons = a
                .reason_map
                .into_iter()
                .map(|(reason, models)| HealthReason { reason, models })
                .collect();
            // A GAP IS OURS BY CONSTRUCTION, whatever the counts say: the fixture
            // itself reported that it could not fairly ask its question. One is
            // enough.
            a.health.suspicion = if a.health.gapped > 0 {
                Suspicion::Ours
            } else if a.health.tested < 2 {
                Suspicion::Unknown
            } else if a.health.failed == a.health.tested {
                Suspicion::Ours
            } else if a.health.failed * 2 > a.health.tested {
                Suspicion::Shared
            } else {
                Suspicion::Model
            };
            a.health
        })
        .collect();

    out.retain(|f| f.failed > 0 || f.gapped > 0 || f.unmeasured > 0);
    out.sort_by(|a, b| {
        a.suspicion
            .rank()
            .cmp(&b.suspicion.rank())
            .then_with(|| b.failed.cmp(&a.failed))
            .then_with(|| a.harness.cmp(&b.harness))
    });
    out
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSummary {
    /// Models whose archives were read. Two is the minimum for any conclusion.
    pub models: Vec<String>,
    pub fixtures: Vec<FixtureHealth>,
    /// Fixtures every model that ran them got wrong. THE NUMBER: it is a count
    /// of our own bugs, and it should be worked to zero rather than explained.
    pub ours: usize,
    pub shared: usize,
}

pub fn summarize(runs: &[HealthInput<'_>]) -> HealthSummary {
    let fixtures = harness_health(runs);
    HealthSummary {
        models: runs.iter().map(|r| r.model.to_string()).collect(),
        ours: fixtures.iter().filter(|f| f.suspicion == Suspicion::Ours).count(),
        shared: fixtures.iter().filter(|f| f.suspicion == Suspicion::Shared).count(),
        fixtures,
    }
}

#[cfg(test)]
mod tests {
    // Ported one-for-one from health.test.ts (11 its).
    //
    // THE ONE THING THIS FILE MUST NOT GET WRONG: calling a fixture ours when
    // it is the model's, or the model's when it is ours. Both are worse than
    // saying nothing — the first sends somebody to rewrite a working assertion,
    // and the second is the status quo this report exists to replace.
    use super::*;
    use crate::fitness::evals::TaskVerdict;

    fn c(over: impl FnOnce(&mut EvalCaseScore)) -> EvalCaseScore {
        let mut case = EvalCaseScore {
            harness: "h".to_string(),
            case: "one".to_string(),
            band: EvalBand::Standard,
            skipped: None,
            contract_held: true,
            first_pass: true,
            repairs: 0,
            answered: true,
            task: TaskVerdict::Pass,
            task_error: None,
            gap: None,
            findings: 0,
            latency_ms: 10,
            started_at: "2026-08-01T00:00:00.000Z".to_string(),
            wall_ms: 10,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            estimated: false,
            timed_out: false,
            optimistic: false,
            error: None,
            prompt: None,
            raw: None,
            turns: None,
            calls: None,
            upstream: None,
        };
        over(&mut case);
        case
    }

    fn run<'a>(model: &'a str, cases: &'a [EvalCaseScore]) -> HealthInput<'a> {
        HealthInput { model, cases }
    }

    #[test]
    fn calls_a_fixture_ours_when_every_model_that_ran_it_got_it_wrong() {
        // The signal that found the turn-budget bug: `work-session · hands a
        // finished ticket to review` failing on gemma AND deepseek. Two models
        // with nothing in common failing the same assertion for the same stated
        // reason is the assertion, not the models.
        let gemma = vec![c(|x| {
            x.task = TaskVerdict::Fail;
            x.task_error = Some("never handed the finished work to review".into());
        })];
        let deepseek = vec![c(|x| {
            x.task = TaskVerdict::Fail;
            x.task_error = Some("never handed the finished work to review".into());
        })];
        let out = harness_health(&[run("gemma", &gemma), run("deepseek", &deepseek)]);
        let f = &out[0];
        assert_eq!((f.harness.as_str(), f.case.as_str()), ("h", "one"));
        assert_eq!((f.tested, f.failed), (2, 2));
        assert_eq!(f.suspicion, Suspicion::Ours);
        // GROUPED VERBATIM, because two models failing for the SAME words is a
        // much stronger signal than two models failing.
        assert_eq!(
            f.reasons,
            vec![HealthReason {
                reason: "never handed the finished work to review".to_string(),
                models: vec!["gemma".to_string(), "deepseek".to_string()],
            }]
        );
    }

    #[test]
    fn calls_it_the_model_when_one_of_several_got_it_wrong() {
        let gemma = vec![c(|x| {
            x.task = TaskVerdict::Fail;
            x.task_error = Some("lost the decision".into());
        })];
        let clean = vec![c(|_| {})];
        let out = harness_health(&[run("gemma", &gemma), run("deepseek", &clean), run("claude", &clean)]);
        assert_eq!((out[0].tested, out[0].failed), (3, 1));
        assert_eq!(out[0].suspicion, Suspicion::Model);
    }

    #[test]
    fn calls_it_shared_when_most_but_not_all_got_it_wrong() {
        let a = vec![c(|x| {
            x.task = TaskVerdict::Fail;
            x.task_error = Some("x".into());
        })];
        let b = vec![c(|x| {
            x.task = TaskVerdict::Fail;
            x.task_error = Some("y".into());
        })];
        let clean = vec![c(|_| {})];
        let out = harness_health(&[run("a", &a), run("b", &b), run("c", &clean)]);
        assert_eq!(out[0].suspicion, Suspicion::Shared);
    }

    #[test]
    fn refuses_to_conclude_anything_from_one_model() {
        // "No evidence" and "the model's fault" are the two readings this whole
        // file exists to separate, so a single run says `unknown` rather than
        // defaulting to blaming the candidate.
        let only = vec![c(|x| {
            x.task = TaskVerdict::Fail;
            x.task_error = Some("x".into());
        })];
        let out = harness_health(&[run("only", &only)]);
        assert_eq!((out[0].tested, out[0].failed), (1, 1));
        assert_eq!(out[0].suspicion, Suspicion::Unknown);
    }

    #[test]
    fn calls_a_gap_ours_on_the_strength_of_one_model_alone() {
        // A gap is the fixture itself reporting that it could not fairly ask
        // its question. That is a statement about the harness, and no amount of
        // corroboration makes it more or less true.
        let gapped = vec![c(|x| {
            x.gap = Some("the turn budget ran out".into());
            x.task = TaskVerdict::Unscored;
        })];
        let clean = vec![c(|_| {})];
        let out = harness_health(&[run("only", &gapped), run("other", &clean)]);
        assert_eq!(out[0].gapped, 1);
        assert_eq!(out[0].suspicion, Suspicion::Ours);
    }

    #[test]
    fn does_not_count_a_gap_as_a_model_failure() {
        // THE BUG THIS PINS. `failed` fed the headline "133 of 643
        // fixture-runs failed", and gaps were in it — so our own broken
        // fixtures were being reported as models getting things wrong, in the
        // one view whose entire job is telling those two apart. `failed` means
        // THE MODEL GOT IT WRONG. `gapped` means the fixture could not fairly
        // ask. They are never added.
        let gapped = vec![c(|x| {
            x.gap = Some("search returned nothing citable".into());
            x.task = TaskVerdict::Unscored;
        })];
        let failed = vec![c(|x| {
            x.task = TaskVerdict::Fail;
            x.task_error = Some("wrong answer".into());
        })];
        let out = harness_health(&[run("a", &gapped), run("b", &failed)]);
        let f = &out[0];
        assert_eq!((f.tested, f.gapped, f.failed), (2, 1, 1));
        // And the gap's sentence still reaches the reader — it is the most
        // actionable line in the view, because it is the one we can go and fix.
        assert!(f.reasons.iter().any(|r| r.reason.contains("nothing citable")));
    }

    #[test]
    fn reports_a_fixture_that_only_ever_gapped_rather_than_dropping_it() {
        // A fixture no model could fairly be asked is the most broken thing in
        // the registry. Filtering on `failed > 0` hid exactly those.
        let a = vec![c(|x| {
            x.gap = Some("the turn budget ran out".into());
            x.task = TaskVerdict::Unscored;
        })];
        let b = vec![c(|x| {
            x.gap = Some("the turn budget ran out".into());
            x.task = TaskVerdict::Unscored;
        })];
        let out = harness_health(&[run("a", &a), run("b", &b)]);
        assert_eq!((out[0].failed, out[0].gapped), (0, 2));
        assert_eq!(out[0].suspicion, Suspicion::Ours);
    }

    #[test]
    fn counts_a_case_the_sweep_could_not_measure_apart_from_one_it_judged() {
        // A skip is not a verdict. Counting it as a failure would blame a model
        // for a harness its transport cannot drive, or for a provider that was
        // busy.
        let skipped = |why: &str| {
            c(|x| x.skipped = Some(why.to_string()))
        };
        let a = vec![skipped("no tool loop on this candidate")];
        let b = vec![skipped("rate limits on every attempt")];
        let out = harness_health(&[run("a", &a), run("b", &b)]);
        let f = &out[0];
        assert_eq!((f.tested, f.failed, f.unmeasured), (0, 0, 2));
        assert_eq!(f.suspicion, Suspicion::Unknown);
    }

    #[test]
    fn leaves_the_fixtures_that_always_pass_out_of_the_report_entirely() {
        // This is a list of things to fix. A green fixture on it is noise, and
        // 247 of them would bury the four that matter.
        let clean = vec![c(|_| {})];
        assert!(harness_health(&[run("a", &clean), run("b", &clean)]).is_empty());
    }

    #[test]
    fn sorts_ours_first_then_shared_then_the_model() {
        let a = vec![
            c(|x| {
                x.case = "model-fault".into();
                x.task = TaskVerdict::Fail;
                x.task_error = Some("x".into());
            }),
            c(|x| {
                x.case = "everyone".into();
                x.task = TaskVerdict::Fail;
                x.task_error = Some("y".into());
            }),
        ];
        let b = vec![
            c(|x| x.case = "model-fault".into()),
            c(|x| {
                x.case = "everyone".into();
                x.task = TaskVerdict::Fail;
                x.task_error = Some("y".into());
            }),
        ];
        let out = harness_health(&[run("a", &a), run("b", &b)]);
        let names: Vec<&str> = out.iter().map(|f| f.case.as_str()).collect();
        assert_eq!(names, ["everyone", "model-fault"]);
    }

    #[test]
    fn summarize_counts_our_own_bugs_as_a_number_to_work_to_zero() {
        let a = vec![
            c(|x| {
                x.case = "ours".into();
                x.task = TaskVerdict::Fail;
                x.task_error = Some("x".into());
            }),
            c(|x| {
                x.case = "theirs".into();
                x.task = TaskVerdict::Fail;
                x.task_error = Some("z".into());
            }),
        ];
        let b = vec![
            c(|x| {
                x.case = "ours".into();
                x.task = TaskVerdict::Fail;
                x.task_error = Some("x".into());
            }),
            c(|x| x.case = "theirs".into()),
        ];
        let out = summarize(&[run("a", &a), run("b", &b)]);
        assert_eq!(out.models, vec!["a".to_string(), "b".to_string()]);
        assert_eq!((out.ours, out.shared), (1, 0));
    }
}
