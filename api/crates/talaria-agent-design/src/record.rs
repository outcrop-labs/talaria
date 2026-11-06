// The design record and its state machine, pure.
//
// One record per person, at most: what they asked for, the chat the next
// refine reads, the validated draft, and where the run stands. Every
// transition is a function from the record as Redis holds it to the record
// that replaces it, so the rules are tested here without a Redis, and the
// store (store.rs) only has to make each replacement atomic.
//
//   (none) ──start──▶ generating ──land──▶ ready ──refine──▶ generating …
//                         │                  ▲
//                         └──fail / stall──▶ error ──start / edit──┘
//
// A run writes only the record that asked for it: `land` and `fail` take the
// run's id and change nothing when the record now names another run (a
// describe that replaced it, a discard that removed it). That is what lets a
// person start over while a slow model is still answering the old question.

use serde::{Deserialize, Serialize};
use talaria_harness_defs::defs::muse::MuseTurn;

/// How many earlier chat turns a landing keeps before adding its own pair:
/// the window the modal kept (`chat.slice(-8)`), so a refine reads the same
/// history it always did.
pub const CHAT_KEEP: usize = 8;

/// How long a record may say `generating` with no run row behind it before a
/// read calls it stalled. The record is written before the run is enqueued
/// (the step writes to it), so a read can land in that gap; a minute is far
/// past any enqueue and far short of anyone's patience.
pub const MISSING_RUN_GRACE_MS: i64 = 60_000;

/// What a person reads when the run ended without an answer: a driver that
/// died on every attempt, or a run cancelled from somewhere else.
pub const STALLED: &str = "The design stopped before it finished. Start it again.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesignStatus {
    Generating,
    Ready,
    Error,
}

/// One starter skill, as the muse drafts it and the hire seeds it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SkillDraft {
    pub name: String,
    pub content: String,
}

/// The agent the review step shows: the muse's validated draft, or the
/// person's edits to it. Field names are the muse agent schema's, so the
/// chat's assistant turn is the same JSON the modal used to send back.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DraftFields {
    pub name: String,
    pub handle: String,
    pub department: String,
    pub role: String,
    pub soul: String,
    #[serde(default)]
    pub skills: Vec<SkillDraft>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDesign {
    pub id: String,
    /// The run answering the current turn — the `runs` row's id. None for a
    /// record no run ever wrote (a role template, a manual start).
    pub run_id: Option<String>,
    pub status: DesignStatus,
    /// The current (or last) turn is a refine of an existing draft.
    pub refining: bool,
    /// The first describe — what the splash and the roster row name.
    pub purpose: String,
    /// The latest instruction: the describe, or the last refine.
    pub instruction: String,
    pub chat: Vec<MuseTurn>,
    /// None until a describe lands. A refine keeps the draft it started from,
    /// so a refine that fails still has a review step to show.
    pub draft: Option<DraftFields>,
    /// The draft a refine started from: the review's "Refine applied" receipt
    /// compares it with `draft`. None after a fresh describe.
    pub previous: Option<DraftFields>,
    pub error: Option<String>,
    /// Epoch ms. The UI counts the elapsed time from here, so a reload or a
    /// second tab shows the same count.
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub updated_at: i64,
    /// The hire this design is spent on, while that hire is in flight. The
    /// design is consumed when the hire finishes and handed back (with the
    /// hire's sentence in `error`) when it fails, so a hire that needs an
    /// edit never loses the design, and a second hire from it is refused.
    #[serde(default)]
    pub hire_run_id: Option<String>,
    /// The hire a handed-back design came back from. The hiring strip's
    /// Retry replays that hire as a new run; this is how the design follows
    /// it (`follow`) instead of offering an agent that now exists.
    #[serde(default)]
    pub released_from: Option<String>,
}

/// A transition the record refuses — each is a 409 the person can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A design is still generating; nothing else may write it but its run.
    Generating,
    /// There is no draft to refine.
    NothingToRefine,
    /// The caller named a design that is no longer the person's (another tab
    /// or device discarded or replaced it).
    Stale,
    /// A hire from this design is in flight.
    Hiring,
}

impl Refusal {
    pub fn sentence(self) -> &'static str {
        match self {
            Refusal::Generating => {
                "an agent design is still generating — wait for it, or discard it"
            }
            Refusal::NothingToRefine => "there is no agent design to refine — describe one first",
            Refusal::Stale => {
                "this agent design was replaced or discarded since it was opened — showing the current one"
            }
            Refusal::Hiring => "an agent is already being hired from this design",
        }
    }
}

