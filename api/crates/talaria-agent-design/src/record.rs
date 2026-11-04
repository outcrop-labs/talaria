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
}

/// A transition the record refuses — each is a 409 the person can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A design is still generating; nothing else may write it but its run.
    Generating,
    /// There is no draft to refine.
    NothingToRefine,
}

impl Refusal {
    pub fn sentence(self) -> &'static str {
        match self {
            Refusal::Generating => {
                "an agent design is still generating — wait for it, or discard it"
            }
            Refusal::NothingToRefine => "there is no agent design to refine — describe one first",
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

/// A describe. A record that is generating is returned as it is; anything
/// idle (ready, error, a manual start) is replaced, because a fresh describe
/// is a fresh intent.
pub fn start(
    existing: Option<&AgentDesign>,
    purpose: &str,
    now: i64,
    design_id: String,
    run_id: String,
) -> Start {
    if let Some(d) = existing
        && d.status == DesignStatus::Generating
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
    })
}

/// A refine of the current draft. `current` is the form as the person sees
/// it — hand edits since the last save are part of what the model revises —
/// and replaces the stored draft when given.
pub fn refine(
    existing: Option<&AgentDesign>,
    instruction: &str,
    current: Option<DraftFields>,
    now: i64,
    run_id: String,
) -> Result<AgentDesign, Refusal> {
    let Some(d) = existing else {
        return Err(Refusal::NothingToRefine);
    };
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
    fields: DraftFields,
    now: i64,
    design_id: String,
) -> Result<AgentDesign, Refusal> {
    match existing {
        Some(d) if d.status == DesignStatus::Generating => Err(Refusal::Generating),
        Some(d) => Ok(AgentDesign {
            status: DesignStatus::Ready,
            error: None,
            draft: Some(fields),
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
        }),
    }
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
        match start(None, purpose, 1_000, "d-1".into(), run.into()) {
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
        let again = start(Some(&d), "second", 2_000, "d-2".into(), "r-2".into());
        assert_eq!(again, Start::Existing(d));
    }

    #[test]
    fn a_describe_replaces_an_idle_design() {
        let ready = land(&fresh("first", "r-1"), "r-1", draft("Scout"), 2_000).unwrap();
        let Start::Fresh(next) = start(Some(&ready), "second", 3_000, "d-2".into(), "r-2".into())
        else {
            panic!("an idle design is replaced");
        };
        assert_eq!(next.id, "d-2");
        assert_eq!(next.purpose, "second");
        assert!(next.draft.is_none() && next.chat.is_empty());
        let failed = fail(&fresh("x", "r-1"), "r-1", "no", 2_000).unwrap();
        assert!(matches!(
            start(Some(&failed), "y", 3_000, "d-3".into(), "r-3".into()),
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
            refine(Some(&gen_), "x", None, 2_000, "r-2".into()),
            Err(Refusal::Generating)
        );
        assert_eq!(
            refine(None, "x", None, 2_000, "r-2".into()),
            Err(Refusal::NothingToRefine)
        );
        let failed = fail(&gen_, "r-1", "no", 2_000).unwrap();
        assert_eq!(
            refine(Some(&failed), "x", None, 3_000, "r-2".into()),
            Err(Refusal::NothingToRefine)
        );
    }

    #[test]
    fn a_failed_refine_keeps_the_draft_it_started_from() {
        let ready = land(&fresh("a scout", "r-1"), "r-1", draft("Scout"), 5_000).unwrap();
        let r = refine(Some(&ready), "x", None, 6_000, "r-2".into()).unwrap();
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
            let r = refine(Some(&d), &format!("t{i}"), None, i, format!("r-{i}")).unwrap();
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
            edit(Some(&gen_), draft("X"), 2_000, "d-2".into()),
            Err(Refusal::Generating)
        );
        let manual = edit(None, draft("Hand"), 2_000, "d-2".into()).unwrap();
        assert_eq!(manual.status, DesignStatus::Ready);
        assert_eq!(manual.id, "d-2");
        assert!(manual.run_id.is_none() && manual.chat.is_empty());
        let failed = fail(&gen_, "r-1", "no", 2_000).unwrap();
        let fixed = edit(Some(&failed), draft("Hand"), 3_000, "d-3".into()).unwrap();
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
}
