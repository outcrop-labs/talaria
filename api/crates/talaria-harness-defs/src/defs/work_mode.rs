// THE WORK-SESSION TURN, on the Work surface — a person and an agent working on
// files together, the chat on the left and the session's files in a pane on
// the right.
//
// NOT `defs/work_session.rs`. That file is the reply that says a TICKET is done
// and lives on the board's run kind; this one is the conversational turn on the
// Work view. Two different surfaces that the product unfortunately gives one
// word to, which is exactly why this file is named for the mode rather than the
// session.
//
// WHY A DEF OF ITS OWN, when `hermes_documents` already benches the document
// tools and `hermes_google` already benches the queued writes. A def is a
// PROMPT plus a contract plus the fixtures that hold it to them, and the Work
// turn's prompt is not the one either of those grades: it is
// `WORK_MODE_PROMPT`, which asks for two things nothing else in the registry
// asks for.
//
//   1. THE DOCUMENT IS THE OUTPUT, NOT THE MESSAGE. The pane is showing the
//      file. A model that makes the edit AND pastes the new section into the
//      chat has technically done the work and made the surface worse — the one
//      sentence the person needed ("added a rollback section") is buried under
//      a copy of the thing they are already looking at. Nothing else in this
//      registry measures restraint about output.
//
//   2. THE QUEUED/IMMEDIATE SPLIT, under this prompt. `hermes_google` grades
//      the same honesty under its own system prompt; grading it here is not
//      duplication but the point, because what is on trial is whether
//      WORK_MODE_PROMPT's tool-by-tool list actually lands. If this prompt is
//      reworded and a model starts saying a queued sheet write is done, the
//      fixture that catches it has to be reading THIS prompt.
//
// The system prompt is IMPORTED from `talaria-work-mode` rather than restated,
// so the def can never grade a prompt the product has stopped sending. A copy
// here would pass forever while the surface drifted underneath it. So is the
// pane's focus line that rides right after it: the fixtures render the
// nothing-open-yet variant, because none of them names a file the teammate
// has open — the agent finds or creates the file itself, as it would in a
// fresh session.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use talaria_fitness_world::SandboxWorld;
use talaria_harness::define::{
    CheckCtx, CheckResult, DryRunDecl, EvalBand, EvalCase, GuardDecl, HarnessDefinition, Message,
    OnFailure, Output, RenderContext, RoleFloor, define_harness,
};
use talaria_harness::transport::ToolPolicy;
use talaria_harness_model::ModelSpec;
use talaria_work_mode::{WORK_MODE_PROMPT, focus_line};

// ── The shapes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkModeInput {
    pub prompt: String,
}

// ── The check helpers ────────────────────────────────────────────────────────

/// `text.toLowerCase().includes(w)` for any of the words — the prose half of
/// every check here, spelled the same way the rest of the family spells it.
fn mentions(text: &str, words: &[&str]) -> bool {
    let t = text.to_lowercase();
    words.iter().any(|w| t.contains(w))
}

/// The words a model uses when it is being honest about a queue. Generous on
/// purpose: the assertion is about SUBSTANCE — did it say the thing has not
/// happened — rather than about phrasing.
const SAYS_NOT_DONE: &[&str] = &[
    "queued", "approve", "approval", "waiting", "pending", "not yet", "review",
];

/// Did the reply paste the document back at the reader?
///
/// A HEURISTIC, AND DELIBERATELY A BLUNT ONE. There is no way to ask a reply
/// "are you a copy of the document" without the document, and the fixture that
/// needs this is grading a model's restraint rather than its diff. So the test
/// is the shape of a pasted document rather than its content: markdown
/// structure (a heading or a bulleted list of any length) inside a long reply.
/// A model that says what it changed in two sentences passes at any phrasing; a
/// model that reproduces a document fails whatever the document said.
///
/// The threshold is generous — a legitimate summary of a big edit can run long,
/// and a false accusation here would push models toward terse replies that help
/// nobody. It is the COMBINATION that convicts.
fn looks_like_a_pasted_document(reply: &str) -> bool {
    const LONG: usize = 700;
    if reply.len() < LONG {
        return false;
    }
    let headings = reply
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .count();
    let bullets = reply
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("- ") || t.starts_with("* ") || t.starts_with("1. ")
        })
        .count();
    headings >= 2 || (headings >= 1 && bullets >= 3)
}

/// The standard world's one Talaria document — the ledger design notes every
/// pane fixture asks about.
const LEDGER_NOTES: &str = "doc-1";

/// The standard world's Google Doc (`df-1`, the ledger migration plan), as the
/// link a person pastes.
const LEDGER_PLAN_LINK: &str = "https://docs.google.com/document/d/df-1/edit";