/// The answer to a start.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// One is already generating: the caller follows it rather than starting
    /// a second (one design at a time, per person).
    Existing(AgentDesign),
    /// A fresh record, generating, for the run `run_id` names.
    Fresh(AgentDesign),
}

/// A describe. A record that is generating is returned as it is, unless its
/// run can no longer answer (`run`); anything idle (ready, error, a manual
/// start) is replaced, because a fresh describe is a fresh intent.
pub fn start(
    existing: Option<&AgentDesign>,
    purpose: &str,
    now: i64,
    ids: (String, String),
    run: RunSeen,
) -> Start {
    let (design_id, run_id) = ids;
    if let Some(d) = existing
        && d.status == DesignStatus::Generating
        && stalled(d, run, now).is_none()
    {
        return Start::Existing(d.clone());
    }
    Start::Fresh(AgentDesign {
        id: design_id,
        run_id: Some(run_id),
        status: DesignStatus::Generating,
        refining: false,
        purpose: purpose.to_string(),
        instruction: purpose.to_string(),
        chat: Vec::new(),
        draft: None,
        previous: None,
        error: None,
        started_at: Some(now),
        finished_at: None,
        updated_at: now,
        hire_run_id: None,
        released_from: None,
    })
}

/// The design `want` names must be the one held: a caller editing a design
/// another tab replaced is told so rather than writing over the new one. A
/// caller naming none may only start one where none exists.
fn names(existing: Option<&AgentDesign>, want: Option<&str>) -> Result<(), Refusal> {
    match (existing, want) {
        (Some(d), Some(w)) if d.id == w => Ok(()),
        (None, None) => Ok(()),
        _ => Err(Refusal::Stale),
    }
}

/// A refine of the current draft. `current` is the form as the person sees
/// it — hand edits since the last save are part of what the model revises —
/// and replaces the stored draft when given.
pub fn refine(
    existing: Option<&AgentDesign>,
    want: Option<&str>,
    instruction: &str,
    current: Option<DraftFields>,
    now: i64,
    run_id: String,
) -> Result<AgentDesign, Refusal> {
    let Some(d) = existing else {
        return Err(if want.is_some() {
            Refusal::Stale
        } else {
            Refusal::NothingToRefine
        });
    };
    names(existing, want)?;
    if d.status == DesignStatus::Generating {
        return Err(Refusal::Generating);
    }
    let Some(base) = current.or_else(|| d.draft.clone()) else {
        return Err(Refusal::NothingToRefine);
    };
    Ok(AgentDesign {
        run_id: Some(run_id),
        status: DesignStatus::Generating,
        refining: true,
        instruction: instruction.to_string(),
        draft: Some(base.clone()),
        previous: Some(base),
        error: None,
        started_at: Some(now),
        finished_at: None,
        updated_at: now,
        ..d.clone()
    })
}

/// The run's answer. None when the record is no longer this run's — it was
/// replaced, discarded, or already answered — so the answer is dropped.
pub fn land(d: &AgentDesign, run_id: &str, draft: DraftFields, now: i64) -> Option<AgentDesign> {
    if !answering(d, run_id) {
        return None;
    }
    // The turn the next refine reads is the VALIDATED draft, not whatever
    // text the model emitted on the way to it.
    let mut chat = d.chat[d.chat.len().saturating_sub(CHAT_KEEP)..].to_vec();
    chat.push(MuseTurn {
        role: "user".into(),
        content: d.instruction.clone(),
    });
    chat.push(MuseTurn {
        role: "assistant".into(),
        content: serde_json::to_string(&draft).expect("a draft serializes"),
    });
    Some(AgentDesign {
        status: DesignStatus::Ready,
        refining: false,
        chat,
        draft: Some(draft),
        error: None,
        finished_at: Some(now),
        updated_at: now,
        ..d.clone()
    })
}

/// The run's failure, in the sentence the person reads. The draft a refine
/// started from stays, so the review step still has something to show.
pub fn fail(d: &AgentDesign, run_id: &str, error: &str, now: i64) -> Option<AgentDesign> {
    if !answering(d, run_id) {
        return None;
    }
    Some(AgentDesign {
        status: DesignStatus::Error,
        refining: false,
        error: Some(error.to_string()),
        finished_at: Some(now),
        updated_at: now,
        ..d.clone()
    })
}

