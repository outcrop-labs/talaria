// THE DECISION — given what the checks have seen, which pair (if any) to
// smoke-test next. Pure: the state and the clock in, one move or none out,
// with the sentence each harness's panel line shows.
//
// A release is a candidate when it is newer than what jobs run, not held,
// not deprecated, and has soaked (seen for `min_age` — each release its own
// clock, from the first check that saw it), or an admin pressed Update now.
// A Pin names the one version, up or down, until Unpin.
//
// COMPATIBILITY FIRST. An Oh My Pi candidate's `engines.bun` must be met by
// the harness's bun. When it is not, the harness's bun moves FIRST — to the
// newest bun release that has soaked and meets that floor, smoke-tested with
// the Oh My Pi jobs run now — and the Oh My Pi move follows on the next step.
// A bun move on its own must still meet the running Oh My Pi's range. A range
// Talaria cannot read is never a yes.

use std::collections::BTreeMap;

use talaria_workbench_harnesses::{OMP_VERSION, OMP_VERSION_BUN_RANGE, Versions};

use crate::Tool;
use crate::semver::{newer, satisfies};
use crate::state::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    /// How long a release soaks before it is adopted.
    pub min_age_ms: i64,
}

/// One pair to smoke-test and, if it passes, adopt. `tool` is the harness
/// whose release is the candidate: the one held if the smoke test fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub to: Versions,
    pub tool: Tool,
    pub version: String,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub mv: Option<Move>,
    /// Per harness slug: why its newest release is not what jobs run.
    pub details: BTreeMap<String, Option<String>>,
}

/// Oh My Pi `v`'s declared `engines.bun`: `Some(None)` when it declares
/// none, `None` when this instance does not know (never checked).
pub fn requires(s: &State, omp: &str) -> Option<Option<String>> {
    if let Some(seen) = s.tool(Tool::Omp).seen.get(omp) {
        return Some(seen.requires_bun.clone());
    }
    (omp == OMP_VERSION).then(|| Some(OMP_VERSION_BUN_RANGE.to_string()))
}

/// Does `bun` meet `range`? Unknown requirements pass (the smoke test runs
/// the pair for real); an unreadable range never does.
fn meets(bun: &str, range: &Option<Option<String>>) -> bool {
    match range {
        None | Some(None) => true,
        Some(Some(r)) => satisfies(bun, r) == Some(true),
    }
}

fn label(t: Tool, v: &str) -> String {
    format!("{} {v}", t.label())
}

/// The newest release of `t` past `cur` that may be adopted now and passes
/// `pred`; and, when there is none, the newest one still soaking (with when
/// it is ready).
fn pick(
    s: &State,
    t: Tool,
    cur: &str,
    now: i64,
    tuning: &Tuning,
    pred: &dyn Fn(&str) -> bool,
) -> (Option<String>, Option<(String, i64)>) {
    let ts = s.tool(t);
    let mut best: Option<String> = None;
    let mut soaking: Option<(String, i64)> = None;
    for (v, seen) in &ts.seen {
        if !newer(v, cur) || ts.held.contains_key(v) || seen.deprecated.is_some() || !pred(v) {
            continue;
        }
        let ready_at = seen.first_seen_at.saturating_add(tuning.min_age_ms);
        if ready_at <= now || ts.forced.as_deref() == Some(v.as_str()) {
            if best.as_deref().is_none_or(|b| newer(v, b)) {
                best = Some(v.clone());
            }
        } else if soaking.as_ref().is_none_or(|(b, _)| newer(v, b)) {
            soaking = Some((v.clone(), ready_at));
        }
    }
    (best, soaking)
}

fn soak_note(t: Tool, soaking: Option<(String, i64)>) -> Option<String> {
    soaking.map(|(v, at)| {
        format!(
            "{} is soaking: it is adopted from {} unless upstream withdraws it",
            label(t, &v),
            talaria_agent_auth::epoch_ms_to_iso(at)
        )
    })
}

/// The target for `t` when an admin pinned it: `Ok(Some(v))` to move to,
/// `Ok(None)` already there, `Err(detail)` a pin that cannot move.
fn pinned(s: &State, t: Tool, cur: &str) -> Option<Result<Option<String>, String>> {
    let ts = s.tool(t);
    let p = ts.pinned?;
    Some(if p == cur {
        Ok(None)
    } else if let Some(h) = ts.held.get(&p) {
        Err(format!(
            "pinned to {}, which is held ({}): allow it again or pin another",
            label(t, &p),
            h.reason.lines().next().unwrap_or("")
        ))
    } else {
        Ok(Some(p))
    })
}