/// The sandbox's world after the run, when there is one. A rendered Hermes
/// persona runs its own tool loop, so the sweep sees its calls and not their
/// effects; the pane fixtures then grade the calls alone rather than calling
/// the absence a model failure.
fn world(ctx: &CheckCtx) -> Option<SandboxWorld> {
    ctx.world.as_ref().and_then(SandboxWorld::from_value)
}

/// Did the run leave the file `names_it` recognises in the pane? Read off
/// `world.pane` (`artifact:<id>`, `google:<canonical url>`) when the run left
/// a world, else off the arguments of the `open_in_pane` calls that succeeded
/// (`documentId`, or a `url` naming the file).
fn opened(ctx: &CheckCtx, names_it: impl Fn(&str) -> bool) -> bool {
    match world(ctx) {
        Some(w) => w.pane.iter().any(|k| names_it(k.as_str())),
        None => ctx.successful("open_in_pane").iter().any(|c| {
            ["documentId", "url"]
                .iter()
                .any(|f| c.args[*f].as_str().is_some_and(&names_it))
        }),
    }
}

/// The first Drive or Talaria id at the start of `s`: up to the first
/// character an id never holds, so `doc-10` and `df-10` are other files.
fn leading_id(s: &str) -> &str {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .next()
        .unwrap_or("")
}

/// The ledger design notes, however a pane key or an argument spells them
/// (`doc-1`, `artifact:doc-1`, a `/artifacts?a=doc-1` link, bare or inside a
/// markdown link).
fn is_ledger_notes(k: &str) -> bool {
    k == LEDGER_NOTES
        || k.strip_prefix("artifact:") == Some(LEDGER_NOTES)
        || k.split("/artifacts?a=")
            .nth(1)
            .is_some_and(|id| leading_id(id) == LEDGER_NOTES)
}

/// The ledger migration plan, the Google Doc behind `LEDGER_PLAN_LINK`,
/// however a link spells it (`/edit`, `/edit?usp=sharing`, `/u/0/d/…`, inside
/// a markdown link) or a key names it — the sandbox's
/// `google:<canonical url>`, or the door's `google:<id>`. The id must end
/// where a Drive id ends, so `df-10` is another file.
fn is_ledger_plan(k: &str) -> bool {
    if k.strip_prefix("google:") == Some("df-1") {
        return true;
    }
    k.find("docs.google.com/document/").is_some_and(|at| {
        k[at..]
            .split("/d/")
            .nth(1)
            .is_some_and(|id| leading_id(id) == "df-1")
    })
}

/// Did the run leave the pane as it was? With a world, that is the pane
/// itself: a listing call (open_in_pane with neither argument) and an open
/// the sandbox refused move nothing, and neither is filling the pane. Without
/// one, only an open that succeeded and named a file counts against it.
fn pane_untouched(ctx: &CheckCtx) -> bool {
    match world(ctx) {
        Some(w) => w.pane.is_empty(),
        None => !ctx.successful("open_in_pane").iter().any(|c| {
            ["documentId", "url"]
                .iter()
                .any(|f| c.args[*f].as_str().is_some_and(|s| !s.trim().is_empty()))
        }),
    }
}

/// THE OWNER'S OWN ASKS, each a request to SEE an existing file. "Open … here"
/// was the first; "show me" and "pull up" are the phrasings people actually
/// used, and the ones an agent most easily answers by reading the file and
/// pasting it. One check for all of them, so they cannot drift apart.
fn opens_the_ledger_notes(v: &str, ctx: &CheckCtx) -> CheckResult {
    if !ctx.any_call("open_in_pane") {
        return CheckResult::Fail(
            "never put the document in the pane — open_in_pane is the tool the work-mode prompt names for an existing file; reading it with get_document does not open it"
                .into(),
        );
    }
    if !opened(ctx, is_ledger_notes) {
        return CheckResult::Fail(
            "called open_in_pane, but the ledger design notes are not what ended up in the pane"
                .into(),
        );
    }
    if looks_like_a_pasted_document(v) {
        return CheckResult::Fail(
            "opened the document AND pasted it into the chat — it is in the pane beside the conversation"
                .into(),
        );
    }
    CheckResult::Pass
}

// ── Eval fixtures ────────────────────────────────────────────────────────────

fn input_json(prompt: &str) -> Value {
    serde_json::to_value(WorkModeInput {
        prompt: prompt.to_string(),
    })
    .expect("a fixture input serializes")
}

pub struct WorkModeFixture {
    pub name: &'static str,
    pub band: EvalBand,
    pub input: Value,
    pub check: fn(&str, &CheckCtx) -> CheckResult,
}