fn answering(d: &AgentDesign, run_id: &str) -> bool {
    d.status == DesignStatus::Generating && d.run_id.as_deref() == Some(run_id)
}

/// The person's own edits on the review step (or a role template, or a
/// manual start): the draft is theirs now. Refused while a run is writing it.
pub fn edit(
    existing: Option<&AgentDesign>,
    want: Option<&str>,
    fields: DraftFields,
    now: i64,
    design_id: String,
) -> Result<AgentDesign, Refusal> {
    names(existing, want)?;
    match existing {
        Some(d) if d.status == DesignStatus::Generating => Err(Refusal::Generating),
        // The receipt is the refine's, not the person's: a hand edit after a
        // refine ends it, so typing never reads as "Refine applied".
        // An edit also ends a hand-back's link to its failed hire: the strip's
        // Retry replays that hire's old input, and must not spend the edited
        // design on it.
        Some(d) => Ok(AgentDesign {
            status: DesignStatus::Ready,
            error: None,
            draft: Some(fields),
            previous: None,
            released_from: None,
            updated_at: now,
            ..d.clone()
        }),
        None => Ok(AgentDesign {
            id: design_id,
            run_id: None,
            status: DesignStatus::Ready,
            refining: false,
            purpose: String::new(),
            instruction: String::new(),
            chat: Vec::new(),
            draft: Some(fields),
            previous: None,
            error: None,
            started_at: None,
            finished_at: None,
            updated_at: now,
            hire_run_id: None,
            released_from: None,
        }),
    }
}

/// A design spent on a hire, out of the person's slot (store.rs `hires`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    pub design: AgentDesign,
    /// The hire's run, or the retry the hiring strip started from it.
    pub hire_run_id: String,
    pub claimed_at: i64,
    /// Set when the hire failed before the agent existed: the sentence the
    /// design comes back with. A returned design waits here only while the
    /// slot holds another one.
    #[serde(default)]
    pub returned: Option<String>,
    /// The bell row saying it waits was filed: once per hold, so reading
    /// that row never brings it back (nor its toast or push).
    #[serde(default)]
    pub noticed: bool,
}

/// Spend the slot's design on a hire: refused when the caller names another
/// design (or none is held) or it is still generating.
pub fn claim(
    existing: Option<&AgentDesign>,
    want: &str,
    hire_run_id: &str,
    now: i64,
) -> Result<Held, Refusal> {
    names(existing, Some(want))?;
    let d = existing.ok_or(Refusal::Stale)?;
    if d.status == DesignStatus::Generating {
        return Err(Refusal::Generating);
    }
    Ok(Held {
        design: AgentDesign {
            error: None,
            released_from: None,
            hire_run_id: None,
            updated_at: now,
            ..d.clone()
        },
        hire_run_id: hire_run_id.to_string(),
        claimed_at: now,
        returned: None,
        noticed: false,
    })
}

/// What a read saw of the hire a design is spent on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HireSeen {
    Live,
    Done,
    /// Failed or cancelled: whether its create stage had made the agent,
    /// and the hire's own sentence when it has one.
    Failed {
        created: bool,
        why: Option<String>,
    },
    Missing,
}

/// What a settled hire does to its held design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HireOutcome {
    Unchanged,
    /// The agent exists: the design is spent, whatever failed after the
    /// create (a pull, a boot, a Start) — the hiring strip's Retry and the
    /// agent's own Start are how that is finished, never this design.
    Consumed,
    /// The hire failed before the agent existed: the design comes back.
    Returned(String),
}

pub fn settle_held(h: &Held, hire: &HireSeen, now: i64) -> HireOutcome {
    if h.returned.is_some() {
        return HireOutcome::Unchanged;
    }
    match hire {
        HireSeen::Live => HireOutcome::Unchanged,
        HireSeen::Done | HireSeen::Failed { created: true, .. } => HireOutcome::Consumed,
        HireSeen::Failed {
            created: false,
            why,
        } => HireOutcome::Returned(returned_sentence(why.as_deref())),
        HireSeen::Missing if now - h.claimed_at > MISSING_RUN_GRACE_MS => {
            HireOutcome::Returned(returned_sentence(None))
        }
        HireSeen::Missing => HireOutcome::Unchanged,
    }
}