/// Why a release is the next move: an admin's pin, or newer.
fn newer_or_pinned(s: &State, t: Tool, v: &str) -> String {
    if s.tool(t).pinned.as_deref() == Some(v) {
        format!("{} is pinned", label(t, v))
    } else {
        format!("{} is newer", label(t, v))
    }
}

pub fn decide(s: &State, now: i64, tuning: &Tuning) -> Plan {
    let cur = s.running();
    let mut plan = Plan::default();
    fn say(plan: &mut Plan, t: Tool, d: Option<String>) {
        plan.details.insert(t.slug().to_string(), d);
    }
    say(&mut plan, Tool::Bun, None);
    say(&mut plan, Tool::Omp, None);
    let omp_now = requires(s, &cur.omp);
    let target = |plan: &mut Plan, t: Tool, cur: &str| match pinned(s, t, cur) {
        Some(Ok(v)) => v,
        Some(Err(d)) => {
            say(plan, t, Some(d));
            None
        }
        None => {
            let (best, soaking) = pick(s, t, cur, now, tuning, &|_| true);
            if best.is_none() {
                say(plan, t, soak_note(t, soaking));
            }
            best
        }
    };
    let omp_target = target(&mut plan, Tool::Omp, &cur.omp);
    let bun_target = target(&mut plan, Tool::Bun, &cur.bun);

    // 1. An Oh My Pi candidate whose floor is above the harness's bun: that
    //    bun first.
    if let Some(o) = &omp_target {
        let need = requires(s, o);
        if !meets(&cur.bun, &need) {
            let range = need.clone().flatten().unwrap_or_default();
            if range.starts_with("unreadable:") || satisfies("0.0.0", &range).is_none() {
                say(
                    &mut plan,
                    Tool::Omp,
                    Some(format!(
                        "{} declares engines.bun {range:?}, which Talaria cannot read; it is not adopted",
                        label(Tool::Omp, o)
                    )),
                );
            } else if let Some(p) = s.tool(Tool::Bun).pinned {
                say(
                    &mut plan,
                    Tool::Omp,
                    Some(format!(
                        "{} needs bun {range}, but the harness's bun is pinned to {p}",
                        label(Tool::Omp, o)
                    )),
                );
            } else {
                let fits = |b: &str| satisfies(b, &range) == Some(true);
                let (bun, soaking) = pick(s, Tool::Bun, &cur.bun, now, tuning, &fits);
                match bun {
                    Some(b) => {
                        say(
                            &mut plan,
                            Tool::Omp,
                            Some(format!(
                                "{} needs bun {range}: bun {b} is adopted first",
                                label(Tool::Omp, o)
                            )),
                        );
                        // With the Oh My Pi jobs run now — unless that one
                        // will not run on it, and then the two together.
                        plan.mv = Some(if meets(&b, &omp_now) {
                            Move {
                                to: Versions {
                                    bun: b.clone(),
                                    omp: cur.omp.clone(),
                                },
                                tool: Tool::Bun,
                                version: b.clone(),
                                why: format!(
                                    "{} needs bun {range}: {} first",
                                    label(Tool::Omp, o),
                                    label(Tool::Bun, &b)
                                ),
                            }
                        } else {
                            Move {
                                to: Versions {
                                    bun: b.clone(),
                                    omp: o.clone(),
                                },
                                tool: Tool::Omp,
                                version: o.clone(),
                                why: format!(
                                    "{} needs bun {range}, which {} does not run on: both together",
                                    label(Tool::Omp, o),
                                    label(Tool::Omp, &cur.omp)
                                ),
                            }
                        });
                        return plan;
                    }
                    None => {
                        let why = match soaking {
                            Some((v, at)) => format!(
                                "{} needs bun {range}: bun {v} meets it and is adopted first, from {}",
                                label(Tool::Omp, o),
                                talaria_agent_auth::epoch_ms_to_iso(at)
                            ),
                            None => format!(
                                "{} needs bun {range}, and no bun release that meets it has been seen yet; jobs keep bun {}",
                                label(Tool::Omp, o),
                                cur.bun
                            ),
                        };
                        say(&mut plan, Tool::Omp, Some(why));
                    }
                }
            }
        }
    }

    // 2. The harness's bun on its own — only to a bun the running Oh My Pi
    //    declares it runs on.
    if let Some(b) = &bun_target {
        if meets(b, &omp_now) {
            plan.mv = Some(Move {
                to: Versions {
                    bun: b.clone(),
                    omp: cur.omp.clone(),
                },
                tool: Tool::Bun,
                version: b.clone(),
                why: newer_or_pinned(s, Tool::Bun, b),
            });
            return plan;
        }
        say(
            &mut plan,
            Tool::Bun,
            Some(format!(
                "{} is not a bun {} runs on (it declares bun {}); it waits for an Oh My Pi that does",
                label(Tool::Bun, b),
                label(Tool::Omp, &cur.omp),
                omp_now.clone().flatten().unwrap_or_default()
            )),
        );
    }

    // 3. Oh My Pi, on the bun jobs run now.
    if let Some(o) = omp_target
        && meets(&cur.bun, &requires(s, &o))
    {
        plan.mv = Some(Move {
            to: Versions {
                bun: cur.bun.clone(),
                omp: o.clone(),
            },
            tool: Tool::Omp,
            version: o.clone(),
            why: newer_or_pinned(s, Tool::Omp, &o),
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Held, Pair, Seen};

    const H: i64 = 60 * 60 * 1000;
    const DAY: Tuning = Tuning { min_age_ms: 24 * H };

    fn state(bun: &str, omp: &str) -> State {
        State {
            current: Some(Pair {
                bun: bun.into(),
                omp: omp.into(),
                at: 0,
            }),
            ..Default::default()
        }
    }

    fn seen(s: &mut State, t: Tool, v: &str, at: i64, requires: Option<&str>) {
        s.tool_mut(t).seen.insert(
            v.into(),
            Seen {
                first_seen_at: at,
                requires_bun: requires.map(Into::into),
                deprecated: None,
            },
        );
    }

    fn mv(p: &Plan) -> Option<(String, String, Tool)> {
        p.mv.as_ref()
            .map(|m| (m.to.bun.clone(), m.to.omp.clone(), m.tool))
    }

    #[test]
    fn a_newer_release_is_adopted_only_once_it_has_soaked() {
        let mut s = state("1.4.4", "18.15.0");
        seen(&mut s, Tool::Omp, "18.15.0", 0, Some(">=1.4.4"));
        seen(&mut s, Tool::Omp, "18.16.0", 10 * H, Some(">=1.4.4"));
        let p = decide(&s, 20 * H, &DAY);
        assert_eq!(p.mv, None, "seen 10 h ago: soaking");
        assert!(
            p.details["omp"].as_deref().unwrap().contains("soaking"),
            "{p:?}"
        );
        let p = decide(&s, 34 * H, &DAY);
        assert_eq!(mv(&p), Some(("1.4.4".into(), "18.16.0".into(), Tool::Omp)));
        // Update now skips the soak for that version.
        s.tool_mut(Tool::Omp).forced = Some("18.16.0".into());
        assert!(decide(&s, 11 * H, &DAY).mv.is_some());
    }

    #[test]
    fn the_newest_soaked_release_wins_and_a_newer_one_soaking_does_not_reset_it() {
        let mut s = state("1.4.4", "18.14.0");
        seen(&mut s, Tool::Omp, "18.15.0", 0, Some(">=1.4.4"));
        seen(&mut s, Tool::Omp, "18.16.0", 30 * H, Some(">=1.4.4"));
        // 18.16.0 is soaking; 18.15.0 has soaked: adopt 18.15.0 now.
        let p = decide(&s, 31 * H, &DAY);
        assert_eq!(mv(&p).unwrap().1, "18.15.0");
    }

    #[test]
    fn a_held_or_deprecated_release_is_never_a_candidate() {
        let mut s = state("1.4.4", "18.15.0");
        seen(&mut s, Tool::Omp, "18.16.0", 0, Some(">=1.4.4"));
        s.tool_mut(Tool::Omp).held.insert(
            "18.16.0".into(),
            Held {
                reason: "does not start".into(),
                at: 1,
                department: None,
            },
        );
        assert_eq!(decide(&s, 100 * H, &DAY).mv, None);
        s.tool_mut(Tool::Omp).held.clear();
        s.tool_mut(Tool::Omp)
            .seen
            .get_mut("18.16.0")
            .unwrap()
            .deprecated = Some("x".into());
        assert_eq!(decide(&s, 100 * H, &DAY).mv, None);
    }

    #[test]
    fn a_floor_above_the_harness_bun_moves_the_bun_first() {
        // The 28 Oct release: Oh My Pi raised engines.bun to >=1.4.4.
        let mut s = state("1.3.14", "18.13.2");
        seen(&mut s, Tool::Omp, "18.13.2", 0, Some(">=1.3.14"));
        seen(&mut s, Tool::Omp, "18.13.3", 0, Some(">=1.4.4"));
        seen(&mut s, Tool::Bun, "1.4.4", 0, None);
        let p = decide(&s, 100 * H, &DAY);
        assert_eq!(mv(&p), Some(("1.4.4".into(), "18.13.2".into(), Tool::Bun)));
        assert!(p.mv.unwrap().why.contains("needs bun >=1.4.4"));
        // Once that bun is what jobs run, Oh My Pi follows.
        s.current = Some(Pair {
            bun: "1.4.4".into(),
            omp: "18.13.2".into(),
            at: 0,
        });
        assert_eq!(
            mv(&decide(&s, 100 * H, &DAY)),
            Some(("1.4.4".into(), "18.13.3".into(), Tool::Omp))
        );
    }

    #[test]
    fn with_no_bun_that_meets_the_floor_the_release_waits_and_says_why() {
        let mut s = state("1.3.14", "18.13.2");
        seen(&mut s, Tool::Omp, "18.13.2", 0, Some(">=1.3.14"));
        seen(&mut s, Tool::Omp, "18.13.3", 0, Some(">=1.4.4"));
        seen(&mut s, Tool::Bun, "1.4.0", 0, None);
        let p = decide(&s, 100 * H, &DAY);
        // bun 1.4.0 is newer and the running Oh My Pi runs on it: it moves,
        // but it does not meet the candidate's floor, so Oh My Pi waits.
        assert_eq!(mv(&p), Some(("1.4.0".into(), "18.13.2".into(), Tool::Bun)));
        s.current = Some(Pair {
            bun: "1.4.0".into(),
            omp: "18.13.2".into(),
            at: 0,
        });
        let p = decide(&s, 100 * H, &DAY);
        assert_eq!(p.mv, None);
        assert!(
            p.details["omp"]
                .as_deref()
                .unwrap()
                .contains("needs bun >=1.4.4"),
            "{p:?}"
        );
        // A bun pin blocks it too, and says so.
        s.tool_mut(Tool::Bun).pinned = Some("1.4.0".into());
        seen(&mut s, Tool::Bun, "1.4.4", 0, None);
        let p = decide(&s, 100 * H, &DAY);
        assert_eq!(p.mv, None);
        assert!(
            p.details["omp"]
                .as_deref()
                .unwrap()
                .contains("pinned to 1.4.0")
        );
    }

    #[test]
    fn a_bun_the_running_omp_will_not_start_on_is_never_moved_to_on_its_own() {
        // Pinning the harness's bun under the running Oh My Pi's floor would
        // break every new job: it waits, and says why.
        let mut s = state("1.4.4", "18.16.0");
        seen(&mut s, Tool::Omp, "18.16.0", 0, Some(">=1.4.4"));
        s.tool_mut(Tool::Bun).pinned = Some("1.3.14".into());
        let p = decide(&s, 100 * H, &DAY);
        assert_eq!(p.mv, None);
        assert!(
            p.details["bun"]
                .as_deref()
                .unwrap()
                .contains("declares bun >=1.4.4")
        );
        // The bootstrap's own floor is known without a check.
        let mut s = State::default();
        s.tool_mut(Tool::Bun).pinned = Some("1.3.14".into());
        assert_eq!(decide(&s, 0, &DAY).mv, None);
    }

    #[test]
    fn a_pin_moves_up_or_down_and_unknown_floors_go_to_the_smoke_test() {
        let mut s = state("1.4.4", "18.16.0");
        s.tool_mut(Tool::Omp).pinned = Some("18.13.2".into());
        seen(&mut s, Tool::Omp, "18.13.2", 0, Some(">=1.3.14"));
        assert_eq!(
            mv(&decide(&s, 0, &DAY)),
            Some(("1.4.4".into(), "18.13.2".into(), Tool::Omp))
        );
        // An unreadable floor is never a yes.
        let mut s = state("1.4.4", "18.16.0");
        seen(&mut s, Tool::Omp, "18.17.0", 0, Some("unreadable:{}"));
        let p = decide(&s, 100 * H, &DAY);
        assert_eq!(p.mv, None);
        assert!(p.details["omp"].as_deref().unwrap().contains("cannot read"));
    }

    #[test]
    fn nothing_to_do_says_nothing() {
        let s = State::default();
        let p = decide(&s, 0, &DAY);
        assert_eq!(p.mv, None);
        assert_eq!(p.details["omp"], None);
        assert_eq!(p.details["bun"], None);
    }
}