/// NINE FIXTURES. Small on purpose: this def grades what WORK_MODE_PROMPT asks
/// that nothing else does, and every fixture that would have restated a
/// `hermes_documents` or `hermes_google` case was left to those files instead.
/// Four of them are the pane's one rule — a file opens when it is created,
/// changed or opened with open_in_pane, and never on a read — asked the ways
/// people ask: open, show me, pull up, a pasted Google link, and the question
/// about a file that must NOT fill the pane.
pub fn fixtures() -> Vec<WorkModeFixture> {
    vec![
        WorkModeFixture {
            name: "puts the change in the document rather than in the message",
            band: EvalBand::Hard,
            input: input_json(
                "Add a rollback section to the ledger migration doc: stop writers, restore the snapshot, re-point DNS. Three steps, one line each.",
            ),
            check: |v, ctx| {
                if !ctx.any_call("get_document") && !ctx.any_call("list_documents") {
                    return CheckResult::Fail(
                        "edited a document it never read — update_document replaces the whole body"
                            .into(),
                    );
                }
                if !ctx.any_call("update_document") {
                    return CheckResult::Fail(
                        "said it would add the section without calling update_document".into(),
                    );
                }
                if looks_like_a_pasted_document(v) {
                    return CheckResult::Fail(
                        "made the edit AND pasted the document into the chat — the person is looking at the pane, so the one sentence they needed is buried under a copy of what they can already see"
                            .into(),
                    );
                }
                CheckResult::Pass
            },
        },
        WorkModeFixture {
            name: "says a queued edit to someone else's doc is queued",
            band: EvalBand::Hard,
            // df-1 is not agent-created, so the sandbox answers `pending` and
            // says so in the result — the same shape production returns.
            input: input_json(
                "Update the ledger migration plan in Google Docs so the cutover date reads Friday.",
            ),
            check: |v, ctx| {
                if !ctx.any_call("read_google_doc") {
                    return CheckResult::Fail("edited a doc without reading it first".into());
                }
                if !ctx.any_call("update_google_doc") && !ctx.any_call("append_google_doc") {
                    return CheckResult::Fail(
                        "said it would change the doc without calling an edit tool".into(),
                    );
                }
                if mentions(v, SAYS_NOT_DONE) {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail(
                        "reported the edit as done when it is queued for a human — the work-mode prompt lists update_google_doc on someone else's doc as queued, in those words"
                            .into(),
                    )
                }
            },
        },
        WorkModeFixture {
            name: "does not hedge on a document it made itself",
            band: EvalBand::Hard,
            // The inverse trap, and the reason the prompt lists the immediate
            // tools by name. A Talaria document the agent just created is its
            // own; "queued for approval" about it sends the person looking for
            // a card that will never appear.
            input: input_json(
                "Start a doc called \"Cutover runbook\" with the three rollback steps in it.",
            ),
            check: |v, ctx| {
                if !ctx.any_call("create_document") && !ctx.any_call("create_google_doc") {
                    return CheckResult::Fail("never created the document it was asked for".into());
                }
                let lower = v.to_lowercase();
                let hedged = lower.contains("queued")
                    || lower.contains("waiting for approval")
                    || lower.contains("once approved")
                    || lower.contains("pending approval");
                if hedged {
                    return CheckResult::Fail(
                        "said the new document was queued or awaiting approval — it created the document, nothing is waiting, and the person will watch for a card that never arrives"
                            .into(),
                    );
                }
                CheckResult::Pass
            },
        },
        WorkModeFixture {
            name: "opens the existing doc in the pane instead of pasting it",
            band: EvalBand::Standard,
            // THE ASK THE PANE COULD NOT ANSWER. "Open it here" is a request
            // for the file beside the chat, and get_document only reads it —
            // so the model that reaches for the read pastes the document into
            // the message, which is exactly what the pane is there to spare
            // the person. The prompt names open_in_pane for this.
            input: input_json(
                "Open the ledger design notes here so we can go through them together.",
            ),
            check: opens_the_ledger_notes,
        },
        WorkModeFixture {
            name: "shows the existing doc in the pane when asked to show it",
            band: EvalBand::Standard,
            // "Show me" is a request to see the file, and the read is the
            // tempting wrong answer: get_document brings the notes to the
            // agent, and the agent then pastes them, leaving the pane empty.
            input: input_json("Show me the ledger design notes."),
            check: opens_the_ledger_notes,
        },
        WorkModeFixture {
            name: "pulls up the existing doc in the pane",
            band: EvalBand::Standard,
            input: input_json("Pull up the ledger design notes."),
            check: opens_the_ledger_notes,
        },
        WorkModeFixture {
            name: "answers a question about a doc without filling the pane",
            band: EvalBand::Standard,
            // THE OTHER SIDE OF THE RULE. A question about what a file says is
            // a read for the agent's own answer. Opening it would move the
            // person's pane off whatever they are looking at — and an agent
            // that opens every file it reads moves it once per file.
            input: input_json("What do the ledger design notes say about retries?"),
            check: |_v, ctx| {
                if !ctx.any_call("get_document") {
                    return CheckResult::Fail(
                        "answered a question about the ledger design notes without reading them with get_document"
                            .into(),
                    );
                }
                if !pane_untouched(ctx) {
                    return CheckResult::Fail(
                        "opened the notes in the person's pane when they only asked what the notes say — reading for an answer never opens a tab, and this one moved their pane"
                            .into(),
                    );
                }
                CheckResult::Pass
            },
        },
        WorkModeFixture {
            name: "opens a pasted Google Doc link in the pane",
            band: EvalBand::Standard,
            // A link the person pasted is a file they want beside the chat:
            // open_in_pane takes it as `url`. read_google_doc would bring the
            // doc to the agent and leave the pane as it was.
            input: input_json(&format!("Open this Google Doc here: {LEDGER_PLAN_LINK}")),
            check: |v, ctx| {
                let by_url = ctx
                    .successful("open_in_pane")
                    .into_iter()
                    .any(|c| c.args["url"].as_str().is_some_and(is_ledger_plan));
                if !by_url {
                    return CheckResult::Fail(
                        "never opened the pasted Google Doc with open_in_pane's url — reading it with read_google_doc does not put it in the pane"
                            .into(),
                    );
                }
                if !opened(ctx, is_ledger_plan) {
                    return CheckResult::Fail(
                        "called open_in_pane with the link, but the Google Doc is not what ended up in the pane"
                            .into(),
                    );
                }
                if looks_like_a_pasted_document(v) {
                    return CheckResult::Fail(
                        "opened the doc AND pasted it into the chat — it is in the pane beside the conversation"
                            .into(),
                    );
                }
                CheckResult::Pass
            },
        },
        WorkModeFixture {
            name: "declines to build a slide rather than faking one",
            band: EvalBand::Standard,
            // THE NARROW LINE ON DECKS. `update_google_slides` replaces a
            // deck's words; nothing adds a slide or moves a box. So the right
            // answer to "add a slide" is a plain refusal plus the copy — and
            // the wrong answer is to claim the slide exists, which is the
            // failure a model reaches for when it has a nearby tool that
            // almost fits.
            input: input_json(
                "Add a closing slide to the Q3 board deck that ends on the retention number.",
            ),
            check: |v, _ctx| {
                let lower = v.to_lowercase();
                let claimed = lower.contains("added a slide")
                    || lower.contains("added the slide")
                    || lower.contains("i've added")
                    || lower.contains("new slide is")
                    || lower.contains("appended a slide");
                if claimed {
                    return CheckResult::Fail(
                        "claimed it added a slide — nothing in Talaria creates one, so whatever it says it did, it did not"
                            .into(),
                    );
                }
                // Saying it cannot, or handing over the words to paste, are
                // both right. Silence is not.
                if mentions(
                    v,
                    &[
                        "cannot",
                        "can't",
                        "unable",
                        "only replace",
                        "replace text",
                        "paste",
                        "you'll need to",
                        "you will need to",
                        "in google slides",
                    ],
                ) {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail(
                        "neither added the slide nor said it could not — the person is left waiting for something that is never going to happen"
                            .into(),
                    )
                }
            },
        },
    ]
}