/// Only for a hire that failed before the agent existed, so "create it
/// again" is what then works.
fn returned_sentence(why: Option<&str>) -> String {
    match why.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => format!("The hire did not finish: {w}. Check the fields and create it again."),
        None => "The hire did not finish. Check the fields and create it again.".to_string(),
    }
}

/// The design a returned hold puts back in the slot: ready, saying why.
pub fn hand_back(h: &Held, now: i64) -> AgentDesign {
    AgentDesign {
        status: DesignStatus::Ready,
        error: h.returned.clone(),
        released_from: Some(h.hire_run_id.clone()),
        hire_run_id: None,
        updated_at: now,
        ..h.design.clone()
    }
}

/// The hiring strip retried `failed` as `retry`: a design held on it follows.
pub fn follow_held(h: &Held, failed: &str, retry: &str, now: i64) -> Option<Held> {
    (h.hire_run_id == failed).then(|| Held {
        hire_run_id: retry.to_string(),
        claimed_at: now,
        returned: None,
        noticed: false,
        ..h.clone()
    })
}

/// …and one already handed back into the slot from it (not hired again
/// since) is spent on the retry again.
pub fn follow_back(d: &AgentDesign, failed: &str, retry: &str, now: i64) -> Option<Held> {
    if d.released_from.as_deref() != Some(failed) || d.status == DesignStatus::Generating {
        return None;
    }
    claim(Some(d), &d.id, retry, now).ok()
}

/// What a read saw of the run behind a generating record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSeen {
    /// Queued or running: it will answer.
    Live,
    /// Done, failed or cancelled, and the record still says generating — the
    /// answer will never come.
    Ended,
    /// No row at all.
    Missing,
}

/// A generating record whose run can no longer answer, as the error the
/// person reads: never a spinner forever. None when the record is fine.
pub fn stalled(d: &AgentDesign, run: RunSeen, now: i64) -> Option<AgentDesign> {
    if d.status != DesignStatus::Generating {
        return None;
    }
    let run_id = d.run_id.as_deref()?;
    let gone = match run {
        RunSeen::Live => false,
        RunSeen::Ended => true,
        RunSeen::Missing => now - d.started_at.unwrap_or(d.updated_at) > MISSING_RUN_GRACE_MS,
    };
    if !gone {
        return None;
    }
    fail(d, run_id, STALLED, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(name: &str) -> DraftFields {
        DraftFields {
            name: name.into(),
            handle: name.to_lowercase(),
            department: "research".into(),
            role: "Analyst".into(),
            soul: "A careful analyst.".into(),
            skills: vec![SkillDraft {
                name: "weekly-digest".into(),
                content: "# Weekly digest".into(),
            }],
        }
    }

    fn fresh(purpose: &str, run: &str) -> AgentDesign {
        match start(
            None,
            purpose,
            1_000,
            ("d-1".into(), run.into()),
            RunSeen::Live,
        ) {
            Start::Fresh(d) => d,
            Start::Existing(_) => unreachable!(),
        }
    }

    #[test]
    fn a_describe_starts_generating_with_its_purpose_and_clock() {
        let d = fresh("a release manager", "r-1");
        assert_eq!(d.status, DesignStatus::Generating);
        assert_eq!(d.purpose, "a release manager");
        assert_eq!(d.instruction, "a release manager");
        assert_eq!(d.run_id.as_deref(), Some("r-1"));
        assert_eq!(d.started_at, Some(1_000));
        assert!(d.draft.is_none() && d.chat.is_empty() && !d.refining);
    }

    #[test]
    fn a_second_start_while_generating_follows_the_first() {
        let d = fresh("first", "r-1");
        let again = start(
            Some(&d),
            "second",
            2_000,
            ("d-2".into(), "r-2".into()),
            RunSeen::Live,
        );
        assert_eq!(again, Start::Existing(d));
    }

    #[test]
    fn a_describe_replaces_an_idle_design() {
        let ready = land(&fresh("first", "r-1"), "r-1", draft("Scout"), 2_000).unwrap();
        let Start::Fresh(next) = start(
            Some(&ready),
            "second",
            3_000,
            ("d-2".into(), "r-2".into()),
            RunSeen::Live,
        ) else {
            panic!("an idle design is replaced");
        };
        assert_eq!(next.id, "d-2");
        assert_eq!(next.purpose, "second");
        assert!(next.draft.is_none() && next.chat.is_empty());
        let failed = fail(&fresh("x", "r-1"), "r-1", "no", 2_000).unwrap();
        assert!(matches!(
            start(
                Some(&failed),
                "y",
                3_000,
                ("d-3".into(), "r-3".into()),
                RunSeen::Live
            ),
            Start::Fresh(_)
        ));
    }

    #[test]
    fn landing_fills_the_draft_and_the_chat_the_next_refine_reads() {
        let d = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        assert_eq!(d.status, DesignStatus::Ready);
        assert_eq!(d.finished_at, Some(5_000));
        assert_eq!(d.draft, Some(draft("Scout")));
        assert_eq!(d.chat.len(), 2);
        assert_eq!(d.chat[0].content, "a scout");
        assert_eq!(
            serde_json::from_str::<DraftFields>(&d.chat[1].content).unwrap(),
            draft("Scout")
        );
    }

    #[test]
    fn an_answer_for_another_run_changes_nothing() {
        let d = fresh("a scout", "r-2");
        assert_eq!(land(&d, "r-1", draft("Scout"), 5_000), None);
        assert_eq!(fail(&d, "r-1", "no", 5_000), None);
        let ready = land(&d, "r-2", draft("Scout"), 5_000).unwrap();
        // Answered once: a reclaimed driver's second answer is dropped too.
        assert_eq!(land(&ready, "r-2", draft("Other"), 6_000), None);
    }

    #[test]
    fn a_refine_revises_the_form_as_the_person_sees_it() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        let mut edited = draft("Scout");
        edited.role = "Lead Analyst".into();
        let r = refine(
            Some(&ready),
            Some("d-1"),
            "make it terse",
            Some(edited.clone()),
            6_000,
            "r-2".into(),
        )
        .unwrap();
        assert_eq!(r.status, DesignStatus::Generating);
        assert!(r.refining);
        assert_eq!(r.instruction, "make it terse");
        assert_eq!(r.purpose, "a scout");
        assert_eq!(r.previous, Some(edited.clone()));
        assert_eq!(r.started_at, Some(6_000));
        let landed = land(&r, "r-2", draft("Terse"), 7_000).unwrap();
        assert_eq!(landed.chat.len(), 4);
        assert_eq!(landed.chat[2].content, "make it terse");
        assert_eq!(landed.previous, Some(edited));
        assert!(!landed.refining);
    }

    #[test]
    fn a_refine_is_refused_while_generating_or_with_no_draft() {
        let gen_ = fresh("a scout", "r-1");
        assert_eq!(
            refine(Some(&gen_), Some("d-1"), "x", None, 2_000, "r-2".into()),
            Err(Refusal::Generating)
        );
        assert_eq!(
            refine(None, None, "x", None, 2_000, "r-2".into()),
            Err(Refusal::NothingToRefine)
        );
        let failed = fail(&gen_, "r-1", "no", 2_000).unwrap();
        assert_eq!(
            refine(Some(&failed), Some("d-1"), "x", None, 3_000, "r-2".into()),
            Err(Refusal::NothingToRefine)
        );
    }

    #[test]
    fn a_failed_refine_keeps_the_draft_it_started_from() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        let r = refine(Some(&ready), Some("d-1"), "x", None, 6_000, "r-2".into()).unwrap();
        let failed = fail(&r, "r-2", "Muse could not design an agent", 7_000).unwrap();
        assert_eq!(failed.status, DesignStatus::Error);
        assert_eq!(failed.draft, Some(draft("Scout")));
        assert_eq!(
            failed.error.as_deref(),
            Some("Muse could not design an agent")
        );
        assert_eq!(
            failed.chat.len(),
            2,
            "a failed turn adds nothing to the chat"
        );
    }

    #[test]
    fn the_chat_window_keeps_eight_turns_then_the_new_pair() {
        let mut d = land(&fresh("t0", "r-0"), "r-0", draft("A"), 1).unwrap();
        for i in 1..8 {
            let r = refine(
                Some(&d),
                Some("d-1"),
                &format!("t{i}"),
                None,
                i,
                format!("r-{i}"),
            )
            .unwrap();
            d = land(&r, &format!("r-{i}"), draft("A"), i).unwrap();
        }
        assert_eq!(d.chat.len(), 10);
        assert_eq!(d.chat[0].content, "t3", "the oldest turns fall out first");
        assert_eq!(d.chat[8].content, "t7");
    }

    #[test]
    fn an_edit_is_refused_while_generating_and_makes_a_manual_start_otherwise() {
        let gen_ = fresh("a scout", "r-1");
        assert_eq!(
            edit(Some(&gen_), Some("d-1"), draft("X"), 2_000, "d-2".into()),
            Err(Refusal::Generating)
        );
        let manual = edit(None, None, draft("Hand"), 2_000, "d-2".into()).unwrap();
        assert_eq!(manual.status, DesignStatus::Ready);
        assert_eq!(manual.id, "d-2");
        assert!(manual.run_id.is_none() && manual.chat.is_empty());
        let failed = fail(&gen_, "r-1", "no", 2_000).unwrap();
        let fixed = edit(
            Some(&failed),
            Some("d-1"),
            draft("Hand"),
            3_000,
            "d-3".into(),
        )
        .unwrap();
        assert_eq!(fixed.status, DesignStatus::Ready);
        assert_eq!(fixed.error, None);
        assert_eq!(fixed.id, "d-1", "an edit keeps the record it edits");
    }

    #[test]
    fn a_run_that_ended_without_answering_reads_as_stalled() {
        let d = fresh("a scout", "r-1");
        assert_eq!(stalled(&d, RunSeen::Live, 1_000_000), None);
        let s = stalled(&d, RunSeen::Ended, 2_000).unwrap();
        assert_eq!(s.status, DesignStatus::Error);
        assert_eq!(s.error.as_deref(), Some(STALLED));
        // No row yet: the enqueue may still be on its way.
        assert_eq!(
            stalled(&d, RunSeen::Missing, 1_000 + MISSING_RUN_GRACE_MS),
            None
        );
        assert!(stalled(&d, RunSeen::Missing, 2_000 + MISSING_RUN_GRACE_MS).is_some());
        let ready = land(&d, "r-1", draft("Scout"), 2_000).unwrap();
        assert_eq!(stalled(&ready, RunSeen::Ended, 3_000), None);
    }

    #[test]
    fn the_wire_shape_is_camel_case_with_lowercase_status() {
        let v = serde_json::to_value(fresh("a scout", "r-1")).unwrap();
        assert_eq!(v["status"], "generating");
        assert_eq!(v["runId"], "r-1");
        assert_eq!(v["startedAt"], 1_000);
        assert!(v.get("finishedAt").is_some());
    }

    #[test]
    fn a_describe_over_a_design_whose_run_died_starts_fresh() {
        let dead = fresh("an orphan", "r-1");
        let later = 1_000 + MISSING_RUN_GRACE_MS + 1;
        assert!(matches!(
            start(
                Some(&dead),
                "again",
                later,
                ("d-2".into(), "r-2".into()),
                RunSeen::Ended
            ),
            Start::Fresh(_)
        ));
        assert!(matches!(
            start(
                Some(&dead),
                "again",
                2_000,
                ("d-2".into(), "r-2".into()),
                RunSeen::Live
            ),
            Start::Existing(_)
        ));
    }

    #[test]
    fn an_edit_or_refine_must_name_the_design_it_edits() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        assert_eq!(
            edit(
                Some(&ready),
                Some("d-other"),
                draft("X"),
                6_000,
                "d-9".into()
            ),
            Err(Refusal::Stale)
        );
        assert_eq!(
            edit(Some(&ready), None, draft("X"), 6_000, "d-9".into()),
            Err(Refusal::Stale),
            "a nameless save only starts a design where none exists"
        );
        assert_eq!(
            edit(None, Some("d-1"), draft("X"), 6_000, "d-9".into()),
            Err(Refusal::Stale),
            "a save for a design discarded elsewhere does not recreate it"
        );
        assert_eq!(
            refine(
                Some(&ready),
                Some("d-other"),
                "x",
                None,
                6_000,
                "r-2".into()
            ),
            Err(Refusal::Stale)
        );
        assert_eq!(
            refine(None, Some("d-1"), "x", None, 6_000, "r-2".into()),
            Err(Refusal::Stale)
        );
    }

    #[test]
    fn a_hand_edit_ends_the_refine_receipt() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        let r = refine(Some(&ready), Some("d-1"), "x", None, 6_000, "r-2".into()).unwrap();
        let landed = land(&r, "r-2", draft("Terse"), 7_000).unwrap();
        assert!(landed.previous.is_some());
        let edited = edit(
            Some(&landed),
            Some("d-1"),
            draft("Hand"),
            8_000,
            "d-9".into(),
        )
        .unwrap();
        assert_eq!(edited.previous, None);
    }

    #[test]
    fn a_hire_moves_the_design_out_of_the_slot_and_settles_it() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        let held = claim(Some(&ready), "d-1", "h-1", 6_000).unwrap();
        assert_eq!(held.hire_run_id, "h-1");
        assert_eq!(held.design.draft, Some(draft("Scout")));
        assert_eq!(
            claim(Some(&ready), "d-9", "h-2", 6_000),
            Err(Refusal::Stale)
        );
        assert_eq!(claim(None, "d-1", "h-2", 6_000), Err(Refusal::Stale));
        assert_eq!(
            settle_held(&held, &HireSeen::Live, 7_000),
            HireOutcome::Unchanged
        );
        assert_eq!(
            settle_held(&held, &HireSeen::Done, 7_000),
            HireOutcome::Consumed
        );
        // The agent exists: whatever failed after (a pull, a boot), the
        // design is spent; Retry and Start finish it.
        assert_eq!(
            settle_held(
                &held,
                &HireSeen::Failed {
                    created: true,
                    why: Some("the pull failed".into())
                },
                7_000
            ),
            HireOutcome::Consumed
        );
        // Failed before the agent existed: it comes back, saying why.
        let HireOutcome::Returned(why) = settle_held(
            &held,
            &HireSeen::Failed {
                created: false,
                why: Some("the department research is taken".into()),
            },
            7_000,
        ) else {
            panic!("a pre-create failure hands the design back");
        };
        assert!(why.contains("the department research is taken"));
        let back = hand_back(
            &Held {
                returned: Some(why),
                ..held.clone()
            },
            8_000,
        );
        assert_eq!(back.status, DesignStatus::Ready);
        assert_eq!(back.draft, Some(draft("Scout")));
        assert_eq!(back.released_from.as_deref(), Some("h-1"));
        // A hire row that never appeared hands it back after the grace.
        assert_eq!(
            settle_held(&held, &HireSeen::Missing, 7_000),
            HireOutcome::Unchanged
        );
        assert!(matches!(
            settle_held(&held, &HireSeen::Missing, 6_000 + MISSING_RUN_GRACE_MS + 1),
            HireOutcome::Returned(_)
        ));
    }

    #[test]
    fn a_retried_hire_takes_the_design_with_it() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        let held = claim(Some(&ready), "d-1", "h-1", 6_000).unwrap();
        let moved = follow_held(
            &Held {
                returned: Some("x".into()),
                ..held.clone()
            },
            "h-1",
            "h-1r",
            7_000,
        )
        .unwrap();
        assert_eq!(moved.hire_run_id, "h-1r");
        assert_eq!(moved.returned, None);
        assert_eq!(follow_held(&held, "h-9", "h-9r", 7_000), None);
        // Already handed back into the slot: spent on the retry again.
        let back = hand_back(
            &Held {
                returned: Some("x".into()),
                ..held.clone()
            },
            8_000,
        );
        let again = follow_back(&back, "h-1", "h-1r", 9_000).unwrap();
        assert_eq!(again.hire_run_id, "h-1r");
        assert_eq!(again.design.error, None);
        assert_eq!(follow_back(&ready, "h-1", "h-1r", 9_000), None);
    }

    #[test]
    fn an_edit_after_a_hand_back_is_not_spent_on_the_old_hires_retry() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        let held = claim(Some(&ready), "d-1", "h-1", 6_000).unwrap();
        let back = hand_back(
            &Held {
                returned: Some("x".into()),
                ..held
            },
            7_000,
        );
        assert!(follow_back(&back, "h-1", "h-1r", 8_000).is_some());
        let edited = edit(
            Some(&back),
            Some("d-1"),
            draft("Renamed"),
            8_000,
            "d-9".into(),
        )
        .unwrap();
        assert_eq!(edited.released_from, None);
        assert!(follow_back(&edited, "h-1", "h-1r", 9_000).is_none());
    }
}