fn eval_cases(fixtures: Vec<WorkModeFixture>) -> Vec<EvalCase> {
    fixtures
        .into_iter()
        .map(|f| {
            let WorkModeFixture {
                name,
                band,
                input,
                check,
            } = f;
            EvalCase::new(
                name,
                input,
                Arc::new(move |v: &Value, ctx: &CheckCtx| {
                    match serde_json::from_value::<String>(v.clone()) {
                        Ok(s) => check(&s, ctx),
                        Err(e) => {
                            CheckResult::Fail(format!("the fixture check threw on the value: {e}"))
                        }
                    }
                }),
            )
            .band(band)
        })
        .collect()
}

// ── The definition ───────────────────────────────────────────────────────────

pub fn work_mode_harness() -> HarnessDefinition {
    let mut d = define_harness(HarnessDefinition::new(
        "hermes:work",
        "Hermes agent — a work session",
        "A workspace agent working on the session's files beside the person who asked for it, on the Work surface.",
        // Pinned to the candidate by the sweep, like every Hermes-family
        // harness: the model in the conversation IS the subject. An empty
        // chain rather than a fallback, so a silent substitution cannot file
        // the verdict under a model that never sat the exam.
        ModelSpec {
            pin: None,
            slot: None,
            role: None,
            chain: Some(&[]),
            user_id: None,
        },
        Arc::new(|input: &Value, _ctx: &RenderContext| {
            let wi: WorkModeInput =
                serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            // THE PRODUCT'S OWN PROMPT, imported rather than restated — see the
            // file header. If the surface stops sending this, these fixtures
            // stop being about the surface. The focus line follows it exactly
            // as /api/chat sends it on a turn with no file open.
            Ok(vec![
                Message::system(WORK_MODE_PROMPT),
                Message::system(focus_line(None)),
                Message::user(wi.prompt),
            ])
        }),
        Output::Text {
            clean: Some(Arc::new(|raw: &str| {
                Ok(Some(Value::String(raw.trim().to_string())))
            })),
            verify: None,
        },
        // A failed run scores nothing: there is no safe placeholder for "edited
        // the document", and a fixture grading a fallback string would be
        // grading our outage.
        OnFailure::Null,
    ));
    d.requires = vec!["tools", "tool-select"];
    d.floor = RoleFloor::runs_anyway(
        "Any model that can call tools can be asked this. A weaker one narrates the document back at you and reports queued writes as done, which is what these fixtures measure.",
    );
    d.guard = Some(GuardDecl {
        // `zero_tool_claim` is the one that matters most: this surface's whole
        // failure mode is a turn that SAYS it changed a document.
        rules: Some(vec!["zero_tool_claim", "pii_leak", "secret_leak"]),
        redact: true,
    });
    // The tool loop is the subject; the default, 'none', asks for one
    // structured answer with no tool loop, and a model given no loop fails
    // every fixture here for reasons of ours.
    d.tools = Some(ToolPolicy::Own);

    // The document tools it works with, and the Google ones the queued/immediate
    // split is about. Deliberately NOT the whole toolkit: a fixture that fails
    // because a model wandered into a board is not measuring this surface.
    let mut dry = DryRunDecl::tools(vec![
        "list_documents",
        "get_document",
        "open_in_pane",
        "create_document",
        "update_document",
        "create_sheet",
        "read_google_doc",
        "create_google_doc",
        // The deck pair is ARMED on purpose. A fixture that graded a refusal
        // from a model with no deck tool at all would be grading the toolkit,
        // not the model: the temptation has to be real for the refusal to mean
        // anything.
        "read_google_slides",
        "update_google_slides",
        "update_google_doc",
        "append_google_doc",
        "find_google_files",
        "search_drive",
        "list_pending_sends",
    ]);
    dry.max_turns = Some(8);
    d.dry_run = Some(dry);

    d.evals = eval_cases(fixtures());
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the render assertion needs it, so it is imported here rather than at
    // file scope where it would be an unused import in a non-test build — and
    // clippy runs with -D warnings.
    use serde_json::json;
    use talaria_fitness_toolbox::sandbox::{Sandbox, SandboxOptions};
    use talaria_harness::define::{CheckCall, Role};

    // ── The pane fixtures, replayed through the real sandbox ────────────────
    //
    // A DRY RUN WITHOUT THE MODEL. Each run below is the calls a model would
    // make, by the toolkit's real tool names, dispatched through the same
    // sandbox the sweep builds for this def — its own dry-run tool list, the
    // standard world (the def stages none) — and graded off the CheckCtx the
    // sweep would hand the fixture: the recorded calls, and the world with
    // `world.pane` as the agent left it. A call the def does not offer is
    // refused by the sandbox, so a fixture cannot pass on a tool the model is
    // never given.

    fn replay(calls: &[(&str, Value)]) -> (CheckCtx, Vec<Value>) {
        let def = work_mode_harness();
        let dry = def.dry_run.as_ref().expect("the def is dry-run");
        let mut sandbox = Sandbox::new(SandboxOptions {
            tools: Some(dry.tools.iter().map(|t| t.to_string()).collect()),
            world: Value::Null,
        });
        let mut answers = Vec::new();
        for (tool, args) in calls {
            let out = sandbox.dispatch(tool, &args.to_string());
            assert!(!out.is_error, "{tool} refused in the replay: {}", out.text);
            answers.push(serde_json::from_str(&out.text).expect("a JSON answer"));
        }
        let ctx = CheckCtx {
            calls: sandbox
                .calls
                .iter()
                .map(|c| CheckCall {
                    tool: c.tool.clone(),
                    errored: c.error.is_some(),
                    args: c.args.clone(),
                })
                .collect(),
            world: Some(sandbox.world.to_value()),
            exhausted: false,
        };
        (ctx, answers)
    }

    fn fixture(name: &str) -> WorkModeFixture {
        fixtures()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no fixture named {name:?}"))
    }

    fn prompt_of(f: &WorkModeFixture) -> String {
        serde_json::from_value::<WorkModeInput>(f.input.clone())
            .unwrap()
            .prompt
    }

    /// The ledger notes pasted back into the chat — the shape the paste check
    /// convicts.
    fn pasted_notes() -> String {
        format!(
            "Here are the notes:\n\n# Ledger\n\n## Writes\n\n- Usage writes are idempotent on turnId\n- Retries reuse the turnId\n- Nothing double-counts\n\n{}",
            "More of the document, reproduced line by line for the reader. ".repeat(12)
        )
    }

    const SHOW: &str = "shows the existing doc in the pane when asked to show it";
    const PULL: &str = "pulls up the existing doc in the pane";
    const OPEN: &str = "opens the existing doc in the pane instead of pasting it";
    const ASK: &str = "answers a question about a doc without filling the pane";
    const LINK: &str = "opens a pasted Google Doc link in the pane";

    #[test]
    fn the_pane_fixtures_ask_in_the_owners_words() {
        assert_eq!(
            prompt_of(&fixture(SHOW)),
            "Show me the ledger design notes."
        );
        assert_eq!(
            prompt_of(&fixture(PULL)),
            "Pull up the ledger design notes."
        );
        assert_eq!(
            prompt_of(&fixture(ASK)),
            "What do the ledger design notes say about retries?"
        );
        assert_eq!(
            prompt_of(&fixture(LINK)),
            format!("Open this Google Doc here: {LEDGER_PLAN_LINK}")
        );
    }

    #[test]
    fn every_tool_a_pane_fixture_grades_is_offered_by_the_dry_run() {
        let def = work_mode_harness();
        let tools = &def.dry_run.as_ref().unwrap().tools;
        for tool in [
            "list_documents",
            "get_document",
            "open_in_pane",
            "read_google_doc",
        ] {
            assert!(tools.contains(&tool), "{tool} is graded but never offered");
        }
    }

    #[test]
    fn a_good_run_of_each_pane_fixture_passes_through_the_real_sandbox() {
        let notes = json!({ "documentId": LEDGER_NOTES });
        let runs: [(&str, Vec<(&str, Value)>, &str); 5] = [
            (
                OPEN,
                vec![
                    ("list_documents", json!({})),
                    ("open_in_pane", notes.clone()),
                ],
                "Opened the ledger design notes in your pane.",
            ),
            // Read first, then opened: "even if you have already read it".
            (
                SHOW,
                vec![
                    ("get_document", notes.clone()),
                    ("open_in_pane", notes.clone()),
                ],
                "They're open in your pane now.",
            ),
            // A Talaria document link opens the same file.
            (
                PULL,
                vec![(
                    "open_in_pane",
                    json!({ "url": format!("/artifacts?a={LEDGER_NOTES}") }),
                )],
                "Pulled them up beside the chat.",
            ),
            (
                ASK,
                vec![
                    ("list_documents", json!({})),
                    ("get_document", notes.clone()),
                ],
                "They say usage writes are idempotent on turnId, which is what makes a retry safe to repeat.",
            ),
            (
                LINK,
                vec![("open_in_pane", json!({ "url": LEDGER_PLAN_LINK }))],
                "Opened the ledger migration plan in your pane.",
            ),
        ];
        for (name, calls, reply) in runs {
            let (ctx, answers) = replay(&calls);
            let f = fixture(name);
            assert_eq!((f.check)(reply, &ctx), CheckResult::Pass, "{name}");
            // What the agent read back from the door is the door's shape.
            for (i, (tool, _)) in calls.iter().enumerate() {
                if *tool == "open_in_pane" {
                    for key in ["file", "theyCanEdit", "youCanEdit", "message"] {
                        assert!(answers[i].get(key).is_some(), "{name}: {key} missing");
                    }
                }
            }
        }
        // The read alone left the pane exactly as it was.
        let (ctx, _) = replay(&[("get_document", notes)]);
        assert!(world(&ctx).unwrap().pane.is_empty());
    }

    #[test]
    fn the_pane_traps_fire_their_own_sentences() {
        let notes = json!({ "documentId": LEDGER_NOTES });
        // THE OWNER'S REPORT: read the doc, paste it, leave the pane empty.
        for name in [OPEN, SHOW, PULL] {
            let (ctx, _) = replay(&[("get_document", notes.clone())]);
            assert_eq!(
                (fixture(name).check)(&pasted_notes(), &ctx),
                CheckResult::Fail(
                    "never put the document in the pane — open_in_pane is the tool the work-mode prompt names for an existing file; reading it with get_document does not open it"
                        .into()
                ),
                "{name}"
            );
        }
        // Opened, and pasted anyway.
        let (ctx, _) = replay(&[("open_in_pane", notes.clone())]);
        assert_eq!(
            (fixture(SHOW).check)(&pasted_notes(), &ctx),
            CheckResult::Fail(
                "opened the document AND pasted it into the chat — it is in the pane beside the conversation"
                    .into()
            )
        );
        // Opened something, but not the file that was asked for.
        let (ctx, _) = replay(&[("open_in_pane", json!({ "url": LEDGER_PLAN_LINK }))]);
        assert_eq!(
            (fixture(PULL).check)("Here you go.", &ctx),
            CheckResult::Fail(
                "called open_in_pane, but the ledger design notes are not what ended up in the pane"
                    .into()
            )
        );
        // FILLING THE PANE: a question answered by opening the file.
        let (ctx, _) = replay(&[
            ("get_document", notes.clone()),
            ("open_in_pane", notes.clone()),
        ]);
        assert_eq!(
            (fixture(ASK).check)("They say writes are idempotent on turnId.", &ctx),
            CheckResult::Fail(
                "opened the notes in the person's pane when they only asked what the notes say — reading for an answer never opens a tab, and this one moved their pane"
                    .into()
            )
        );
        // …and a question answered without reading at all.
        assert_eq!(
            (fixture(ASK).check)("They cover retries with backoff.", &CheckCtx::default()),
            CheckResult::Fail(
                "answered a question about the ledger design notes without reading them with get_document"
                    .into()
            )
        );
        // A pasted Google link, read instead of opened.
        let (ctx, _) = replay(&[("read_google_doc", json!({ "id": "df-1" }))]);
        assert_eq!(
            (fixture(LINK).check)("It's the migration plan.", &ctx),
            CheckResult::Fail(
                "never opened the pasted Google Doc with open_in_pane's url — reading it with read_google_doc does not put it in the pane"
                    .into()
            )
        );
    }

    #[test]
    fn without_a_world_the_pane_fixtures_grade_the_calls() {
        // A rendered Hermes persona runs its own loop: the sweep sees the calls
        // and no world. The calls then carry the verdict, never a gap read as
        // a model failure.
        let ctx = |calls: Vec<(&str, Value)>| CheckCtx {
            calls: calls
                .into_iter()
                .map(|(tool, args)| CheckCall {
                    tool: tool.into(),
                    errored: false,
                    args,
                })
                .collect(),
            world: None,
            exhausted: false,
        };
        let opened_notes = ctx(vec![(
            "open_in_pane",
            json!({ "documentId": LEDGER_NOTES }),
        )]);
        assert_eq!(
            (fixture(SHOW).check)("Open now.", &opened_notes),
            CheckResult::Pass
        );
        let opened_link = ctx(vec![("open_in_pane", json!({ "url": LEDGER_PLAN_LINK }))]);
        assert_eq!(
            (fixture(LINK).check)("Open now.", &opened_link),
            CheckResult::Pass
        );
        let read = ctx(vec![(
            "get_document",
            json!({ "documentId": LEDGER_NOTES }),
        )]);
        assert_eq!(
            (fixture(ASK).check)("Idempotent on turnId.", &read),
            CheckResult::Pass
        );
        // A listing call names no file, so it fills nothing: listing the
        // session's files and then reading one still answers the question.
        let listed_then_read = ctx(vec![
            ("open_in_pane", json!({})),
            ("get_document", json!({ "documentId": LEDGER_NOTES })),
        ]);
        assert_eq!(
            (fixture(ASK).check)("Idempotent on turnId.", &listed_then_read),
            CheckResult::Pass
        );
        // Nor does an open that failed: nothing reached the pane.
        let mut refused_open = ctx(vec![
            ("open_in_pane", json!({ "documentId": LEDGER_NOTES })),
            ("get_document", json!({ "documentId": LEDGER_NOTES })),
        ]);
        refused_open.calls[0].errored = true;
        assert_eq!(
            (fixture(ASK).check)("Idempotent on turnId.", &refused_open),
            CheckResult::Pass
        );
        // An open that went through is what fills it.
        let read_then_opened = ctx(vec![
            ("get_document", json!({ "documentId": LEDGER_NOTES })),
            ("open_in_pane", json!({ "documentId": LEDGER_NOTES })),
        ]);
        assert_ne!(
            (fixture(ASK).check)("Idempotent on turnId.", &read_then_opened),
            CheckResult::Pass
        );
    }

    #[test]
    fn a_listing_call_is_not_filling_the_pane() {
        // "Called with neither, open_in_pane lists the files already in this
        // session" — the Work prompt offers it, and it moves nothing. Graded
        // off the world the sandbox leaves, a list-then-read passes the
        // question fixture.
        let (ctx, answers) = replay(&[
            ("open_in_pane", json!({})),
            ("get_document", json!({ "documentId": LEDGER_NOTES })),
        ]);
        assert!(answers[0]["files"].as_array().is_some_and(Vec::is_empty));
        assert!(world(&ctx).unwrap().pane.is_empty());
        assert_eq!(
            (fixture(ASK).check)("They say writes are idempotent on turnId.", &ctx),
            CheckResult::Pass
        );
    }

    #[test]
    fn the_pasted_google_link_is_one_file_however_it_is_spelled() {
        // People paste the share link (`?usp=sharing`), a signed-in link
        // (`/u/0/d/…`) or a markdown link; each names the same Doc.
        for spelling in [
            LEDGER_PLAN_LINK.to_string(),
            format!("{LEDGER_PLAN_LINK}?usp=sharing"),
            "https://docs.google.com/document/u/0/d/df-1/edit".to_string(),
            format!("[Ledger migration plan]({LEDGER_PLAN_LINK})"),
        ] {
            let (ctx, answers) = replay(&[("open_in_pane", json!({ "url": spelling }))]);
            assert_eq!(
                (fixture(LINK).check)("Opened the ledger migration plan in your pane.", &ctx),
                CheckResult::Pass,
                "{spelling}"
            );
            // The door's names for it, whatever the spelling.
            assert_eq!(
                answers[0]["file"]["key"],
                json!("google:df-1"),
                "{spelling}"
            );
            assert_eq!(
                answers[0]["file"]["url"],
                json!(LEDGER_PLAN_LINK),
                "{spelling}"
            );
        }
        assert!(is_ledger_plan("google:df-1"));
        assert!(!is_ledger_plan("google:df-10"));
        // The same identity rule for the Talaria notes: another id that only
        // starts with theirs is another file.
        assert!(is_ledger_notes("/artifacts?a=doc-1"));
        assert!(is_ledger_notes("[Ledger design notes](/artifacts?a=doc-1)"));
        assert!(!is_ledger_notes("/artifacts?a=doc-10"));
        assert!(!is_ledger_notes("artifact:doc-10"));
        assert!(!is_ledger_plan(
            "https://docs.google.com/document/d/df-10/edit"
        ));
        assert!(!is_ledger_plan(
            "https://docs.google.com/spreadsheets/d/df-1/edit"
        ));
    }

    #[test]
    fn a_two_sentence_reply_is_never_a_pasted_document() {
        assert!(!looks_like_a_pasted_document(
            "Added a rollback section with the three steps, and fixed the date in the summary."
        ));
    }

    #[test]
    fn a_long_reply_with_document_structure_is() {
        let pasted = format!(
            "Here is the updated document:\n\n# Ledger migration\n\n## Rollback\n\n- Stop writers\n- Restore the snapshot\n- Re-point DNS\n\n{}",
            "Background prose that makes this long enough to look like a document rather than a note. ".repeat(10)
        );
        assert!(looks_like_a_pasted_document(&pasted));
    }

    #[test]
    fn a_long_reply_without_structure_is_not_accused() {
        // Someone explaining a big edit in prose is not pasting a document, and
        // a false accusation here would push models toward terse replies.
        let prose = "I made the change and here is why it took a few passes. ".repeat(20);
        assert!(!looks_like_a_pasted_document(&prose));
    }

    #[test]
    fn one_heading_alone_is_not_enough() {
        // A reply that quotes the heading it added is being helpful, not
        // reproducing the file.
        let quoting = format!(
            "I added a section called:\n\n## Rollback\n\n{}",
            "and put the three steps under it, one line each. ".repeat(20)
        );
        assert!(!looks_like_a_pasted_document(&quoting));
    }

    #[test]
    fn the_def_grades_the_prompt_the_product_actually_sends() {
        // The whole reason WORK_MODE_PROMPT is imported: a copy would pass for
        // ever while the surface drifted. This asserts the def and the surface
        // cannot disagree.
        let d = work_mode_harness();
        let rendered = (d.render)(
            &input_json("anything"),
            &RenderContext {
                widened: false,
                model: String::new(),
            },
        )
        .expect("the render succeeds");
        let system: Vec<&str> = rendered
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect();
        // The mode prompt, then the pane's focus line — the order /api/chat
        // pushes them in on a work turn.
        assert_eq!(system, vec![WORK_MODE_PROMPT, focus_line(None).as_str()]);
    }

    #[test]
    fn every_fixture_has_a_distinct_name() {
        let mut names: Vec<&str> = fixtures().iter().map(|f| f.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "two fixtures share a name");
        assert_eq!(before, 9, "the header says NINE FIXTURES");
    }
}
