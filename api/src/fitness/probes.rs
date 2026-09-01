// TIER-1 CAPABILITY PROBES — ~10 cheap calls that establish model-level FACTS.
//
// WHY THIS FILE IS THE POINT OF THE WHOLE HARNESS REFACTOR
//   Talaria has to be decent on a 7-14B self-hosted model and excellent on a
//   frontier one, and an admin has to be able to tell WHICH, per role, from the
//   UI. `capability.rs` is the type that answers that; the runner reads it for
//   the floor and for widening. Nothing has ever WRITTEN it in the affirmative.
//
//   THIS IS THE FIRST PRODUCTION WRITER OF `value: true`. Until now the only
//   affirmative-fact writer was the gateway's learned-parameter path — it
//   writes `value: false` when an upstream 400s on a contract parameter — so
//   every capability-gated behavior in the product (floor refusals, widening)
//   has never fired in anger. That cuts both ways and both ways are dangerous:
//
//     a false `true`   silently widens a model's surface across the app. The
//                      Inbox hands a 7B model the item's whole action list
//                      because `tool-select` says it earned it.
//     a false `false`  refuses a working model. Probe facts DO NOT EXPIRE (see
//                      capability.rs) — a learned fact ages out in 30 days, a
//                      probe fact is a deliberate measurement and stands until
//                      someone re-measures. A wrong one is forever.
//
//   So every verdict below is asymmetric on purpose. Proof of PRESENCE is
//   allowed to be a single verified observation; proof of ABSENCE has to come
//   from a check that cannot fail for an unrelated reason. Where neither is
//   available the probe writes NOTHING, and an absent fact means unknown, which
//   `missing_capabilities` already treats as safe.
//
// WHAT RUNS AND WHAT DOES NOT, as of the tool-definition slot landing
//   `tools` and `tool-select` are ARMED: `TransportRequest.tool_defs` carries
//   real definitions to the model and `TransportReply.tool_calls` reports what
//   it called, so the fact that widens the Inbox command harness (audit 1.8)
//   can finally be recorded. They still SKIP on a fleet persona, whose tool loop
//   runs inside the agent container where we can neither place our tools nor see
//   the call — a skip, never a `false`, because nothing about the model was
//   measured. `vision` runs through `ProbeDeps.ask_with_images`, the seam the
//   transport layer owns (`gateway_image_turn` / `persona_probe_turn`), because
//   a measurement does not require the harness tree's `Message.content` to
//   become a content-parts union.
//
// THE THREE RULES
//   1. DETERMINISTIC SCORING ONLY. No LLM-as-judge anywhere in tier 1. If it
//      cannot be checked with code — a parse, a string equality, a clock, an
//      HTTP GET, a boa run of the assertions (code_runner.rs) — it is not a
//      probe. That keeps the suite fast, cheap, and free of the
//      who-judges-the-judge regress.
//   2. A PROBE THAT ERRORS WRITES NOTHING. Transport down, 401, gateway
//      restarting: those are facts about the deployment, not about the model.
//      `run_probes` distinguishes "the transport threw" from "the model
//      answered badly" by watching the transport itself, not by string-matching
//      an error message.
//   3. THE ESTIMATE IS DATA. `estimate_probes` returns calls and tokens (and a
//      price when one is known) so the admin UI can show a number before
//      anyone spends money. Nothing in here prints.
//
// HOW A PROBE REACHES THE MODEL
//   Through `run_harness` with `ctx.model` pinned — the same runner, the same
//   `dispatch_transport` selection, so a fleet persona is probed exactly the
//   way a harness turn on it would run. Four dependencies are injected on every
//   probe call and each one closes a specific way a probe could lie:
//
//     missing_capabilities -> []   A PROBE MEASURES THE MODEL, NOT THE RECORD.
//                                  The runner suppresses `response_format` when
//                                  a `json: false` fact exists, so without this a
//                                  re-probe after one bad 400 could never observe
//                                  the model honoring JSON mode again — the
//                                  ratchet the TTL exists to release, reinstated
//                                  in the one tool built to release it.
//     capabilities -> {}          Same reason, for the widening gate.
//     record_run -> no-op         `harness_runs` is the OBSERVED half of the
//                                  fitness matrix. Synthetic probe traffic in it
//                                  would corrupt the production contract rate the
//                                  page reads beside these facts.
//     record_findings -> no-op    `guard_findings.model` is the live
//                                  confabulation rate per model. Probe prompts
//                                  are adversarial-ish by construction and would
//                                  inflate it.
//
// THE CODE PROBE'S EXECUTOR lives one module over (code_runner.rs): `CodeTask`,
// `CODE_TASKS`, `extract_code`, `same_value`, `CODE_TIMEOUT_MS` and
// `run_code_task` crossed ahead of this file because they are the one place in
// the fitness family that executes a program a MODEL wrote. This file imports
// `CODE_TASKS` and `run_code_task` and adds nothing to them.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use url::Url;

use crate::capability::{
    CapabilityFact, capability_key, get_capabilities, merge_capabilities,
};
use crate::fitness::code_runner::{CODE_TASKS, run_code_task};
use crate::fitness::live_feed::note_live;
use crate::fitness::surface::{EvalLogLine, LogVerdict};
use crate::gateway::guard::{GuardConfig, GuardMode};
use crate::gateway::params::{epoch_to_iso, now_ms};
use crate::gateway::registry::routing_for;
use crate::gateway::upstream::gateway_pulse;
use crate::gateway::usage::estimate_tokens;
use crate::harness::define::{
    GuardDecl, HarnessDefinition, Message, OnFailure, Output, RenderContext, RoleFloor, VerifyFn,
};
use crate::harness_model::ModelSpec;
use crate::harness::run::{
    BoxFut, HarnessDeps, RunContext, TransportFn, run_harness,
};
use crate::harness::schema::{Field, Schema};
use crate::harness::transport::{
    ToolCall, ToolDefinition, TransportRequest, dispatch_transport,
    gateway_image_turn, offers_tool_definitions, persona_probe_turn,
};
use crate::model_catalog::advertised_window;
use crate::persona::persona_capability_keys;
use crate::price_oracle::TokPrice;
use crate::safe_fetch::{SafeFetch, safe_fetch};
use crate::state::AppState;

// ── What a probe is ──────────────────────────────────────────────────────────

/// Every probe id is also the `Capability` it writes, deliberately: a probe that
/// scored something no capability names would be a number with nowhere to go.
///
/// Declared in the TS union's order (not capability.rs's), so the registry test
/// comparing this set against `ALL_CAPABILITIES` sorts both — a tenth member of
/// the capability union with no probe has to fail there, whichever side adds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeId {
    Json,
    JsonStrict,
    Tools,
    ToolSelect,
    InstructionFollowing,
    Search,
    LongContext,
    Code,
    Vision,
}

impl ProbeId {
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeId::Json => "json",
            ProbeId::JsonStrict => "json-strict",
            ProbeId::Tools => "tools",
            ProbeId::ToolSelect => "tool-select",
            ProbeId::InstructionFollowing => "instruction-following",
            ProbeId::Search => "search",
            ProbeId::LongContext => "long-context",
            ProbeId::Code => "code",
            ProbeId::Vision => "vision",
        }
    }
}

/// One graded observation inside a probe.
///
/// `ok: None` IS THE LOAD-BEARING CASE and it is not a stylistic nicety. A
/// search trial whose cited page answers 403 to a bare GET told us nothing about
/// the model; counting it as a failure would write `search: false` — permanently
/// — about a model that searched correctly. Inconclusive trials leave the
/// denominator, and a probe with an empty denominator writes nothing.
#[derive(Debug, Clone, Serialize)]
pub struct Trial {
    pub name: String,
    pub ok: Option<bool>,
    /// One line a human reads in the admin drill-down.
    pub note: String,
    /// The model's reply, bounded. None when the trial never got one.
    pub raw: Option<String>,
}

/// The scored answer: what to write, or None for "we did not learn anything".
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProbeVerdict {
    pub value: bool,
    /// Pass rate over the CONCLUSIVE trials, 0..1.
    pub score: f64,
    /// One line, written for the admin looking at the fitness matrix.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ProbeOutcome {
    Scored {
        verdict: ProbeVerdict,
        trials: Vec<Trial>,
    },
    /// Nothing to measure here (no vision advertised, no context window known, a
    /// fleet candidate whose tool loop is not ours to drive). Not a failure and
    /// not a fact — `reason` is the sentence the admin reads instead of a cell.
    Skipped {
        reason: String,
        trials: Vec<Trial>,
    },
    /// WE ALREADY MEASURED THIS, so no call was made and the standing fact is
    /// reported instead.
    ///
    /// DELIBERATELY NOT `skipped`, and the difference is the whole point of the
    /// kind. A skip means NO FACT EXISTS — the channel could not be opened, and
    /// an admin reading it should conclude nothing. This means a fact exists, we
    /// wrote it, and it still stands; the verdict below is that fact, with the
    /// date it was measured. Folding the two together would make a probed
    /// capability read as unmeasured the moment we stopped re-paying for it.
    ///
    /// WHY IT IS SAFE TO REUSE THE ANSWER. A probe fact is a property of an
    /// `endpoint:model`, `probe_keys` refuses to write when the id is ambiguous,
    /// and a re-pointed model id is exactly what "Forget recorded capabilities"
    /// is for. Nothing else about a deployment can change what a past measurement
    /// established.
    Known {
        verdict: ProbeVerdict,
        at: String,
        trials: Vec<Trial>,
    },
    /// The deployment failed, not the model. Writes nothing, by rule 2.
    Errored {
        reason: String,
        trials: Vec<Trial>,
    },
}

/// One model call, normalized — the unit every scorer in this file takes, which
/// is what makes the scorers testable against recorded replies with no gateway,
/// no database and no clock anywhere near them.
#[derive(Debug, Clone, Serialize)]
pub struct Attempt {
    /// The reply, or '' when none arrived.
    pub raw: String,
    /// THE DEPLOYMENT FAILED, not the model: the transport threw. Some makes
    /// the whole probe error out, because one 401 must not be scored as a model
    /// that answered badly nine times.
    pub transport_error: Option<String>,
    /// Did the call ask for JSON at the PROTOCOL level (`response_format`)?
    pub json_requested: bool,
    /// Did the gateway report the constraint stripped on the way out (audit 1.2)?
    /// This is the silent-strip case, and it is why a reply that happens to parse
    /// is still not evidence of a `json` capability.
    pub contract_dropped: bool,
    /// The output contract held: parsed and schema-valid, first attempt.
    pub contract_held: bool,
}

impl Attempt {
    /// The blank every `ask` variant starts from — nothing observed yet.
    fn blank() -> Attempt {
        Attempt {
            raw: String::new(),
            transport_error: None,
            json_requested: false,
            contract_dropped: false,
            contract_held: false,
        }
    }
}

/// A native tool call the model made — the transport's own type, aliased rather
/// than re-declared. Two structurally identical shapes across the seam is how a
/// scorer starts reading a field the transport stopped filling.
pub type ProbeToolCall = ToolCall;

const RAW_CAP: usize = 1_200;
fn bounded(s: &str) -> Option<String> {
    // TS `s ? s.slice(0, 1_200) : null` — an empty reply is "no reply", not a
    // zero-length one, and the cap is UTF-16 because JS `.slice` is.
    if s.is_empty() {
        None
    } else {
        Some(crate::body::truncate_utf16(s, RAW_CAP).to_string())
    }
}

// ── The fixtures ─────────────────────────────────────────────────────────────
//
// Static, so `estimate_probes` can size a run exactly rather than guessing, and
// so a scorer test can drive the same prompt the production probe sends.

fn sys(content: &str) -> Message {
    Message::system(content)
}
fn usr(content: &str) -> Message {
    Message::user(content)
}

/// Every probe opens with this. Small models follow a terse system line better
/// than a chatty one, and a probe that lost a trial to a preamble would be
/// measuring our prompt rather than the model.
fn terse() -> Message {
    sys("You answer exactly what is asked, with nothing else. No preamble, no explanation, no markdown fence.")
}

/// A named prompt plus its messages — the element type of every trial fixture.
struct PromptFixture {
    name: &'static str,
    messages: Vec<Message>,
}

/// A named prompt whose reply is checked against an exact expected string.
struct ExpectedFixture {
    name: &'static str,
    messages: Vec<Message>,
    expect: &'static str,
}

// THE OUTPUT CONTRACT a probe asks for: the runner's `Schema` plus, where zod
// could say something this build's schema algebra cannot (an array-length
// minimum), a `VerifyFn` running the same check beside it. A verify failure
// keeps `schema_valid` false in the runner, so `contract_held` stays faithful to
// the TS zod schema, and the probes always set `repair: Some(0)` so neither the
// schema's nor the verify's failure sentence is ever shown to the model.
#[derive(Clone)]
pub struct JsonContract {
    pub schema: Schema,
    pub verify: Option<VerifyFn>,
}

impl JsonContract {
    fn bare(schema: Schema) -> JsonContract {
        JsonContract { schema, verify: None }
    }
}

static JSON_TRIVIAL: LazyLock<JsonContract> = LazyLock::new(|| {
    JsonContract::bare(Schema::Object(vec![
        Field::required("name", Schema::string()),
        Field::required("count", Schema::Num),
        Field::required("ok", Schema::Bool),
    ]))
});

/// Three phrasings of the same trivial object. Three rather than one because a
/// single lucky parse is not a capability, and rather than five because this
/// probe's real question is the PROTOCOL one (did `response_format` survive),
/// which one call already answers.
static JSON_TRIALS: LazyLock<Vec<PromptFixture>> = LazyLock::new(|| {
    vec![
        PromptFixture {
            name: "trivial object",
            messages: vec![
                terse(),
                usr("Return a JSON object with the keys name (the string \"talaria\"), count (the number 3) and ok (the boolean true)."),
            ],
        },
        PromptFixture {
            name: "trivial object, reordered",
            messages: vec![
                terse(),
                usr("Return JSON. It must have ok set to true, count set to 3, and name set to \"talaria\"."),
            ],
        },
        PromptFixture {
            name: "trivial object, restated",
            messages: vec![
                terse(),
                usr("Give me one JSON object describing a record whose name is \"talaria\", whose count is 3, and which is ok."),
            ],
        },
    ]
});

static JSON_STRICT: LazyLock<JsonContract> = LazyLock::new(|| {
    JsonContract {
        schema: Schema::Object(vec![
            Field::required("id", Schema::Str { trim: false, min: Some(1), max: None }),
            Field::required(
                "tags",
                Schema::Array(Box::new(Schema::Str { trim: false, min: Some(1), max: None })),
            ),
            Field::required(
                "items",
                Schema::Array(Box::new(Schema::Object(vec![
                    Field::required("label", Schema::Str { trim: false, min: Some(1), max: None }),
                    Field::required("weight", Schema::Num),
                ]))),
            ),
            // The long string field is where small models break structured
            // output: they start the prose, forget they are inside JSON, and
            // close the object early or emit an unescaped newline. `min(200)` is
            // the point of the field.
            Field::required("summary", Schema::Str { trim: false, min: Some(200), max: None }),
        ]),
        // zod's `.min(2)` on the two arrays has no spelling in the schema
        // algebra (there is an `ArrayMax`, no `ArrayMin`), so the same check
        // rides in the verify slot. The runner runs it after the schema, a
        // failure fails the contract exactly as zod's would, and with
        // `repair: Some(0)` the sentence never reaches the model.
        verify: Some(Arc::new(
            |_input: &Value, reply: &Value, _ctx: &RenderContext| {
                let tags = reply
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|a| a.len())
                    .unwrap_or(0);
                if tags < 2 {
                    return Ok(Some("tags must contain at least 2 short lowercase strings".into()));
                }
                let items = reply
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|a| a.len())
                    .unwrap_or(0);
                if items < 2 {
                    return Ok(Some("items must contain at least 2 objects".into()));
                }
                Ok(None)
            },
        )),
    }
});

const JSON_STRICT_INSTRUCTION: &str = "Return exactly one JSON object with these keys and nothing else:\n  id       a short string identifier\n  tags     an array of at least 2 short lowercase strings\n  items    an array of at least 2 objects, each { \"label\": string, \"weight\": number }\n  summary  a single string of AT LEAST 200 characters";

static JSON_STRICT_TRIALS: LazyLock<Vec<PromptFixture>> = LazyLock::new(|| {
    [
        "a small coastal town",
        "a distributed build cache",
        "a rescued greyhound",
        "a municipal water tender",
        "a used bookshop",
    ]
    .iter()
    .map(|subject| PromptFixture {
        name: Box::leak(format!("nested object: {subject}").into_boxed_str()),
        messages: vec![
            terse(),
            usr(&format!("{JSON_STRICT_INSTRUCTION}\n\nThe object describes: {subject}.")),
        ],
    })
    .collect()
});

/// The classic small-model tell, and the one the titler and the summarizer lean
/// on hardest: both end their prompt with "reply with ONLY the title/summary",
/// and both are `output: Text`, so a model that adds "Sure! Here you go:"
/// fails their contract on every single run.
static INSTRUCTION_TRIALS: LazyLock<Vec<ExpectedFixture>> = LazyLock::new(|| {
    vec![
        ExpectedFixture {
            name: "exactly OK",
            messages: vec![terse(), usr("Reply with exactly the word OK. No punctuation, no explanation.")],
            expect: "OK",
        },
        ExpectedFixture {
            name: "exactly three words",
            messages: vec![terse(), usr("Reply with exactly these three words in this order and nothing else: red green blue")],
            expect: "red green blue",
        },
        ExpectedFixture {
            name: "exactly one digit",
            messages: vec![terse(), usr("Reply with the single digit 7 and nothing else.")],
            expect: "7",
        },
    ]
});

static SEARCH_CONTRACT: LazyLock<JsonContract> = LazyLock::new(|| {
    JsonContract::bare(Schema::Object(vec![
        Field::required("date", Schema::string()),
        Field::required("url", Schema::string()),
        Field::required("quote", Schema::string()),
    ]))
});

const SEARCH_INSTRUCTION: &str = "Search the web right now, open one page, and reply with exactly one JSON object:\n  {\"date\": \"<today's date in UTC as YYYY-MM-DD>\", \"url\": \"<the full https URL of the page you opened>\", \"quote\": \"<a verbatim sentence of at least 40 characters copied from that page>\"}";

static SEARCH_TRIALS: LazyLock<Vec<PromptFixture>> = LazyLock::new(|| {
    vec![
        PromptFixture {
            name: "fresh page, verbatim quote",
            messages: vec![
                terse(),
                usr(&format!("{SEARCH_INSTRUCTION}\n\nThe page must be a news story published in the last two days.")),
            ],
        },
        PromptFixture {
            name: "fresh page, verbatim quote (second topic)",
            messages: vec![
                terse(),
                usr(&format!("{SEARCH_INSTRUCTION}\n\nThe page must be documentation for a software project, opened today.")),
            ],
        },
    ]
});

// ── Scorers: pure, deterministic, and the entire test surface ────────────────

/// Pass rate over the CONCLUSIVE trials. None when nothing was conclusive —
/// which is the signal to write no fact at all.
pub fn rate_of(trials: &[Trial]) -> Option<f64> {
    let graded: Vec<&Trial> = trials.iter().filter(|t| t.ok.is_some()).collect();
    if graded.is_empty() {
        return None;
    }
    let passed = graded.iter().filter(|t| t.ok == Some(true)).count();
    Some(passed as f64 / graded.len() as f64)
}

fn pct(n: f64) -> String {
    format!("{}%", (n * 100.0).round() as i64)
}

/// The reason the first failing trial gives, for the one-line detail.
fn first_failure(trials: &[Trial]) -> String {
    trials
        .iter()
        .find(|t| t.ok == Some(false))
        .map(|t| t.note.clone())
        .unwrap_or_else(|| "no failure recorded".into())
}

/// `json` — a trivial object requested WITH `response_format`.
///
/// THE SILENT-STRIP CASE (audit 1.2) IS THE POINT. The gateway learns
/// unsupported parameters from upstream 400s and pre-strips them forever after;
/// `response_format` was as strippable as `top_p`, so a model that refuses JSON
/// mode had the constraint deleted, the retry SUCCEEDED, and the caller — which
/// had asked for JSON precisely because it was about to run a JSON parser — got
/// free prose. A model whose reply happens to parse in that state does NOT have
/// this capability: the next caller with a harder schema gets prose, and the
/// runner's `json_mode` will keep asking for something the endpoint throws away.
/// So a reported drop changes the sentence, never the verdict.
pub fn score_json(
    trials: &[Trial],
    protocol: JsonProtocol,
) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    if !protocol.requested {
        // Cannot happen through `run_probes` — it injects an empty capability
        // record precisely so the runner always asks. Kept because a caller
        // supplying its own `ask` could reach it, and "we never tested the thing
        // we are about to record" must not become a recorded fact.
        return None;
    }
    // THIS FACT IS ABOUT THE MODEL, NOT THE ENDPOINT, and it used to be about
    // both. A dropped `response_format` returned `value: false` even when every
    // reply parsed — the detail said so in as many words ("replies still parsed
    // 100% of the time, but the JSON constraint is not honored here"). One word
    // therefore carried two unrelated claims: "this model cannot produce JSON"
    // and "this server does not implement response_format".
    //
    // THAT CONFLATION BECAME LOAD-BEARING the moment a JSON harness put `json`
    // in its floor: a self-hosted llama.cpp or Ollama box whose models emit
    // perfect JSON would have had all nine structured harnesses declared unfit,
    // for a property of the SERVER. The deployment half is already tracked where
    // it belongs — `contract_dropped` on the reply, and the gateway's
    // learned-param ratchet, which is what suppresses the parameter on later
    // calls.
    //
    // So the verdict is the parse rate either way, and the drop only changes the
    // sentence: on a dropped parameter this measured whether the model returns
    // JSON when ASKED IN PROSE, which is the harder question and the one a floor
    // should turn on.
    if rate < 1.0 {
        return Some(ProbeVerdict {
            value: false,
            score: rate,
            detail: format!(
                "only {} of {} JSON-mode calls returned a usable object - {}",
                pct(rate),
                trials.len(),
                first_failure(trials)
            ),
        });
    }
    Some(if protocol.dropped {
        ProbeVerdict {
            value: true,
            score: 1.0,
            detail: format!(
                "returned a valid object on all {} calls, though this endpoint dropped response_format - the model produces JSON from the prompt alone",
                trials.len()
            ),
        }
    } else {
        ProbeVerdict {
            value: true,
            score: 1.0,
            detail: format!(
                "honored response_format and returned a valid object on all {} calls",
                trials.len()
            ),
        }
    })
}

/// What the json probe observed about the PROTOCOL, distilled from the attempts.
#[derive(Debug, Clone, Copy)]
pub struct JsonProtocol {
    pub requested: bool,
    pub dropped: bool,
}

/// `json-strict` — nested arrays plus a 200-character string field, scored as a
/// conformance RATE rather than pass/fail. A model at 4/5 is genuinely usable
/// behind the runner's repair turn; one at 1/5 is not, and the number is what
/// tells them apart.
const JSON_STRICT_FLOOR: f64 = 0.8;

pub fn score_json_strict(trials: &[Trial]) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    Some(if rate >= JSON_STRICT_FLOOR {
        ProbeVerdict {
            value: true,
            score: rate,
            detail: format!(
                "{} of {} nested-schema objects conformed on the first attempt",
                pct(rate),
                trials.len()
            ),
        }
    } else {
        ProbeVerdict {
            value: false,
            score: rate,
            detail: format!(
                "only {} of {} nested-schema objects conformed - {}",
                pct(rate),
                trials.len(),
                first_failure(trials)
            ),
        }
    })
}

/// `tools` — one tool definition, one prompt that cannot be answered without
/// calling it. Any correct call is proof; nothing else is.
pub fn score_tools(trials: &[Trial]) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    Some(if rate == 1.0 {
        ProbeVerdict { value: true, score: 1.0, detail: "called the offered tool with well-formed arguments".into() }
    } else {
        ProbeVerdict {
            value: false,
            score: rate,
            detail: format!("did not call the offered tool - {}", first_failure(trials)),
        }
    })
}

/// `tool-select` — 4 tools, 4 prompts, one correct tool each.
///
/// STRICT ON PURPOSE: this is the capability that WIDENS the Inbox command
/// harness from a regex-chosen single action to the item's whole action list
/// (audit 1.8). A model that picks right 3 times out of 4 has not earned that —
/// the fourth pick is an action taken on somebody's ticket. Anything below 4/4
/// is `false`, and the score says how close it got.
pub fn score_tool_select(trials: &[Trial]) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    let graded = trials.iter().filter(|t| t.ok.is_some()).count();
    Some(if rate == 1.0 {
        ProbeVerdict {
            value: true,
            score: 1.0,
            detail: format!("picked the correct tool on all {graded} prompts"),
        }
    } else {
        ProbeVerdict {
            value: false,
            score: rate,
            detail: format!(
                "picked the correct tool on {} of {graded} prompts - widening needs all of them",
                pct(rate)
            ),
        }
    })
}

/// `instruction-following` — "reply with exactly the word OK" and two siblings.
///
/// Compared after `trim()` and nothing else. A model that answers "OK." or
/// "Sure — OK" did not do what it was told, and every text harness in the
/// product (titler, summarizer, librarian) ends its prompt with exactly this
/// kind of instruction. Being generous here would score a model as passing a
/// test the product then fails it on.
pub fn score_instruction(trials: &[Trial]) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    Some(if rate == 1.0 {
        ProbeVerdict {
            value: true,
            score: 1.0,
            detail: format!("reproduced all {} exact-output instructions verbatim", trials.len()),
        }
    } else {
        ProbeVerdict {
            value: false,
            score: rate,
            detail: format!(
                "followed {} of {} exact-output instructions - {}",
                pct(rate),
                trials.len(),
                first_failure(trials)
            ),
        }
    })
}

/// `search` — ASYMMETRIC, and this is the most carefully-hedged verdict here.
///
/// A pass means ONE ATTEMPT did the whole thing: named today's date, cited a
/// URL, and quoted a sentence WE THEN FETCHED AND FOUND on that page.
///
/// A `false` is much harder to earn, because probe facts never expire and this
/// one gates `research-recon`: it requires a trial that failed the DATE check,
/// which needs no network of ours and no cooperation from the cited host. A
/// model that got the date right and then failed only on the quote lands
/// INCONCLUSIVE — plenty of search models cite pages that answer a bare GET with
/// 403, and refusing research to one of them for good would be worse than
/// knowing nothing.
pub const SEARCH_DATE_TRIAL: &str = "date";
pub const SEARCH_CITATION_TRIAL: &str = "citation";

/// The attempt a trial belongs to. `search_trials` names both of one reply's
/// observations `${name} / date` and `${name} / citation`, so the part before
/// the separator is the reply they were read off.
fn attempt_of(name: &str) -> &str {
    name.split(" / ").next().unwrap_or(name)
}

pub fn score_search(trials: &[Trial]) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    // A VERIFIED QUOTE IS NOT PROOF ON ITS OWN, which this used to assume — the
    // quote check asks whether the sentence is on the page, and a model with a
    // large memorized corpus answers that from training data. `deepseek-v4-pro`
    // did exactly that: it passed the citation check on one attempt out of three
    // and was written `search: true` FOREVER, on an endpoint that returns no
    // citations at all. Research then ran its search stages natively on a model
    // that never searched, and every run died with an empty source registry.
    //
    // SO THE TWO OBSERVATIONS HAVE TO CORROBORATE EACH OTHER, and from the SAME
    // REPLY. A model that really searched knows what day it is; one quoting a
    // page it remembers is answering a question about the past. Neither check is
    // sufficient alone — the date is stamped into plenty of system prompts, and
    // the quote is memorizable — but a reply that lands both did the work.
    // Trials from different attempts are not evidence about each other, which is
    // what pairing by attempt enforces.
    let searched_for_real = trials.iter().any(|t| {
        t.ok == Some(true)
            && t.name.contains(SEARCH_CITATION_TRIAL)
            && trials.iter().any(|d| {
                d.ok == Some(true)
                    && d.name.contains(SEARCH_DATE_TRIAL)
                    && attempt_of(&d.name) == attempt_of(&t.name)
            })
    });
    if searched_for_real {
        return Some(ProbeVerdict {
            value: true,
            score: rate,
            detail: "named today’s date and quoted a sentence that is actually on the page it cited".into(),
        });
    }
    let stale_date: Vec<&Trial> = trials
        .iter()
        .filter(|t| t.ok == Some(false) && t.name.contains(SEARCH_DATE_TRIAL))
        .collect();
    if stale_date.len() >= 2 {
        return Some(ProbeVerdict {
            value: false,
            score: rate,
            detail: format!(
                "no live data: {}",
                stale_date
                    .first()
                    .map(|t| t.note.clone())
                    .unwrap_or_else(|| "the model could not name today’s date".into())
            ),
        });
    }
    // INCONCLUSIVE, and this is where a verified-quote-but-stale-date model now
    // lands. It is the right answer for it: nothing here can tell a search model
    // having a bad day from a model with a good memory, and `capability_reach`
    // sends the run through a real search tool either way.
    None
}

/// `long-context` — a needle at 50% and 90% of the window actually tested. Both
/// depths have to land: a model that finds the needle halfway in and loses it at
/// 90% is exactly the model that will drop the last of a long transcript.
pub fn score_long_context(trials: &[Trial], tested: i64, assumed: bool) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    let window = grouped(tested);
    // SAY WHEN THE WINDOW WAS ASSUMED. The measurement is exactly as real either
    // way — a needle either came back or it did not — but "we tested 32,000
    // tokens because that is our ceiling" and "we tested 32,000 tokens because
    // that is what this model advertises" support different conclusions, and an
    // admin reading a green tag deserves to know which they have.
    let how = if assumed {
        format!("{window}-token prompt (this model advertises no window, so the probe used its own ceiling)")
    } else {
        format!("{window}-token prompt")
    };
    Some(if rate == 1.0 {
        ProbeVerdict {
            value: true,
            score: 1.0,
            detail: format!("found the needle at 50% and 90% depth in a {how}"),
        }
    } else {
        ProbeVerdict {
            value: false,
            score: rate,
            detail: format!("found the needle in {} of a {how} - {}", pct(rate), first_failure(trials)),
        }
    })
}

/// `code` — graded by RUNNING the assertions, never by another model's opinion.
pub fn score_code(trials: &[Trial]) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    Some(if rate == 1.0 {
        ProbeVerdict { value: true, score: 1.0, detail: "every code task passed every assertion when run".into() }
    } else {
        ProbeVerdict {
            value: false,
            score: rate,
            detail: format!(
                "{} of the code tasks passed their assertions - {}",
                pct(rate),
                first_failure(trials)
            ),
        }
    })
}

/// `vision` — only ever reached when the endpoint advertises it.
pub fn score_vision(trials: &[Trial]) -> Option<ProbeVerdict> {
    let rate = rate_of(trials)?;
    Some(if rate == 1.0 {
        ProbeVerdict { value: true, score: 1.0, detail: "read every probe image correctly".into() }
    } else {
        ProbeVerdict {
            value: false,
            score: rate,
            detail: format!(
                "read {} of the probe images correctly - {}",
                pct(rate),
                first_failure(trials)
            ),
        }
    })
}

// ── Deterministic checks the scorers are built from ──────────────────────────

/// Hosts a model reaches for when it is inventing a citation. A URL here is a
/// fabricated source however well-formed it looks.
const PLACEHOLDER_HOSTS: [&str; 6] =
    ["example.com", "www.example.com", "example.org", "example.net", "localhost", "test.com"];

pub fn citation_problem(url: &str) -> Option<String> {
    // WHATWG `new URL` becomes the `url` crate; the same relative-URL throw
    // becomes `Url::parse`'s error, and `hostname` (already lowercased by both
    // parsers) becomes `host_str`.
    let parsed = match Url::parse(url) {
        Ok(parsed) => parsed,
        Err(_) => return Some("the citation is not an absolute URL".into()),
    };
    // WHATWG's `protocol` carries its own colon ("ftp:"); the url crate's
    // `scheme()` does not, so the sentence appends one and stays byte-identical
    // to the TS an admin may have already read.
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Some(format!("the citation is not http(s): {scheme}:"));
    }
    let host = parsed.host_str().unwrap_or("");
    if !host.contains('.') {
        return Some(format!("the citation has no real host: {host}"));
    }
    if PLACEHOLDER_HOSTS.contains(&host) {
        return Some(format!("the citation is a placeholder host: {host}"));
    }
    None
}

/// Days between an ISO date and now, or None when the string is not a date.
/// Tolerance is a full day in each direction because "today in UTC" is a
/// genuinely ambiguous question for a model whose provider stamps a local date
/// into its system prompt, and we are not measuring timezone arithmetic.
pub fn date_drift_days(date: &str, now: i64) -> Option<f64> {
    static SHAPE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("a static regex compiles"));
    if !SHAPE.is_match(date) {
        return None;
    }
    // `Date.parse` rejects 2026-13-45 with NaN; the calendar check replaces the
    // NaN with an Option, which is the same "not a date" answer.
    let year: i32 = date[0..4].parse().ok()?;
    let month: u32 = date[5..7].parse().ok()?;
    let day: u32 = date[8..10].parse().ok()?;
    let at = chrono::NaiveDate::from_ymd_opt(year, month, day)?
        .and_hms_opt(0, 0, 0)?
        .and_utc()
        .timestamp_millis();
    // UTC midnight of `now` — TS's `new Date(now).toISOString().slice(0, 10)`.
    let today = now.div_euclid(86_400_000) * 86_400_000;
    Some(((at - today).abs()) as f64 / 86_400_000.0)
}

/// Whitespace- and case-insensitive containment, with HTML tags removed from the
/// haystack. A search model quotes rendered text; the page we fetch is markup,
/// and the difference between them is tags and line wrapping, not content.
pub fn quote_appears(quote: &str, page: &str) -> bool {
    static TAGS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<[^>]*>").expect("a static regex compiles"));
    static WS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s+").expect("a static regex compiles"));
    let flat = |s: &str| -> String {
        WS.replace_all(&TAGS.replace_all(s, " ").replace("&nbsp;", " "), " ")
            .trim()
            .to_lowercase()
    };
    let needle = flat(quote);
    // JS `.length` is UTF-16 units, and the probe's instruction asks for "at
    // least 40 characters" in the units the model counts.
    if crate::body::utf16_len(&needle) < 40 {
        return false;
    }
    flat(page).contains(&needle)
}

// `extract_code`, `same_value`, `CODE_TIMEOUT_MS` and `run_code_task` are the
// code probe's executor and crossed already — see code_runner.rs, whose module
// header says why they live in a module of their own.

// ── The needle haystack ──────────────────────────────────────────────────────

/// Filler that is boring, uniform and countable, so the needle is the only thing
/// in the prompt worth retrieving and depth is a real position rather than an
/// artifact of where the interesting text happened to be.
pub fn haystack(tokens: i64, needle: &str, depth: f64) -> String {
    let mut lines: Vec<String> = Vec::new();
    // ~11 tokens a line at the 4-chars-per-token estimate the ledger uses.
    let target = std::cmp::max(2, tokens / 11);
    for i in 0..target {
        lines.push(format!("Line {}: routine maintenance log entry, nothing of note recorded today.", i + 1));
    }
    let at = std::cmp::min(lines.len(), std::cmp::max(1, (lines.len() as f64 * depth).round() as usize));
    lines.insert(at, needle.to_string());
    lines.join("\n")
}

pub const NEEDLE_QUESTION: &str =
    "What is the passphrase for record 4817? Reply with the passphrase and nothing else.";
pub fn needle_line(token: &str) -> String {
    format!("Line 0: IMPORTANT - the passphrase for record 4817 is {token}.")
}

/// Smallest prompt worth calling a long-context test. Below this the probe is
/// measuring ordinary recall and a `true` would overstate what was checked.
pub const MIN_LONG_CONTEXT_TOKENS: i64 = 8_000;

/// The default ceiling on how much context a probe will pay for.
///
/// A 200k-window model probed at 90% depth is ~180k prompt tokens per trial —
/// dollars, not cents, and this whole tier exists to reject a bad model in
/// seconds for pennies. The probe therefore tests a CAPPED window and says so in
/// the detail, so nobody reads `long-context: true` as a claim about the full
/// advertised window. Raise it per run when the answer matters more than the
/// bill.
pub const DEFAULT_MAX_CONTEXT_TOKENS: i64 = 32_000;

/// THE CLOCK ONE PROBE RACES. Generous, because `long-context` legitimately
/// sends two ~25k-token prompts and a reasoning model is slow per call — this is
/// a backstop against a hung transport, not a performance budget.
pub const PROBE_TIMEOUT_MS: u64 = 180_000;

// `toLocaleString('en-US')` for the two sentences that name a token count —
// an admin reads "25,600" as a size and "25600" as a serial number.
fn grouped(n: i64) -> String {
    let raw = n.to_string();
    let (sign, digits) = match raw.strip_prefix('-') {
        Some(digits) => ("-", digits),
        None => ("", raw.as_str()),
    };
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    format!("{sign}{out}")
}

// ── Injected edges ───────────────────────────────────────────────────────────

/// One probe call. `ask` is the plain text/JSON turn every probe but two takes;
/// the tool probes go through `ToolAskSpec` below, and the image channel is
/// `ImageAskSpec`.
pub struct AskSpec {
    pub id: String,
    pub messages: Vec<Message>,
    /// Present: request JSON at the protocol level and validate against it.
    pub schema: Option<JsonContract>,
}

impl AskSpec {
    /// The probe def's `output` for this ask — a JSON contract with no repair
    /// turns, or a plain text one. `repair: Some(0)` is deliberate: the probe
    /// measures the FIRST attempt, so a repair turn would replace the
    /// measurement with a second chance the fitness page does not mean.
    fn output(&self) -> Output {
        match &self.schema {
            Some(contract) => Output::Json {
                schema: contract.schema.clone(),
                preprocess: None,
                repair: Some(0),
                verify: contract.verify.clone(),
            },
            None => Output::Text { clean: None, verify: None },
        }
    }
}

/// A tool the probe OFFERS, which is exactly `TransportRequest.tool_defs`'s
/// element type. Aliased for the same reason as `ProbeToolCall`: the fixture the
/// test drives and the definition the model is sent must be one type.
pub type ToolSpec = ToolDefinition;

pub struct ToolAskSpec {
    pub id: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Debug, Clone)]
pub struct ToolAttempt {
    pub tool_calls: Vec<ProbeToolCall>,
    pub transport_error: Option<String>,
}

pub struct ImageAskSpec {
    pub id: String,
    pub messages: Vec<Message>,
    /// `data:` URLs. Never a remote fetch — a probe must not depend on a host.
    pub images: Vec<String>,
}

pub type AskFn = Arc<dyn Fn(AskSpec) -> BoxFut<Attempt> + Send + Sync>;
pub type ToolAskFn = Arc<dyn Fn(ToolAskSpec) -> BoxFut<ToolAttempt> + Send + Sync>;
pub type OffersFn = Arc<dyn Fn() -> BoxFut<bool> + Send + Sync>;
pub type ImageAskFn = Arc<dyn Fn(ImageAskSpec) -> BoxFut<Attempt> + Send + Sync>;
pub type ContextWindowFn = Arc<dyn Fn() -> BoxFut<Option<f64>> + Send + Sync>;
pub type AdvertisesFn = Arc<dyn Fn(ProbeId) -> BoxFut<bool> + Send + Sync>;
pub type FetchTextFn = Arc<dyn Fn(String) -> BoxFut<Option<String>> + Send + Sync>;
pub type RecordFn =
    Arc<dyn Fn(String, String, CapabilityFact) -> BoxFut<Result<(), String>> + Send + Sync>;
pub type MeasuredFn = Arc<dyn Fn(ProbeId) -> BoxFut<Option<CapabilityFact>> + Send + Sync>;
pub type KeysFn = Arc<dyn Fn() -> BoxFut<Vec<String>> + Send + Sync>;
pub type NowEdgeFn = Arc<dyn Fn() -> i64 + Send + Sync>;
pub type PriceFn = Arc<dyn Fn() -> BoxFut<Option<TokPrice>> + Send + Sync>;
pub type RunFn = Arc<dyn Fn(&ProbeDeps) -> BoxFut<ProbeOutcome> + Send + Sync>;

/// Every edge a probe reads, injected on every run.
///
/// THE ONE ADDITION THIS PORT MAKES: `keys`. The TS asks `probeKeys(model)`
/// directly, and its tests script the module seam (`vi.mock` on llm-gateway and
/// persona); Rust has no module mock, so the key derivation is an edge like the
/// rest — the default is the real `probe_keys`, and a test scripts it the way it
/// scripts `ask`. Everything else is field-for-field the TS `ProbeDeps`.
#[derive(Clone)]
pub struct ProbeDeps {
    /// The pinned-candidate call. Default: `run_harness` with `ctx.model` set.
    pub ask: AskFn,
    /// THE SIXTH ASK, ARMED. `TransportRequest` now carries tool DEFINITIONS
    /// alongside the tool POLICY it always had (transport.rs), and
    /// `TransportReply.tool_calls` reports what the model called, so both tool
    /// probes make real calls and score real answers.
    ///
    /// What is still NOT allowed, and the reason this took a transport slot
    /// rather than an afternoon: a prompt-level imitation. "Reply with the name
    /// of the tool you would call" measures instruction following, and recording
    /// its result as `tools: true` would be exactly the false `true` this file
    /// exists to avoid — permanently, since probe facts do not expire.
    pub ask_with_tools: ToolAskFn,
    /// CAN this candidate be offered tool definitions at all? False for a FLEET
    /// PERSONA: its tool loop runs inside the agent container, so tools we offer
    /// are neither guaranteed to reach the model nor observable when called, and
    /// the fleet transport refuses the call outright.
    ///
    /// Asked BEFORE the call so the probes SKIP instead of erroring. A refusal
    /// thrown from the transport would land as `Errored`, which by rule 2 means
    /// "the deployment failed" — and a perfectly healthy persona is not a broken
    /// deployment. `estimate_probes` reads the same edge, so the priced call
    /// count matches the calls a run actually makes.
    pub offers_tool_definitions: OffersFn,
    /// THE CHANNEL THAT STILL SHUTS BY DEFAULT, and deliberately so.
    /// `Message.content` is a string by construction (see the note on `Message`
    /// in define.rs): widening it to the OpenAI content-parts union is a
    /// tree-wide change — the gateway renderers, the grounding text and tool
    /// record the guard pass is built from, both token estimates, 23 harness
    /// renders — and a union only some transports honored would meter
    /// `[object Object]` and ground the guard against nothing. A half-widened
    /// content type is worse than none, so `vision` skips and says which wall
    /// it hit.
    ///
    /// The DEFAULT is `Some(runner_image_ask)` — the transport layer's own
    /// image seam — so the channel is open wherever the deployment can serve
    /// it. A test (or a caller) sets None to represent the closed channel.
    pub ask_with_images: Option<ImageAskFn>,
    /// The advertised context window of the endpoint serving this model, or None
    /// when nothing advertises one.
    pub context_window: ContextWindowFn,
    /// Does anything DECLARE this capability for the model? Only the vision probe
    /// asks, and only so it can skip cleanly on an endpoint that never claimed
    /// it. A `declared` fact is the advertisement; the probe is the verification.
    pub advertises: AdvertisesFn,
    /// Fetch a cited page as text for the search probe's quote check. None (a
    /// failed fetch, a 403, a timeout) makes the trial inconclusive, never
    /// failed.
    pub fetch_text: FetchTextFn,
    pub record: RecordFn,
    /// THE FACT WE ALREADY MEASURED for this capability, or None.
    ///
    /// Only a fact whose `source` is `probe` counts. A `declared`, `catalog` or
    /// `learned` fact is a CLAIM or an inference, and the whole job of tier 1 is
    /// to verify those — treating one as "already measured" would mean a model
    /// catalog's marketing copy could permanently prevent us from checking it.
    ///
    /// None for an ambiguous or unroutable id, because nothing was ever written
    /// under a key for it (`run_probes`'s ambiguity rule), so there is nothing
    /// to reuse and every probe runs.
    pub measured: MeasuredFn,
    /// The capability keys this candidate's facts belong under. The TS reads
    /// this through `probeKeys` at two removes (its own `advertises`/`measured`
    /// defaults and `run_probes`); here it is one edge those three share.
    pub keys: KeysFn,
    pub now: NowEdgeFn,
    pub max_context_tokens: i64,
    /// The needle. An argument so a test is not at the mercy of a random value.
    pub needle_token: String,
    /// Prices for the estimate, $/MTok. None when nothing prices this model.
    pub price: PriceFn,
}

// ── The default `ask`: run_harness with the candidate pinned ──────────────────

fn probe_def(
    id: &str,
    messages: Vec<Message>,
    output: Output,
    tools: Vec<ToolSpec>,
) -> HarnessDefinition {
    // The id and label are formatted per ask, and `HarnessDefinition` demands
    // `&'static str` for both. Leaking a few dozen bytes per probe ask — at
    // most ~22 per run — is the price the type asks, and the precedent
    // (transport.rs's tests, harness/defs/workbench.rs) set it first.
    let mut def = HarnessDefinition::new(
        Box::leak(format!("fitness:probe:{id}").into_boxed_str()),
        Box::leak(format!("capability probe ({id})").into_boxed_str()),
        "measure one model capability against a fixed prompt",
        ModelSpec { pin: None, role: None, chain: Some(&[]), user_id: None },
        {
            let messages = messages.clone();
            Arc::new(move |_input: &Value, _ctx: &RenderContext| Ok(messages.clone()))
        },
        output,
        OnFailure::Null,
    );
    // A PROBE NEVER REFUSES THE MODEL IT IS MEASURING. An empty floor with
    // `refuse_below: false` is the only correct declaration here — the whole
    // purpose of the run is to produce the fact a floor would consult.
    def.floor = RoleFloor::runs_anyway("A probe measures; it does not refuse.");
    // Never consulted: `run_probes` always pins `ctx.model`. An empty chain is
    // the runner's declared way of saying "the model comes from the caller" and
    // fails loudly rather than silently probing the org's utility model.
    def.on_failure = OnFailure::Null;
    // THE PROBE'S OWN PATIENCE, ON THE SOCKET. `run_probes` races every probe
    // against `PROBE_TIMEOUT_MS` and moves on; without this the abandoned HTTP
    // request kept running to the gateway's ten-minute default, holding a
    // connection nobody was waiting for. Eight candidate sweeps doing that at
    // once is how a healthy provider starts queueing and every later call blows
    // its budget too — see `UpstreamCall`'s cancellation.
    def.hold_ms = Some(PROBE_TIMEOUT_MS);
    // No rule in the registry is meaningful about a probe reply, and a probe
    // must not add rows to the guard statistics the fitness page reads as a
    // per-model confabulation rate.
    def.guard = Some(GuardDecl { rules: Some(Vec::new()), redact: false });
    def.temperature = Some(0.0);
    def.tool_defs = tools;
    def
}

/// The four injected edges every probe run shares, so `runner_ask` and
/// `runner_tool_ask` cannot drift on any of them — each one is load-bearing and
/// the header says why. The runner's other edges get inert answers: `resolve_model`
/// is never called (the context pins the model), `slot_effort` and `reach` are
/// consulted only for harnesses that declare an effort slot or suppliable
/// capabilities, and `now` is the real clock because a probe's timestamps are its
/// own to make.
fn probe_run_deps(transport: TransportFn) -> HarnessDeps {
    HarnessDeps {
        resolve_model: Arc::new(|_spec, _user| {
            Box::pin(async move { Option::<(String, crate::harness_model::ModelChainStep)>::None })
        }),
        slot_effort: Arc::new(|_slot, _model| Box::pin(async move { Option::<String>::None })),
        routing: Arc::new(|m: String| {
            Box::pin(async move { (Vec::<String>::new(), m) })
        }),
        persona_keys: Arc::new(|_m: String| Box::pin(async move { Vec::<String>::new() })),
        // See the header: a probe measures the model, not the record.
        missing_capabilities: Arc::new(|_key: String, _asked: Vec<String>| {
            Box::pin(async move { Vec::<String>::new() })
        }),
        capabilities: Arc::new(|_key: String| {
            Box::pin(async move { HashMap::<String, CapabilityFact>::new() })
        }),
        reach: Arc::new(|_keys: Vec<String>, _wanted: Vec<String>| {
            Box::pin(async move { HashMap::<String, crate::capability_reach::Reach>::new() })
        }),
        transport,
        // NO GUARD PASS ON A PROBE, declared at both ends: `guard` with no rules
        // narrows the registry to nothing, and this makes the run independent of
        // the org's guard settings entirely. A probe measures a capability; it is
        // not evidence about how the model behaves on real work, and the org's
        // confabulation statistics must not move because an admin benchmarked a
        // model. It also keeps a probe run free of a settings read per call.
        // (The TS stub adds `coach: false`; the Rust GuardConfig has no coach
        // half to turn off, so `Off` says it in one word instead.)
        guard_config: Arc::new(|| {
            Box::pin(async move {
                Some(GuardConfig {
                    mode: GuardMode::Off,
                    checks: serde_json::Map::new(),
                    min_confidence: 1.0,
                    policed_hosts: Vec::new(),
                })
            })
        }),
        guard_text: Arc::new(|_text: String, _input: Option<String>| {
            Box::pin(async move { Vec::new() })
        }),
        record_findings: Arc::new(|_findings, _meta| Box::pin(async move {})),
        record_run: Arc::new(|_row| Box::pin(async move {})),
        now: Arc::new(now_ms),
    }
}

/// THE IMAGE CHANNEL, OPENED — without widening `Message`.
///
/// BUT MEASURING A MODEL DOES NOT REQUIRE THE HARNESS TREE TO CARRY IMAGES. Both
/// branches below live in transport.rs (`gateway_image_turn`, `persona_probe_turn`)
/// because both build a raw upstream body, and raw-body construction is the
/// transport's job — `gateway_tool_turn` is the precedent right beside them.
/// Doing it here tripped `hand-written-harness` in the TS tree, and the rule was
/// right: a model call assembled outside the transport layer is exactly the
/// thing that grew six JSON extractors and three unguarded paths the last time.
///
/// WHAT THE TAG THEN MEANS, precisely: this model reads images. It does NOT mean
/// a harness can send one yet — that still needs the widening. A capability is a
/// fact about the model, and refusing to record one Talaria cannot yet spend is
/// how `vision` stayed blank on models that have had it for years.
///
/// A PERSONA HAS A SEAM AFTER ALL: `proxy_chat` forwards its payload to the
/// agent's own `/v1/chat/completions`, and that payload has always accepted
/// OpenAI content parts "passed through untouched". (The Rust
/// `persona_probe_turn` takes no caller label — attribution is the proxy's own
/// there — so the TS's `caller` argument has no counterpart to pass.)
pub fn runner_image_ask(state: &AppState, model: &str) -> ImageAskFn {
    let state = state.clone();
    let model = model.to_string();
    Arc::new(move |spec: ImageAskSpec| {
        let state = state.clone();
        let model = model.clone();
        Box::pin(async move {
            let blank = Attempt::blank();
            let caller = format!("fitness:probe:{}", spec.id);
            let text = match if offers_tool_definitions(&state, &model).await {
                gateway_image_turn(&state, &model, &spec.messages, &spec.images, &caller, Some(PROBE_TIMEOUT_MS)).await
            } else {
                persona_probe_turn(&model, &spec.messages, &spec.images, None).await.map(|turn| turn.text)
            } {
                Ok(text) => text,
                Err(err) => {
                    return Attempt { transport_error: Some(err), ..blank };
                }
            };
            Attempt { raw: text, ..blank }
        })
    })
}

pub fn runner_ask(state: &AppState, model: &str, base: TransportFn) -> AskFn {
    let state = state.clone();
    let model = model.to_string();
    Arc::new(move |spec: AskSpec| {
        let state = state.clone();
        let model = model.clone();
        let base = base.clone();
        Box::pin(async move {
            // What the wrapped transport saw, filled from inside the transport
            // closure — the runner's result says what the REPLY was, only the
            // wrapper knows what the CALL carried. A std Mutex held for one
            // statement at a time; nothing is ever held across an await.
            let seen = Arc::new(Mutex::new(SeenAsk::default()));
            let transport: TransportFn = {
                let seen = seen.clone();
                let base = base.clone();
                Arc::new(move |req: TransportRequest| {
                    let seen = seen.clone();
                    let base = base.clone();
                    Box::pin(async move {
                        if req.json_mode {
                            seen.lock().expect("the ask watcher is not contended").json_requested = true;
                        }
                        match base(req).await {
                            Ok(reply) => {
                                if reply.contract_dropped {
                                    seen.lock().expect("the ask watcher is not contended").contract_dropped = true;
                                }
                                Ok(reply)
                            }
                            // WATCHED HERE, NOT PARSED OUT OF AN ERROR STRING.
                            // `answered: false` covers both "the transport
                            // threw" and "the model returned an empty reply",
                            // and only the first of those must void the probe
                            // (rule 2).
                            Err(err) => {
                                seen.lock().expect("the ask watcher is not contended").threw = Some(err.clone());
                                Err(err)
                            }
                        }
                    })
                })
            };
            let ctx = RunContext {
                caller: format!("fitness:probe:{}", spec.id),
                model: Some(model),
                deps: Some(Arc::new(probe_run_deps(transport))),
                ..RunContext::default()
            };
            let output = spec.output();
            let def = probe_def(&spec.id, spec.messages, output, Vec::new());
            // `OnFailure::Null` means every pre-call and transport failure
            // comes back as a result rather than an error; the Err arm is the
            // belt for a failure the runner could not shape, and it reads as a
            // transport error because nothing else was observed.
            let result = match run_harness(&state, &def, &Value::Null, ctx).await {
                Ok(result) => result,
                Err(err) => {
                    return Attempt { transport_error: Some(err.0), ..Attempt::blank() };
                }
            };
            let seen = seen.lock().expect("the ask watcher is not contended").clone();
            Attempt {
                raw: result.raw.unwrap_or_default(),
                transport_error: seen.threw,
                json_requested: seen.json_requested,
                contract_dropped: seen.contract_dropped,
                contract_held: result.schema_valid,
            }
        })
    })
}

#[derive(Default, Clone)]
struct SeenAsk {
    json_requested: bool,
    contract_dropped: bool,
    threw: Option<String>,
}

/// The tool-offering call, through the same runner and the same transport rule.
///
/// There is no contract to hold here and that is deliberate: the whole
/// observation lives in `TransportReply.tool_calls`, and a model that calls the
/// right tool typically returns EMPTY content, which every text contract in the
/// tree reads as a failure. So the probe def is a plain text harness with
/// `OnFailure::Null`, the run's value is ignored, and the trial is graded by
/// `tool_call_problem` over what the transport reported.
pub fn runner_tool_ask(state: &AppState, model: &str, base: TransportFn) -> ToolAskFn {
    let state = state.clone();
    let model = model.to_string();
    Arc::new(move |spec: ToolAskSpec| {
        let state = state.clone();
        let model = model.clone();
        let base = base.clone();
        Box::pin(async move {
            let seen = Arc::new(Mutex::new(SeenToolAsk::default()));
            let transport: TransportFn = {
                let seen = seen.clone();
                let base = base.clone();
                let model = model.clone();
                Arc::new(move |req: TransportRequest| {
                    let seen = seen.clone();
                    let base = base.clone();
                    let model = model.clone();
                    Box::pin(async move {
                        match base(req).await {
                            Ok(reply) => {
                                // ABSENT IS NOT EMPTY (`TransportReply.tool_calls`).
                                // A transport that never ran the loop reports
                                // None, and reading that as "called nothing"
                                // would write `tools: false` — forever — about a
                                // model that was never offered a tool.
                                // `dispatch_transport` cannot produce it (the
                                // fleet path refuses a request carrying
                                // `tool_defs` rather than answering it), so this
                                // is the belt on a bespoke `base` handed in by a
                                // caller.
                                let Some(calls) = &reply.tool_calls else {
                                    let msg = format!(
                                        "the transport for \"{model}\" answered a tool-definition request without reporting any tool calls"
                                    );
                                    seen.lock().expect("the tool-ask watcher is not contended").threw = Some(msg.clone());
                                    return Err(msg);
                                };
                                seen.lock().expect("the tool-ask watcher is not contended").calls = calls.clone();
                                Ok(reply)
                            }
                            Err(err) => {
                                seen.lock().expect("the tool-ask watcher is not contended").threw = Some(err.clone());
                                Err(err)
                            }
                        }
                    })
                })
            };
            let ctx = RunContext {
                caller: format!("fitness:probe:{}", spec.id),
                model: Some(model),
                deps: Some(Arc::new(probe_run_deps(transport))),
                ..RunContext::default()
            };
            let def = probe_def(&spec.id, spec.messages, Output::Text { clean: None, verify: None }, spec.tools);
            let _ = run_harness(&state, &def, &Value::Null, ctx).await;
            let seen = seen.lock().expect("the tool-ask watcher is not contended").clone();
            ToolAttempt { tool_calls: seen.calls, transport_error: seen.threw }
        })
    })
}

#[derive(Default, Clone)]
struct SeenToolAsk {
    calls: Vec<ProbeToolCall>,
    threw: Option<String>,
}

// ── Default edges that read the deployment ───────────────────────────────────

/// One member of the pool this model can land on, carrying the two numbers the
/// window and the price are derived from.
struct PoolEndpoint {
    name: String,
    context_length: Option<f64>,
    price: Option<TokPrice>,
}

/// Every endpoint that could serve this model, for the window and the price.
/// Deliberately the same derivation the runner uses for capability keys.
///
/// The TS reads one shape: `routing_for`'s endpoint rows carry the price
/// columns. The Rust `LlmEndpoint` deliberately omits them, so the price half is
/// read here in one query over the routed names, coalescing exactly as the TS
/// does (`model_prices[upstream] ?? auto_prices[upstream] ?? the column`).
async fn endpoints_for(pg: &PgPool, model: &str) -> Vec<PoolEndpoint> {
    // `.catch(() => null)` then "no route, no endpoints" — a routing read that
    // fails is a deployment fact, not a probe failure, and it prices nothing.
    let Some(route) = routing_for(pg, model).await.ok().filter(|r| !r.endpoints.is_empty()) else {
        return Vec::new();
    };
    let names: Vec<String> = route.endpoints.iter().map(|ep| ep.name.clone()).collect();
    let prices: HashMap<String, (Option<f64>, Option<f64>)> =
        sqlx::query_as::<_, (String, Option<f64>, Option<f64>)>(
            "select name, \
             coalesce(model_prices->$2->>'in', auto_prices->$2->>'in', price_in_per_mtok::text)::float8 as in_tok, \
             coalesce(model_prices->$2->>'out', auto_prices->$2->>'out', price_out_per_mtok::text)::float8 as out_tok \
             from llm_endpoints where name = any($1)",
        )
        .bind(&names)
        .bind(&route.upstream_model)
        .fetch_all(pg)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(name, p_in, p_out)| (name, (p_in, p_out)))
        .collect();
    route
        .endpoints
        .iter()
        .map(|ep| PoolEndpoint {
            name: ep.name.clone(),
            context_length: ep.context_length.map(|n| n as f64),
            price: match prices.get(&ep.name) {
                Some((Some(in_tok), Some(out_tok))) => Some(TokPrice {
                    in_per_mtok: *in_tok,
                    out_per_mtok: *out_tok,
                }),
                _ => None,
            },
        })
        .collect()
}

/// THE SMALLEST advertised window in the pool, not the largest. A bare model id
/// can land on any member, so a claim has to hold for the worst of them.
///
/// THE MODEL'S OWN NUMBER FIRST, and this is the fix for a probe that used to
/// skip on models the provider describes in full. `llm_endpoints.context_length`
/// is ONE integer per endpoint — a single number for an OpenRouter row serving
/// four hundred models with windows from 4k to 1M. It is written only by
/// `fleet-federate`, and `ensure_endpoint`'s `on conflict do update` does not
/// refresh it, so on a normal install it is null and the long-context probe
/// skipped with "nothing advertises a context window for this model" about
/// models whose catalog entry says 1,048,576.
///
/// The endpoint row stays as the FALLBACK rather than being deleted: a federated
/// fleet writes it and publishes no catalog, so for those deployments it is the
/// only number there is.
async fn smallest_window(pg: &PgPool, model: &str) -> Option<f64> {
    if let Some(advertised) = advertised_window(pg, model).await {
        return Some(advertised);
    }
    let eps = endpoints_for(pg, model).await;
    let windows: Vec<f64> = eps
        .iter()
        .filter_map(|e| e.context_length.filter(|n| *n > 0.0))
        .collect();
    // Not a gateway model, and no catalog entry: a fleet persona records its
    // window on the agent's config, not on an endpoint row, and nothing here
    // can read it honestly.
    windows.iter().cloned().fold(None::<f64>, |acc, n| Some(match acc {
        Some(best) if best <= n => best,
        _ => n,
    }))
}

async fn price_for(pg: &PgPool, model: &str) -> Option<TokPrice> {
    let eps = endpoints_for(pg, model).await;
    let priced: Vec<TokPrice> = eps.into_iter().filter_map(|e| e.price).collect();
    if priced.is_empty() {
        return None;
    }
    // The DEAREST member, for the same reason as the window: an estimate that
    // could be exceeded by the endpoint the round-robin happens to pick is not
    // an estimate an admin can act on.
    Some(
        priced
            .into_iter()
            .reduce(|a, b| if a.in_per_mtok + a.out_per_mtok >= b.in_per_mtok + b.out_per_mtok { a } else { b })
            .expect("a non-empty list reduces"),
    )
}

/// A cited page, as text. Never fails, never blocks for long, and answers None
/// for anything that is not a plainly readable 2xx — every one of which makes
/// the trial inconclusive rather than failed. The URL is MODEL-SUPPLIED (it
/// rides in a probe reply), so this goes through `safe_fetch` like every other
/// agent-influenced fetch — a citation pointing at the metadata service or an
/// internal host is refused, not fetched. A refusal is indistinguishable from
/// an unreachable page: the trial reads inconclusive, which is the honest
/// verdict for a citation we would not follow in production either.
async fn fetch_cited_page(url: String) -> Option<String> {
    match safe_fetch(
        &url,
        SafeFetch {
            method: None,
            headers: Vec::new(),
            body: None,
            timeout_ms: Some(10_000),
            max_bytes: Some(400_000),
            max_redirects: None,
        },
    )
    .await
    {
        Ok(res) if (200..300).contains(&res.status) => {
            let body = String::from_utf8(res.body).ok()?;
            if body.is_empty() { None } else { Some(body) }
        }
        _ => None,
    }
}

pub fn default_deps(state: &AppState, model: &str) -> ProbeDeps {
    let pg = state.pg.clone();
    // The one transport every ask shares: what the TS called `defaultTransport`,
    // the runner's own dispatch rather than a bespoke call.
    let gateway: TransportFn = {
        let state = state.clone();
        Arc::new(move |req: TransportRequest| {
            let state = state.clone();
            Box::pin(async move { dispatch_transport(&state, &req).await })
        })
    };
    // The key derivation, built once and shared by `keys`, `advertises` and
    // `measured`, so the three cannot disagree about which model they asked.
    let keys: KeysFn = {
        let pg = pg.clone();
        let model = model.to_string();
        Arc::new(move || {
            let pg = pg.clone();
            let model = model.clone();
            Box::pin(async move { probe_keys(&pg, &model).await })
        })
    };
    ProbeDeps {
        ask: runner_ask(state, model, gateway.clone()),
        ask_with_tools: runner_tool_ask(state, model, gateway),
        // Asked of the TRANSPORT RULE rather than answered here, so this cannot
        // disagree with the transport that would refuse the call.
        offers_tool_definitions: {
            let state = state.clone();
            let model = model.to_string();
            Arc::new(move || {
                let state = state.clone();
                let model = model.clone();
                Box::pin(async move { offers_tool_definitions(&state, &model).await })
            })
        },
        // Gateway-served candidates only — a persona has no raw-body seam, the same
        // wall the tool probes hit.
        ask_with_images: Some(runner_image_ask(state, model)),
        context_window: {
            let pg = pg.clone();
            let model = model.to_string();
            Arc::new(move || {
                let pg = pg.clone();
                let model = model.clone();
                Box::pin(async move { smallest_window(&pg, &model).await })
            })
        },
        advertises: {
            let pg = pg.clone();
            let keys = keys.clone();
            Arc::new(move |cap: ProbeId| {
                let pg = pg.clone();
                let keys = keys.clone();
                Box::pin(async move {
                    // A `declared` fact IS the advertisement — capability.rs's
                    // third source is "an admin (or a model catalog) writes
                    // declared", and nothing else in Talaria records which
                    // modalities an endpoint serves. The probe then VERIFIES the
                    // advertisement, and a probe fact overrides a declared one on
                    // the same key, which is the correct direction: a measurement
                    // beats a claim. Nothing declared means nothing to verify, so
                    // the probe skips.
                    for key in keys().await {
                        let facts = get_capabilities(&pg, &key).await;
                        if let Some(fact) = facts.get(cap.as_str()) {
                            if fact.value && fact.source == "declared" {
                                return true;
                            }
                        }
                    }
                    false
                })
            })
        },
        fetch_text: Arc::new(|url: String| Box::pin(async move { fetch_cited_page(url).await })),
        record: {
            let pg = pg.clone();
            Arc::new(move |key: String, cap: String, fact: CapabilityFact| {
                let pg = pg.clone();
                Box::pin(async move {
                    // The single-fact spelling of the TS's `recordCapability`:
                    // the ranked merge in capability.rs is the one writer that
                    // respects source precedence, and a probe fact outranks
                    // everything.
                    merge_capabilities(&pg, &[(key, vec![(cap, fact)])])
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                })
            })
        },
        measured: {
            let pg = pg.clone();
            let keys = keys.clone();
            Arc::new(move |cap: ProbeId| {
                let pg = pg.clone();
                let keys = keys.clone();
                Box::pin(async move {
                    // ONE key or none. `run_probes` refuses to write facts for a
                    // pooled id, so a pooled id has none to reuse and every probe
                    // runs — which is the right answer twice over: nothing was
                    // recorded, and what one pool member can do is not what
                    // another can.
                    let keys = keys().await;
                    if keys.len() != 1 {
                        return None;
                    }
                    let facts = get_capabilities(&pg, &keys[0]).await;
                    match facts.get(cap.as_str()) {
                        Some(fact) if fact.source == "probe" => Some(fact.clone()),
                        _ => None,
                    }
                })
            })
        },
        keys,
        now: Arc::new(now_ms),
        max_context_tokens: DEFAULT_MAX_CONTEXT_TOKENS,
        needle_token: "GRANITE-FOX-7731".into(),
        price: {
            let pg = pg.clone();
            let model = model.to_string();
            Arc::new(move || {
                let pg = pg.clone();
                let model = model.clone();
                Box::pin(async move { price_for(&pg, &model).await })
            })
        },
    }
}

// ── The tool fixtures, and the one image fixture still waiting ───────────────
//
// The tool fixtures are LIVE: `TransportRequest.tool_defs` carries them to the
// model and `TransportReply.tool_calls` brings back what was called, so both tool
// probes now make real calls on any gateway-served candidate.

fn weather_tool() -> ToolSpec {
    ToolSpec {
        name: "get_weather".into(),
        description: "Look up the current weather for a city. The only way to know current weather.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "city": { "type": "string", "description": "City name" } },
            "required": ["city"]
        }),
    }
}

fn tool_trial() -> Vec<Message> {
    vec![
        terse(),
        usr("What is the weather in Lisbon right now?"),
    ]
}

/// Four tools with four clearly disjoint jobs. Disjoint on purpose: a
/// tool-selection score is only meaningful when a wrong pick is unambiguously
/// wrong, and two plausibly-overlapping tools would measure our fixture design
/// rather than the model.
fn select_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "get_weather".into(),
            description: "Current weather for a city.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "city": { "type": "string" } },
                "required": ["city"]
            }),
        },
        ToolSpec {
            name: "send_email".into(),
            description: "Send an email to a recipient.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "to": { "type": "string" }, "body": { "type": "string" } },
                "required": ["to", "body"]
            }),
        },
        ToolSpec {
            name: "convert_currency".into(),
            description: "Convert an amount from one currency to another.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "amount": { "type": "number" },
                    "from": { "type": "string" },
                    "to": { "type": "string" }
                },
                "required": ["amount", "from", "to"]
            }),
        },
        ToolSpec {
            name: "create_ticket".into(),
            description: "Open a work ticket on a board.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"]
            }),
        },
    ]
}

fn tool_select_trials() -> Vec<ExpectedFixture> {
    vec![
        ExpectedFixture {
            name: "weather",
            messages: vec![terse(), usr("Is it raining in Porto at the moment?")],
            expect: "get_weather",
        },
        ExpectedFixture {
            name: "email",
            messages: vec![terse(), usr("Let ana@example.org know the deploy is finished.")],
            expect: "send_email",
        },
        ExpectedFixture {
            name: "currency",
            messages: vec![terse(), usr("How much is 240 euros in Japanese yen?")],
            expect: "convert_currency",
        },
        ExpectedFixture {
            name: "ticket",
            messages: vec![terse(), usr("Open a task called \"rotate the staging certificate\".")],
            expect: "create_ticket",
        },
    ]
}

/// An upstream saying the MODEL takes no images, as opposed to an upstream that
/// is down. OpenRouter answers a text-only model with `404 No endpoints found
/// that support image input`; that is the deployment telling us plainly what the
/// model can be sent, not a failure to reach it.
///
/// Deliberately narrow. Anything this does not recognize stays an `Errored`,
/// which writes nothing — a wrong `vision: false` never expires.
pub fn no_image_input(err: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new(
            "no endpoints found that support image input|does not support image|image input is not supported|vision is not supported",
        )
        .case_insensitive(true)
        .build()
        .expect("a static regex compiles")
    });
    RE.is_match(err)
}

/// A 128x128 solid colour PNG, inline. A `data:` URL rather than a hosted image
/// because a probe that depended on a host being up would fail as a network
/// problem and be scored as a model that cannot see.
struct VisionFixture {
    name: &'static str,
    messages: Vec<Message>,
    image: &'static str,
    expect: &'static str,
}

/// THREE OPAQUE 128x128 SOLID FIELDS, and the previous fixture is why the size
/// and the opacity are both stated.
///
/// It was a SINGLE PIXEL at RGBA(255, 0, 0, 127) - 1x1, and half transparent.
/// Nothing can be concluded from it: a one-pixel image carries less than one
/// patch of any vision encoder, and a half-alpha red renders pink on a white
/// matte and maroon on a black one, so the "right" answer depended on whichever
/// background the provider happened to composite against. claude-haiku answered
/// BLUE and would have been recorded `vision: false` - a false negative on a
/// model that has read images for years, written into a record that does not
/// expire. That is exactly the wrong fact rule 3 of this file exists to prevent,
/// arriving from the fixture rather than from the scorer.
///
/// Three colours rather than one because a single trial cannot separate "reads
/// images" from "guessed, and there was only one thing to guess".
static VISION_TRIALS: LazyLock<Vec<VisionFixture>> = LazyLock::new(|| {
    let question = "What single color fills this image? Reply with one word: RED, GREEN or BLUE.";
    vec![
        VisionFixture {
            name: "solid red",
            messages: vec![terse(), usr(question)],
            image: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAIAAAACACAIAAABMXPacAAAAyElEQVR42u3RQREAAAjDsCmZf1GIQQY8clcFTabVYbEAAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAAcAEAAAEAIAAABACAAAAQAAACAEAAAAgAAAEAIAAABACAAAAQAAACAEAAAAgAAAEAIAAABACAAAAQAAACAEAAAAgAAAEAIAAABACAAAAQAAACAEAAAAgAAAEAIAAABACAAAAQAAAC8KEFIPUEG5PrRbsAAAAASUVORK5CYII=",
            expect: "RED",
        },
        VisionFixture {
            name: "solid green",
            messages: vec![terse(), usr(question)],
            image: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAIAAAACACAIAAABMXPacAAAAyElEQVR42u3RQQ0AAAjEsFOCEhSjEhnwaDIFa2pah8UCAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAAwAUAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIwIcW6UkD0KHeGfUAAAAASUVORK5CYII=",
            expect: "GREEN",
        },
        VisionFixture {
            name: "solid blue",
            messages: vec![terse(), usr(question)],
            image: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAIAAAACACAIAAABMXPacAAAAyElEQVR42u3RQQ0AAAjEsJODEvwHRciAR5MpWFM9OiwWAAAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAC4AACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABAAD60hHcEsZKvPusAAAAASUVORK5CYII=",
            expect: "BLUE",
        },
    ]
});

// ── The probes ───────────────────────────────────────────────────────────────

pub struct ProbeDefinition {
    pub id: ProbeId,
    pub label: &'static str,
    /// One line an admin reads: what a pass here means for their install.
    pub claim: &'static str,
    /// Calls this probe makes when it is not skipped, for the estimate.
    pub calls: i64,
    /// Prompt tokens per call, from the fixtures. `long-context` overrides it at
    /// estimate time because its prompt is sized from the model's window.
    pub prompt_tokens: i64,
    /// The most a probe reply is worth paying for.
    pub completion_tokens: i64,
    pub run: RunFn,
}

fn prompt_tokens_of(messages: &[Message]) -> i64 {
    // TS sums `.content.length` — UTF-16 units — before the chars-per-token
    // estimate, because the estimate is calibrated on what the ledger meters.
    estimate_tokens(messages.iter().map(|m| crate::body::utf16_len(&m.content)).sum())
}

/// JS `JSON.stringify` on a string, for the two trial notes that quote what the
/// model said back at it.
fn js_quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// A transport failure anywhere in a probe voids the whole probe (rule 2).
fn transport_failure(attempts: &[Attempt]) -> Option<String> {
    attempts.iter().find(|a| a.transport_error.is_some()).and_then(|a| a.transport_error.clone())
}

/// The one reason a tool probe still skips, in one place because both probes
/// say it and `estimate_probes` has to predict it. A persona is not a broken
/// deployment, so this is a skip rather than an error — and not a `false`,
/// because nothing about the model was measured.
const FLEET_TOOL_SKIP: &str = "this candidate is a fleet persona: its tool loop runs inside the agent container, so Talaria cannot offer it tool definitions or see which one it called. Probe the gateway model behind the agent instead.";

fn errored(reason: String, trials: Vec<Trial>) -> ProbeOutcome {
    ProbeOutcome::Errored { reason, trials }
}
fn skipped(reason: impl Into<String>, trials: Vec<Trial>) -> ProbeOutcome {
    ProbeOutcome::Skipped { reason: reason.into(), trials }
}
fn settle(verdict: Option<ProbeVerdict>, trials: Vec<Trial>, why: &str) -> ProbeOutcome {
    match verdict {
        Some(verdict) => ProbeOutcome::Scored { verdict, trials },
        None => skipped(why, trials),
    }
}

pub static PROBES: LazyLock<Vec<ProbeDefinition>> = LazyLock::new(|| {
    vec![
        ProbeDefinition {
            id: ProbeId::Json,
            label: "JSON mode",
            claim: "The endpoint honors response_format and the model returns a parseable object.",
            calls: JSON_TRIALS.len() as i64,
            prompt_tokens: JSON_TRIALS.iter().map(|t| prompt_tokens_of(&t.messages)).max().unwrap_or(0),
            completion_tokens: 120,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    let mut attempts: Vec<Attempt> = Vec::new();
                    let mut trials: Vec<Trial> = Vec::new();
                    for t in JSON_TRIALS.iter() {
                        let a = (deps.ask)(AskSpec {
                            id: format!("json:{}", t.name),
                            messages: t.messages.clone(),
                            schema: Some(JSON_TRIVIAL.clone()),
                        })
                        .await;
                        attempts.push(a.clone());
                        if a.transport_error.is_some() {
                            break;
                        }
                        trials.push(Trial {
                            name: t.name.into(),
                            ok: Some(a.contract_held),
                            note: if a.contract_held { "returned a valid object".into() } else { "the reply was not a valid object".into() },
                            raw: bounded(&a.raw),
                        });
                    }
                    if let Some(down) = transport_failure(&attempts) {
                        return errored(down, trials);
                    }
                    let protocol = JsonProtocol {
                        requested: attempts.iter().all(|a| a.json_requested),
                        dropped: attempts.iter().any(|a| a.contract_dropped),
                    };
                    settle(score_json(&trials, protocol), trials, "no JSON-mode call completed")
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::JsonStrict,
            label: "Schema conformance",
            claim: "Nested arrays and a long string field survive the model intact, first attempt.",
            calls: JSON_STRICT_TRIALS.len() as i64,
            prompt_tokens: JSON_STRICT_TRIALS.iter().map(|t| prompt_tokens_of(&t.messages)).max().unwrap_or(0),
            completion_tokens: 400,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    let mut attempts: Vec<Attempt> = Vec::new();
                    let mut trials: Vec<Trial> = Vec::new();
                    for t in JSON_STRICT_TRIALS.iter() {
                        let a = (deps.ask)(AskSpec {
                            id: format!("json-strict:{}", t.name),
                            messages: t.messages.clone(),
                            schema: Some(JSON_STRICT.clone()),
                        })
                        .await;
                        attempts.push(a.clone());
                        if a.transport_error.is_some() {
                            break;
                        }
                        trials.push(Trial {
                            name: t.name.into(),
                            ok: Some(a.contract_held),
                            note: if a.contract_held { "conformed".into() } else { "the object did not match the schema".into() },
                            raw: bounded(&a.raw),
                        });
                    }
                    if let Some(down) = transport_failure(&attempts) {
                        return errored(down, trials);
                    }
                    settle(score_json_strict(&trials), trials, "no schema call completed")
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::Tools,
            label: "Tool calling",
            claim: "The model emits a well-formed tool call when the answer requires one.",
            calls: 1,
            prompt_tokens: prompt_tokens_of(&tool_trial()),
            completion_tokens: 120,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    let offers = (deps.offers_tool_definitions)().await;
                    if !offers {
                        return skipped(FLEET_TOOL_SKIP, Vec::new());
                    }
                    let a = (deps.ask_with_tools)(ToolAskSpec {
                        id: "tools".into(),
                        messages: tool_trial(),
                        tools: vec![weather_tool()],
                    })
                    .await;
                    if let Some(down) = &a.transport_error {
                        return errored(down.clone(), Vec::new());
                    }
                    let call = a.tool_calls.first();
                    let problem = tool_call_problem(&a.tool_calls, "get_weather", &["city"]);
                    let trials: Vec<Trial> = vec![Trial {
                        name: "calls the offered tool".into(),
                        ok: Some(problem.is_none()),
                        note: problem.unwrap_or_else(|| {
                            format!("called {}", call.map(|c| c.name.as_str()).unwrap_or("undefined"))
                        }),
                        raw: call.and_then(|c| bounded(&format!("{}({})", c.name, c.args))),
                    }];
                    settle(score_tools(&trials), trials, "the tool call never completed")
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::ToolSelect,
            label: "Tool selection",
            claim: "Given four tools, the model picks the right one every time. This is what widens the Inbox.",
            calls: 4,
            prompt_tokens: tool_select_trials().iter().map(|t| prompt_tokens_of(&t.messages)).max().unwrap_or(0),
            completion_tokens: 120,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    let offers = (deps.offers_tool_definitions)().await;
                    if !offers {
                        return skipped(FLEET_TOOL_SKIP, Vec::new());
                    }
                    let mut trials: Vec<Trial> = Vec::new();
                    for t in tool_select_trials() {
                        let a = (deps.ask_with_tools)(ToolAskSpec {
                            id: format!("tool-select:{}", t.name),
                            messages: t.messages,
                            tools: select_tools(),
                        })
                        .await;
                        if let Some(down) = &a.transport_error {
                            return errored(down.clone(), trials);
                        }
                        let problem = tool_call_problem(&a.tool_calls, t.expect, &[]);
                        let call = a.tool_calls.first();
                        trials.push(Trial {
                            name: t.name.into(),
                            ok: Some(problem.is_none()),
                            note: problem.unwrap_or_else(|| format!("picked {}", t.expect)),
                            raw: call.and_then(|c| bounded(&format!("{}({})", c.name, c.args))),
                        });
                    }
                    settle(score_tool_select(&trials), trials, "no tool-selection call completed")
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::InstructionFollowing,
            label: "Exact instructions",
            claim: "\"Reply with exactly the word OK\" produces exactly OK. Every text harness depends on this.",
            calls: INSTRUCTION_TRIALS.len() as i64,
            prompt_tokens: INSTRUCTION_TRIALS.iter().map(|t| prompt_tokens_of(&t.messages)).max().unwrap_or(0),
            completion_tokens: 40,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    let mut attempts: Vec<Attempt> = Vec::new();
                    let mut trials: Vec<Trial> = Vec::new();
                    for t in INSTRUCTION_TRIALS.iter() {
                        let a = (deps.ask)(AskSpec { id: format!("instruction:{}", t.name), messages: t.messages.clone(), schema: None }).await;
                        attempts.push(a.clone());
                        if a.transport_error.is_some() {
                            break;
                        }
                        let trimmed = a.raw.trim();
                        let ok = trimmed == t.expect;
                        trials.push(Trial {
                            name: t.name.into(),
                            ok: Some(ok),
                            note: if ok {
                                "exact".into()
                            } else {
                                format!(
                                    "answered {} instead of {}",
                                    js_quoted(&crate::body::truncate_utf16(trimmed, 60)),
                                    js_quoted(t.expect)
                                )
                            },
                            raw: bounded(&a.raw),
                        });
                    }
                    if let Some(down) = transport_failure(&attempts) {
                        return errored(down, trials);
                    }
                    settle(score_instruction(&trials), trials, "no instruction call completed")
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::Search,
            label: "Live web search",
            claim: "The model can open a page today and quote it verbatim. Without this, research invents citations.",
            calls: SEARCH_TRIALS.len() as i64,
            prompt_tokens: SEARCH_TRIALS.iter().map(|t| prompt_tokens_of(&t.messages)).max().unwrap_or(0),
            completion_tokens: 250,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    let mut attempts: Vec<Attempt> = Vec::new();
                    let mut trials: Vec<Trial> = Vec::new();
                    for t in SEARCH_TRIALS.iter() {
                        let a = (deps.ask)(AskSpec {
                            id: format!("search:{}", t.name),
                            messages: t.messages.clone(),
                            schema: Some(SEARCH_CONTRACT.clone()),
                        })
                        .await;
                        attempts.push(a.clone());
                        if a.transport_error.is_some() {
                            break;
                        }
                        trials.extend(search_trials(t.name, &a, &deps).await);
                    }
                    if let Some(down) = transport_failure(&attempts) {
                        return errored(down, trials);
                    }
                    settle(
                        score_search(&trials),
                        trials,
                        "the cited pages could not be verified - nothing was learned either way",
                    )
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::LongContext,
            label: "Long context",
            claim: "A fact planted at 50% and 90% of the window is still there when asked for.",
            calls: 2,
            prompt_tokens: DEFAULT_MAX_CONTEXT_TOKENS,
            completion_tokens: 40,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    // A WINDOW NOBODY ADVERTISES IS NOT A REASON NOT TO LOOK.
                    //
                    // This used to skip outright, and on the Anthropic endpoint it
                    // skipped every time: Anthropic's /v1/models returns an id and
                    // a display name and nothing else, so `advertised_window` is
                    // None for every Claude model. The result was a permanent
                    // "nothing advertises a context window" on models with some
                    // of the largest windows in the industry — a gap in the
                    // capability matrix caused entirely by a provider's terse
                    // catalog.
                    //
                    // Nothing here is allowed to know Claude's window (catalogs
                    // are fetched, never hardcoded), so the honest move is to
                    // MEASURE instead of guess. Absent an advertisement the probe
                    // tests its own default ceiling, and the verdict says the
                    // window was assumed. A model that cannot hold it fails the
                    // needle, or the upstream rejects the request and that is
                    // recorded as an error — both are findings. Skipping
                    // produced neither.
                    let advertised = (deps.context_window)().await;
                    let tested = advertised
                        .map_or(deps.max_context_tokens as f64, |w| w.min(deps.max_context_tokens as f64));
                    if tested < MIN_LONG_CONTEXT_TOKENS as f64 {
                        return skipped(
                            format!(
                                "the tested window would be {} tokens, below the {} this probe considers long",
                                grouped(tested as i64),
                                grouped(MIN_LONG_CONTEXT_TOKENS)
                            ),
                            Vec::new(),
                        );
                    }
                    // 80% of the window for the prompt: a needle "at 90% depth"
                    // means 90% of the way through the text we sent, and sending
                    // a prompt that fills the window leaves the model no room to
                    // answer in.
                    let budget = (tested * 0.8).floor() as i64;
                    let mut attempts: Vec<Attempt> = Vec::new();
                    let mut trials: Vec<Trial> = Vec::new();
                    for depth in [0.5, 0.9] {
                        let text = haystack(budget, &needle_line(&deps.needle_token), depth);
                        let a = (deps.ask)(AskSpec {
                            id: format!("long-context:{depth}"),
                            messages: vec![terse(), usr(&format!("{text}\n\n{NEEDLE_QUESTION}"))],
                            schema: None,
                        })
                        .await;
                        attempts.push(a.clone());
                        if a.transport_error.is_some() {
                            break;
                        }
                        let ok = a.raw.to_lowercase().contains(&deps.needle_token.to_lowercase());
                        trials.push(Trial {
                            name: format!("needle at {}%", (depth * 100.0).round() as i64),
                            ok: Some(ok),
                            note: if ok { "found".into() } else { "the passphrase was not in the reply".into() },
                            raw: bounded(&a.raw),
                        });
                    }
                    if let Some(down) = transport_failure(&attempts) {
                        return errored(down, trials);
                    }
                    settle(
                        score_long_context(&trials, budget, advertised.is_none()),
                        trials,
                        "no long-context call completed",
                    )
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::Code,
            label: "Code",
            claim: "A small function with a precise contract passes its assertions when run.",
            calls: CODE_TASKS.len() as i64,
            prompt_tokens: CODE_TASKS
                .iter()
                .map(|t| prompt_tokens_of(&[terse(), usr(t.prompt)]))
                .max()
                .unwrap_or(0),
            completion_tokens: 400,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    let mut attempts: Vec<Attempt> = Vec::new();
                    let mut trials: Vec<Trial> = Vec::new();
                    for task in CODE_TASKS.iter() {
                        let a = (deps.ask)(AskSpec {
                            id: format!("code:{}", task.name),
                            messages: vec![terse(), usr(task.prompt)],
                            schema: None,
                        })
                        .await;
                        attempts.push(a.clone());
                        if a.transport_error.is_some() {
                            break;
                        }
                        let problem = run_code_task(task, &a.raw);
                        trials.push(Trial {
                            name: task.name.into(),
                            ok: Some(problem.is_none()),
                            note: problem.unwrap_or_else(|| "passed every assertion".into()),
                            raw: bounded(&a.raw),
                        });
                    }
                    if let Some(down) = transport_failure(&attempts) {
                        return errored(down, trials);
                    }
                    settle(score_code(&trials), trials, "no code call completed")
                })
            }),
        },
        ProbeDefinition {
            id: ProbeId::Vision,
            label: "Vision",
            claim: "The model reads an image it was given.",
            calls: VISION_TRIALS.len() as i64,
            prompt_tokens: 400,
            completion_tokens: 40,
            run: Arc::new(|deps: &ProbeDeps| {
                let deps = deps.clone();
                Box::pin(async move {
                    // THE STRUCTURAL BLOCKER IS CHECKED FIRST, and the catalog gate
                    // is gone.
                    //
                    // "This endpoint does not advertise vision" was the reason
                    // shown for every Claude model — which read as a fact about
                    // Claude and is not one; it is a fact about a catalog that
                    // publishes no modalities. Worse, it hid the REAL blocker
                    // behind it, so the one thing an admin could act on was
                    // invisible. A catalog that does advertise vision is a reason
                    // to believe the probe will pass, never a precondition for
                    // running it.
                    let Some(ask_with_images) = deps.ask_with_images.clone() else {
                        return skipped(
                            "Talaria cannot put image parts in a harness turn: Message.content is a string across the whole tree, and widening it to OpenAI content parts is a change every transport, both token estimates and the guard pass have to make together. Vision is unmeasured here - not absent. See ProbeDeps.ask_with_images.",
                            Vec::new(),
                        );
                    };
                    let mut trials: Vec<Trial> = Vec::new();
                    for t in VISION_TRIALS.iter() {
                        let a = ask_with_images(ImageAskSpec {
                            id: format!("vision:{}", t.name),
                            messages: t.messages.clone(),
                            images: vec![t.image.into()],
                        })
                        .await;
                        // A REFUSAL OF THE IMAGE ITSELF IS AN ANSWER. OpenRouter
                        // replies to a text-only model with `404 No endpoints
                        // found that support image input` — which is not a broken
                        // deployment, it is the deployment telling us plainly
                        // that this model cannot be sent an image. Rule 2 sends
                        // errors to `Errored` (writes nothing) because they are
                        // facts about the gateway; this one is a fact about the
                        // model on this endpoint, which is exactly what a
                        // capability key addresses.
                        if let Some(err) = &a.transport_error {
                            if no_image_input(err) {
                                return settle(
                                    Some(ProbeVerdict {
                                        value: false,
                                        score: 0.0,
                                        detail: "this endpoint serves no provider that accepts image input for this model".into(),
                                    }),
                                    trials,
                                    "",
                                );
                            }
                            return errored(err.clone(), trials);
                        }
                        let ok = a.raw.trim().to_uppercase().contains(t.expect);
                        trials.push(Trial {
                            name: t.name.into(),
                            ok: Some(ok),
                            note: if ok { "read correctly".into() } else { format!("answered {}", js_quoted(&crate::body::truncate_utf16(a.raw.trim(), 60))) },
                            raw: bounded(&a.raw),
                        });
                    }
                    settle(score_vision(&trials), trials, "no image call completed")
                })
            }),
        },
    ]
});

/// What is wrong with the model's tool calls, or None. Public for the scorer
/// tests, which drive it with replies recorded from real providers.
pub fn tool_call_problem(calls: &[ProbeToolCall], expected: &str, required_args: &[&str]) -> Option<String> {
    let Some(call) = calls.first() else {
        return Some("the model answered in prose instead of calling a tool".into());
    };
    if calls.len() > 1 {
        return Some(format!(
            "called {} tools when one was needed ({})",
            calls.len(),
            calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    if call.name != expected {
        return Some(format!("called {} instead of {}", call.name, expected));
    }
    if required_args.is_empty() {
        return None;
    }
    let args: Value = match serde_json::from_str(if call.args.is_empty() { "{}" } else { &call.args }) {
        Ok(args) => args,
        Err(_) => return Some(format!("called {} with arguments that are not JSON", call.name)),
    };
    // An object, not merely JSON: `Array.isArray` and a null both fail the TS
    // check, and both are "not an object" here.
    let Some(obj) = args.as_object() else {
        return Some(format!("called {} with arguments that are not an object", call.name));
    };
    let missing: Vec<&str> = required_args
        .iter()
        .copied()
        .filter(|k| !obj.contains_key(*k))
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!("called {} without {}", call.name, missing.join(", ")))
    }
}

/// The two graded observations one search reply produces: the date (checkable
/// against our own clock, and the ONLY thing allowed to write `search: false`)
/// and the citation (checkable only if the host lets us read the page).
async fn search_trials(name: &str, a: &Attempt, deps: &ProbeDeps) -> Vec<Trial> {
    let raw = bounded(&a.raw);
    let malformed = vec![Trial {
        name: format!("{name} / citation"),
        ok: Some(false),
        note: "the reply was not the requested JSON object".into(),
        raw: raw.clone(),
    }];
    if !a.contract_held {
        return malformed;
    }
    let Some(parsed) = read_search_reply(&a.raw) else {
        return malformed;
    };
    let drift = date_drift_days(&parsed.date, (deps.now)());
    let date_ok = drift.is_some_and(|d| d <= 1.0);
    let mut trials = vec![Trial {
        name: format!("{name} / {SEARCH_DATE_TRIAL}"),
        ok: Some(date_ok),
        note: if date_ok { "named today's date".into() } else { format!("said today is {}", parsed.date) },
        raw: raw.clone(),
    }];
    if let Some(citation) = citation_problem(&parsed.url) {
        trials.push(Trial { name: format!("{name} / citation"), ok: Some(false), note: citation, raw });
        return trials;
    }
    // The TS wraps this call in `.catch(() => null)`; the edge here is
    // infallible because the real `fetch_cited_page` degrades to None inside —
    // every way a fetch can fail is already the inconclusive answer.
    let Some(page) = (deps.fetch_text)(parsed.url.clone()).await else {
        // INCONCLUSIVE, NOT FAILED. A live news site answering our bare GET with
        // a 403 says nothing about the model, and writing `search: false` from
        // it would refuse research to a working search model permanently.
        trials.push(Trial {
            name: format!("{name} / citation"),
            ok: None,
            note: format!("could not read {} to check the quote", parsed.url),
            raw,
        });
        return trials;
    };
    let found = quote_appears(&parsed.quote, &page);
    trials.push(Trial {
        name: format!("{name} / citation"),
        ok: Some(found),
        note: if found {
            format!("quote verified on {}", parsed.url)
        } else {
            format!("the quoted sentence is not on {}", parsed.url)
        },
        raw,
    });
    trials
}

/// The object out of a search reply.
///
/// A second read of text the runner already parsed and validated — `Attempt` is
/// deliberately a flat record of what was OBSERVED rather than a typed value, so
/// that every scorer here can be driven from a recorded string in a test with no
/// runner anywhere. The cost is this one re-parse, on one probe.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SearchReply {
    pub date: String,
    pub url: String,
    pub quote: String,
}

pub fn read_search_reply(text: &str) -> Option<SearchReply> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<SearchReply>(&text[start..=end]).ok()
}

// ── Capability keys: where a fact is allowed to land ─────────────────────────

/// The keys this candidate's facts belong under — the same derivation the
/// runner uses, gateway routing first and a fleet persona's backing model second.
pub async fn probe_keys(pg: &PgPool, model: &str) -> Vec<String> {
    // The TS `.catch(() => null)` on the routing read: a routing failure is a
    // deployment fact and prices no keys.
    if let Some(route) = routing_for(pg, model).await.ok().filter(|r| !r.endpoints.is_empty()) {
        return route
            .endpoints
            .iter()
            .map(|ep| capability_key(&ep.name, &route.upstream_model))
            .collect();
    }
    persona_capability_keys(pg, model).await
}

// ── The estimate, as data ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeEstimateRow {
    pub id: ProbeId,
    pub calls: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// Zero calls because we ALREADY MEASURED IT, not because it cannot run. The
    /// two are both free and mean opposite things to an admin.
    pub known: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeEstimate {
    pub model: String,
    pub rows: Vec<ProbeEstimateRow>,
    pub calls: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    /// None when nothing prices this model — the UI shows tokens and call count,
    /// which are the numbers that do not depend on a catalog being reachable.
    pub usd: Option<f64>,
    /// Probes this run will REUSE rather than re-measure. Reported so the price
    /// line can say why a probes tier costs less than the last one did.
    pub known: i64,
}

/// What a run will cost BEFORE it starts. Returned as data — nothing here
/// prints, and the admin UI is what turns it into a sentence.
///
/// The TS takes `deps?: Partial<ProbeDeps>` and merges it over the defaults; the
/// Rust edge set is wholesale (see `ProbeOpts`), so a caller either supplies a
/// complete `ProbeDeps` or takes the defaults — `run_probes` passes the very deps
/// it ran with, which is the same object the TS merge produces when every
/// override comes from one place.
pub async fn estimate_probes(
    state: &AppState,
    model: &str,
    ids: Option<&[ProbeId]>,
    deps: Option<&ProbeDeps>,
    reprobe: bool,
) -> ProbeEstimate {
    let owned;
    let deps = match deps {
        Some(deps) => deps,
        None => {
            owned = default_deps(state, model);
            &owned
        }
    };
    let chosen = PROBES.iter().filter(|p| ids.is_none_or(|ids| ids.contains(&p.id)));
    let window = (deps.context_window)().await;
    // A PROBE THAT WILL SKIP COSTS NOTHING, and the estimate has to say so.
    // Charging for the six calls of three probes that cannot run made the one
    // number an admin decides on before spending money overstate a probes-only run
    // by a fifth. Read off the same deps the run itself will read, so the estimate
    // cannot claim a probe will happen that `runProbes` then skips — and, now that
    // the tool probes are armed, cannot claim they will skip when they will
    // actually make five calls.
    let offers_tools = (deps.offers_tool_definitions)().await;
    let will_skip = |id: ProbeId| {
        // THE SAME EDGE THE PROBE ASKS, not a copy of its reasoning: the tool probes
        // skip on a fleet persona and run on a gateway model, and this is billed off
        // that answer rather than off a guess about the deployment.
        match id {
            ProbeId::Tools | ProbeId::ToolSelect => !offers_tools,
            ProbeId::Vision => deps.ask_with_images.is_none(),
            // Sized from the model's own window when one is advertised, and from the
            // probe's own ceiling when none is — it runs either way now, so the only
            // skip left is a window too small to call long.
            ProbeId::LongContext => {
                window.unwrap_or(deps.max_context_tokens as f64).min(deps.max_context_tokens as f64)
                    < MIN_LONG_CONTEXT_TOKENS as f64
            }
            _ => false,
        }
    };
    let mut rows: Vec<ProbeEstimateRow> = Vec::new();
    for p in chosen {
        // ALREADY MEASURED COSTS NOTHING EITHER, and it is the commonest reason a
        // probes tier is cheap: a model tested last month re-tested this month pays
        // for the harnesses and nothing else.
        let known = !reprobe && (deps.measured)(p.id).await.is_some();
        let skip = known || will_skip(p.id);
        // The one probe whose prompt is not a fixture: it is sized from the model's
        // own window, so estimating it from the fixture would understate the run by
        // whatever the window happens to be.
        let prompt_tokens = if p.id == ProbeId::LongContext {
            (window
                .unwrap_or(deps.max_context_tokens as f64)
                .min(deps.max_context_tokens as f64)
                * 0.8)
                .floor() as i64
        } else {
            p.prompt_tokens
        };
        rows.push(ProbeEstimateRow {
            id: p.id,
            calls: if skip { 0 } else { p.calls },
            prompt_tokens,
            completion_tokens: p.completion_tokens,
            known,
        });
    }
    let price = (deps.price)().await;
    let prompt_tokens = rows.iter().map(|r| r.calls * r.prompt_tokens).sum();
    let completion_tokens = rows.iter().map(|r| r.calls * r.completion_tokens).sum();
    ProbeEstimate {
        model: model.to_string(),
        calls: rows.iter().map(|r| r.calls).sum(),
        prompt_tokens,
        completion_tokens,
        usd: price.map(|price| {
            (prompt_tokens as f64 * price.in_per_mtok + completion_tokens as f64 * price.out_per_mtok) / 1e6
        }),
        known: rows.iter().filter(|r| r.known).count() as i64,
        rows,
    }
}

// ── Latency and cost: the EXISTING ring, not a second clock ──────────────────

#[derive(Debug, Clone, Serialize)]
pub struct LatencyReading {
    pub requests: i64,
    pub errors: i64,
    /// Time-to-first-byte percentiles over those calls, ms.
    pub p50: Option<i64>,
    pub p95: Option<i64>,
    /// What the probe run itself is expected to cost, so latency and price sit on
    /// one object in the admin UI. None when nothing prices the model.
    pub usd: Option<f64>,
}

/// Read straight off `gateway_pulse()` — the 500-entry TTFB ring the gateway
/// already keeps. A probe suite that timed its own calls would report a
/// different p50 than /observability for the same model, and the one nobody
/// could reconcile would be this one.
pub fn latency_reading(usd: Option<f64>) -> LatencyReading {
    let pulse = gateway_pulse();
    LatencyReading {
        requests: pulse.requests,
        errors: pulse.errors,
        p50: pulse.p50,
        p95: pulse.p95,
        usd,
    }
}

// ── The driver ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub id: ProbeId,
    pub label: &'static str,
    pub outcome: ProbeOutcome,
}

/// ONE PROBE, AS A CONSOLE LINE. The vocabulary is the terminal's, so a probe
/// and a fixture colour the same way and a watcher does not have to learn two.
///
/// `known` is a SKIP rather than a pass: no call was made, so nothing was
/// measured on this run, and painting it green would tell a watcher the model
/// just demonstrated something it did not.
pub fn probe_line(r: &ProbeResult, ms: i64) -> EvalLogLine {
    let (verdict, note) = match &r.outcome {
        ProbeOutcome::Skipped { reason, .. } => (LogVerdict::Skip, Some(reason.clone())),
        ProbeOutcome::Known { verdict, .. } => (
            LogVerdict::Skip,
            Some(format!("already measured ({}); no call made", if verdict.value { "yes" } else { "no" })),
        ),
        ProbeOutcome::Errored { reason, .. } => (LogVerdict::Error, Some(reason.clone())),
        ProbeOutcome::Scored { verdict, .. } => {
            (if verdict.value { LogVerdict::Pass } else { LogVerdict::Fail }, Some(verdict.detail.clone()))
        }
    };
    EvalLogLine {
        harness: "probes".into(),
        case: r.label.to_string(),
        verdict,
        ms,
        tokens: 0,
        calls: 0,
        up: None,
        note: note.map(|note| crate::body::truncate_utf16(&note, 200).to_string()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub model: String,
    /// The keys the facts were written under. Empty when nothing was written.
    pub keys: Vec<String>,
    pub results: Vec<ProbeResult>,
    /// How many facts reached `record`.
    pub wrote: i64,
    pub latency: LatencyReading,
    /// Set when the model resolves to more than one endpoint:model, in which case
    /// NOTHING is written. See the comment on the check below.
    pub ambiguous: Option<Vec<String>>,
}

/// The TS's `opts` bag for both entry points. The deps are WHOLESALE rather than
/// a `Partial` merged over the defaults: a Rust struct has no spread, a
/// half-default edge set is the shape the TS tests abuse, and a caller that
/// wants the defaults with one edge changed says so by taking `default_deps` and
/// overwriting the field.
#[derive(Default)]
pub struct ProbeOpts {
    pub ids: Option<Vec<ProbeId>>,
    pub deps: Option<ProbeDeps>,
    /// Overrides the wall clock — a test drives it at milliseconds.
    pub timeout_ms: Option<u64>,
    /// Re-measures capabilities we already have a probe fact for. Off by
    /// default: a probe fact is a property of an `endpoint:model` and does not
    /// go stale on its own, so paying for it again on every sweep is spend with no
    /// new information behind it.
    pub reprobe: bool,
}

/// Run tier 1 against a pinned candidate and record what it establishes.
///
/// Returns `Err` only when a fact could not be RECORDED — the TS lets that
/// rejection travel out of `runProbes` too, and the `?` below stops at the fact
/// that failed instead of leaving the count wrong. Everything else that can go
/// wrong is already inside an outcome.
///
/// THE AMBIGUITY RULE IS THE MOST IMPORTANT LINE IN THIS FUNCTION. Capability is
/// a property of the ENDPOINT serving a model, not of the model name — a
/// quantized local build and the vendor's own API genuinely differ in what they
/// can hold. A bare model id served by a POOL lands on one member per call
/// (round-robin), so a run against it measured one endpoint; writing the result
/// under all of them would credit a vendor API's tool calling to a llama.cpp
/// build, which is a false `true` on every capability at once. When the
/// candidate resolves to more than one key the run still happens and the results
/// are still returned for a human to read — but nothing is recorded, and the
/// report names the keys so the admin can re-run against the endpoint-qualified
/// ids ("<endpoint>/<model>"), each of which resolves to exactly one.
pub async fn run_probes(state: &AppState, model: &str, opts: ProbeOpts) -> Result<ProbeReport, String> {
    let owned;
    let deps = Arc::new(match opts.deps {
        Some(deps) => deps,
        None => {
            owned = default_deps(state, model);
            owned
        }
    });
    let keys = (deps.keys)().await;
    let chosen: Vec<&ProbeDefinition> = PROBES
        .iter()
        .filter(|p| opts.ids.as_ref().is_none_or(|ids| ids.contains(&p.id)))
        .collect();

    let mut results: Vec<ProbeResult> = Vec::new();
    let mut mark = std::time::Instant::now();
    for probe in chosen {
        // ALREADY MEASURED — report the standing fact and make no call. This is the
        // single biggest saving available on a re-test: nine probes on a model
        // tested before is nine calls buying an answer we already wrote down.
        //
        // It is reported as `known` rather than omitted, because a capability
        // missing from the report reads as unmeasured, and the whole point is that
        // it is not.
        let had = if opts.reprobe { None } else { (deps.measured)(probe.id).await };
        if let Some(had) = had {
            let known = ProbeResult {
                id: probe.id,
                label: probe.label,
                outcome: ProbeOutcome::Known {
                    at: had.at,
                    trials: Vec::new(),
                    verdict: ProbeVerdict {
                        value: had.value,
                        score: had.score.unwrap_or(if had.value { 1.0 } else { 0.0 }),
                        detail: had.detail.unwrap_or_else(|| "measured by an earlier run".into()),
                    },
                },
            };
            note_live(model, probe_line(&known, 0));
            results.push(known);
            continue;
        }
        // A WALL CLOCK, because tier 2 has one and tier 1 did not. A provider that
        // accepts the connection and goes away left the run awaiting a call that
        // never settles — holding a run slot forever, unreachable by Stop (which
        // is only honored between tiers). With eight candidates able to run at
        // once that is eight slots a few hung calls can take permanently.
        //
        // A timeout is an ERROR, never a `false`: nothing about the model was
        // measured, so by rule 2 it writes nothing.
        //
        // The TS races the probe against a timer and leaves the losing call
        // running detached; `tokio::time::timeout` DROPS the future, which cancels
        // the in-flight request instead. For a probe that is the better half of
        // the race — the abandoned call was never going to be read — and it is the
        // only shape available without spawning a task per probe that nothing
        // would join. The one visible difference is that a hung call stops
        // consuming a connection slot at the moment of the timeout rather than
        // whenever the transport gives up on its own.
        //
        // (The TS also catches a probe that THROWS here and converts it to
        // `errored`. In the Rust an ask edge cannot throw — a failed call is an
        // `Attempt.transport_error` — so the catch has no equivalent to wrap; the
        // timeout is the one way out other than an honest outcome.)
        let budget = Duration::from_millis(opts.timeout_ms.unwrap_or(PROBE_TIMEOUT_MS));
        let outcome = match tokio::time::timeout(budget, (probe.run)(&deps)).await {
            Ok(outcome) => outcome,
            Err(_elapsed) => errored(format!("the probe did not finish inside {}ms", budget.as_millis()), Vec::new()),
        };
        let one = ProbeResult { id: probe.id, label: probe.label, outcome };
        note_live(model, probe_line(&one, mark.elapsed().as_millis() as i64));
        mark = std::time::Instant::now();
        results.push(one);
    }

    let estimate = estimate_probes(state, model, opts.ids.as_deref(), Some(deps.as_ref()), opts.reprobe).await;
    let latency = latency_reading(estimate.usd);

    // Nothing routes and nothing backs it: the results are still worth reading,
    // but there is no endpoint:model to file them under and inventing one would
    // pool this model's facts with whatever else lacked a key.
    if keys.len() != 1 {
        return Ok(ProbeReport {
            model: model.to_string(),
            keys,
            results,
            wrote: 0,
            latency,
            ambiguous: None,
        }
        .with_ambiguity());
    }
    let key = keys[0].clone();
    let at = epoch_to_iso((deps.now)());
    let mut wrote = 0;
    for r in &results {
        let ProbeOutcome::Scored { verdict, .. } = &r.outcome else {
            continue;
        };
        let fact = CapabilityFact {
            value: verdict.value,
            source: "probe".into(),
            at: at.clone(),
            detail: Some(verdict.detail.clone()),
            score: Some(verdict.score),
        };
        // One await at a time: the record edge is a read-modify-write of one
        // settings row and it serializes in process, but sequencing here also
        // means a failed write stops at the fact that failed instead of leaving the
        // count wrong.
        (deps.record)(key.clone(), r.id.as_str().to_string(), fact).await?;
        wrote += 1;
    }
    Ok(ProbeReport {
        model: model.to_string(),
        keys: vec![key],
        results,
        wrote,
        latency,
        ambiguous: None,
    })
}

impl ProbeReport {
    /// The TS returns `{ ...base, wrote: 0, ambiguous: keys }` when the candidate
    /// resolved to more than one endpoint:model; `keys.len() != 1` already covers
    /// both early exits, and this puts the ambiguity back where it belongs.
    fn with_ambiguity(self) -> ProbeReport {
        let ambiguous = (self.keys.len() > 1).then(|| self.keys.clone());
        ProbeReport { ambiguous, ..self }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// Port of probes.test.ts. The `runCodeTask` and `extractCode` blocks crossed
// ahead of this file and live in code_runner.rs's own tests; everything else is
// here, in the TS file's order.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ALL_CAPABILITIES;
    use crate::harness::transport::{TransportKind, TransportReply};

    // Every scorer here is driven from a RECORDED REPLY, and that is the whole
    // design of the file rather than a testing convenience. These probes are the
    // first production writer of `value: true` in a capability record, and a wrong
    // fact does not expire — so the thing that has to be held still is what a
    // specific bad reply from a specific weak model scores, not whether some model
    // somewhere passes. The gateway, the database and the clock are all injected.

    // ── Helpers ──────────────────────────────────────────────────────────────────

    fn trial(name: &str, ok: Option<bool>, note: &str) -> Trial {
        Trial { name: name.into(), ok, note: note.into(), raw: None }
    }
    fn pass(name: &str) -> Trial {
        trial(name, Some(true), "")
    }
    fn fail(name: &str, note: &str) -> Trial {
        trial(name, Some(false), note)
    }
    fn unknown(name: &str) -> Trial {
        trial(name, None, "inconclusive")
    }

    /// The TS `attempt({...})` spread: the same defaults, mutations after.
    fn attempt() -> Attempt {
        Attempt {
            raw: String::new(),
            transport_error: None,
            json_requested: true,
            contract_dropped: false,
            contract_held: true,
        }
    }

    #[derive(Clone)]
    struct Written {
        key: String,
        cap: String,
        fact: CapabilityFact,
    }

    /// What the TS scripts through `vi.mock` on the routing and persona modules:
    /// hoisted mutable state every test resets. The Rust `keys` edge reads it.
    struct TestRouting {
        endpoints: Vec<String>,
        upstream_model: String,
        persona_keys: Vec<String>,
    }
    static ROUTING: LazyLock<Mutex<TestRouting>> = LazyLock::new(|| {
        Mutex::new(TestRouting {
            endpoints: vec!["pl-main".into()],
            upstream_model: "qwen3-14b".into(),
            persona_keys: Vec::new(),
        })
    });

    fn set_routing(endpoints: &[&str], upstream_model: &str, persona_keys: &[&str]) {
        let mut routing = ROUTING.lock().expect("the test routing is not contended");
        routing.endpoints = endpoints.iter().map(|e| e.to_string()).collect();
        routing.upstream_model = upstream_model.to_string();
        routing.persona_keys = persona_keys.iter().map(|k| k.to_string()).collect();
    }

    /// The scripted `probeKeys`: endpoint keys when a route exists, the persona's
    /// backing-model keys otherwise.
    fn routing_keys() -> Vec<String> {
        let routing = ROUTING.lock().expect("the test routing is not contended");
        if !routing.endpoints.is_empty() {
            routing
                .endpoints
                .iter()
                .map(|ep| format!("{ep}:{}", routing.upstream_model))
                .collect()
        } else {
            routing.persona_keys.clone()
        }
    }

    /// The `now` every scripted run reads, so `fact.at` is assertable.
    static FIXED_NOW: LazyLock<i64> = LazyLock::new(|| {
        crate::gateway::params::iso_to_epoch_ms("2026-08-06T09:00:00.000Z")
            .expect("the fixture date parses")
    });

    /// Real but lazy state, built PER TEST — the pool dials nothing and no
    /// scripted test reads a table (same posture as the work-session and blurb
    /// tests), but sqlx wants a live Tokio context even to construct a lazy
    /// pool, so the state is made inside each test's own runtime rather than
    /// once in a static. Nothing here ever dials out; the pool's housekeeping
    /// tasks die with the test's runtime and are never noticed.
    fn test_state() -> AppState {
        let url = "postgres://probes-test@localhost:5432/probes-test";
        let cfg = crate::config::Config::from_parts(
            url.into(),
            "redis://probes-test@localhost:6379".into(),
            "test-root".into(),
            String::new(),
            String::new(),
            "0".into(),
        )
        .expect("the test config is valid on its face");
        let cfg = Arc::new(cfg);
        AppState::new(crate::db::pool(&cfg), cfg)
    }

    /// A probe run with every edge injected: no gateway, no database, no network,
    /// no clock. `ask` is keyed on the id prefix each probe passes. A test that
    /// needs a different edge overwrites the field on the returned deps — the
    /// wholesale equivalent of the TS harness's `over:` partial.
    /// ONE ROUTING AT A TIME. The routing table is process-global and every
    /// harness() installs the default on the way in; without the turnstile a
    /// neighbouring test's install lands between this test's override and its
    /// run, and the run reads a routing nobody asserted against. The guard is
    /// returned so it outlives the call — hold it until the asserts are done.
    static ROUTE_TURNSTILE: Mutex<()> = Mutex::new(());

    fn harness(
        reply: Option<Arc<dyn Fn(&AskSpec) -> Attempt + Send + Sync>>,
        tools: Option<Arc<dyn Fn(&ToolAskSpec) -> ToolAttempt + Send + Sync>>,
    ) -> (
        ProbeDeps,
        Arc<Mutex<Vec<Written>>>,
        Arc<Mutex<Vec<String>>>,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let route = ROUTE_TURNSTILE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // The TS `beforeEach` default routing; a test needing another shape sets it
        // after this returns, exactly where the TS test sets `state.endpoints`.
        set_routing(&["pl-main"], "qwen3-14b", &[]);
        let written = Arc::new(Mutex::new(Vec::new()));
        let asked = Arc::new(Mutex::new(Vec::new()));

        let ask: AskFn = {
            let asked = asked.clone();
            let reply = reply.clone();
            Arc::new(move |spec: AskSpec| {
                asked.lock().expect("the ask recorder is not contended").push(spec.id.clone());
                let reply = reply.clone();
                Box::pin(async move { match &reply { Some(f) => f(&spec), None => attempt() } })
            })
        };
        let ask_with_tools: ToolAskFn = {
            let asked = asked.clone();
            let tools = tools.clone();
            Arc::new(move |spec: ToolAskSpec| {
                asked.lock().expect("the ask recorder is not contended").push(spec.id.clone());
                let tools = tools.clone();
                Box::pin(async move { match &tools {
                    Some(f) => f(&spec),
                    // Default: nothing, which is a failed trial rather than a missing one.
                    None => ToolAttempt { tool_calls: Vec::new(), transport_error: None },
                } })
            })
        };
        let record: RecordFn = {
            let written = written.clone();
            Arc::new(move |key: String, cap: String, fact: CapabilityFact| {
                let written = written.clone();
                Box::pin(async move {
                    written.lock().expect("the write recorder is not contended").push(Written { key, cap, fact });
                    Ok(())
                })
            })
        };

        let deps = ProbeDeps {
            ask,
            ask_with_tools,
            // The tool channel is open on a gateway model, which is the ordinary
            // case. The tests that care about a fleet candidate override this.
            offers_tool_definitions: Arc::new(|| Box::pin(async { true })),
            ask_with_images: None,
            context_window: Arc::new(|| Box::pin(async { None::<f64> })),
            advertises: Arc::new(|_id: ProbeId| Box::pin(async { false })),
            fetch_text: Arc::new(|_url: String| Box::pin(async { None::<String> })),
            record,
            // NOTHING MEASURED YET is the default, so every existing test keeps asking
            // the model. The reuse path is opted into by the tests that are about it.
            measured: Arc::new(|_id: ProbeId| Box::pin(async { None::<CapabilityFact> })),
            keys: Arc::new(|| Box::pin(async { routing_keys() })),
            now: Arc::new(|| *FIXED_NOW),
            max_context_tokens: DEFAULT_MAX_CONTEXT_TOKENS,
            needle_token: "GRANITE-FOX-7731".into(),
            price: Arc::new(|| Box::pin(async { None::<TokPrice> })),
        };
        (deps, written, asked, route)
    }

    fn outcome_of(report: &ProbeReport, id: ProbeId) -> &ProbeOutcome {
        report
            .results
            .iter()
            .find(|r| r.id == id)
            .map(|r| &r.outcome)
            .unwrap_or_else(|| panic!("no result for probe {}", id.as_str()))
    }

    /// A run over the given ids with the given scripted deps.
    fn opts(ids: &[ProbeId], deps: ProbeDeps) -> ProbeOpts {
        ProbeOpts { ids: (!ids.is_empty()).then(|| ids.to_vec()), deps: Some(deps), ..ProbeOpts::default() }
    }

    /// The right answer for each of the four `tool-select` prompts, keyed on the
    /// trial id the probe passes. Written as data so a wrong pick is one edit.
    fn correct(spec_id: &str) -> &'static str {
        match spec_id {
            "tool-select:weather" => "get_weather",
            "tool-select:email" => "send_email",
            "tool-select:currency" => "convert_currency",
            "tool-select:ticket" => "create_ticket",
            _ => "?",
        }
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }

    // ── The registry itself ──────────────────────────────────────────────────────

    #[test]
    fn names_every_capability_exactly_once_a_capability_with_no_probe_is_unmeasurable() {
        // ASSERTED AGAINST THE UNION, not against a copy of it. A tenth member of
        // `Capability` with no probe is a fact nothing can ever establish: the
        // gateway only writes those it learns from a 400, so an unprobed capability
        // can only ever be unknown or false, and any harness that `requires` it can
        // never reach Ready. That has to fail here rather than in six months.
        let mut probe_ids: Vec<&str> = PROBES.iter().map(|p| p.id.as_str()).collect();
        let mut caps: Vec<&str> = ALL_CAPABILITIES.to_vec();
        probe_ids.sort_unstable();
        caps.sort_unstable();
        assert_eq!(probe_ids, caps);
    }

    #[test]
    fn declares_a_call_count_and_a_token_size_for_every_probe_because_the_estimate_is_shown_before_spending() {
        for p in PROBES.iter() {
            assert!(p.calls > 0, "{}", p.id.as_str());
            assert!(p.prompt_tokens > 0, "{}", p.id.as_str());
            assert!(p.completion_tokens > 0, "{}", p.id.as_str());
            assert!(p.claim.len() > 10, "{}", p.id.as_str());
        }
    }

    // ── rateOf: the inconclusive rule ────────────────────────────────────────────

    #[test]
    fn rateof_ignores_inconclusive_trials_rather_than_counting_them_as_failures() {
        close(rate_of(&[pass("trial"), fail("trial", "failed"), unknown("trial")]).expect("scored"), 0.5);
    }

    #[test]
    fn rateof_answers_none_when_nothing_was_conclusive_which_is_what_suppresses_the_write() {
        assert_eq!(rate_of(&[unknown("trial"), unknown("trial")]), None);
        assert_eq!(rate_of(&[]), None);
    }

    // ── json ─────────────────────────────────────────────────────────────────────

    #[test]
    fn score_json_records_true_when_every_json_mode_call_returned_a_usable_object() {
        let v = score_json(
            &[pass("a"), pass("b"), pass("c")],
            JsonProtocol { requested: true, dropped: false },
        )
        .expect("scored");
        assert!(v.value);
        close(v.score, 1.0);
        assert!(v.detail.contains("response_format"), "got: {}", v.detail);
    }

    #[test]
    fn score_json_records_the_model_as_capable_on_a_contract_drop_when_every_reply_parsed() {
        // The gateway learned an upstream 400 on `response_format`, pre-stripped it,
        // the call succeeded, and every reply was still JSON because the prompt
        // anchor asked for it.
        //
        // THIS USED TO RECORD `false`, and that became load-bearing the moment a
        // JSON harness put `json` in its floor: a self-hosted server with no
        // response_format support would have had all nine structured harnesses
        // declared unfit for models that produce perfect JSON. The endpoint's gap is
        // tracked where it belongs — `contract_dropped` and the learned-param ratchet
        // — and this fact is about the MODEL.
        //
        // Answering from the prompt alone is the HARDER question, so passing it
        // three for three is a stronger result than honoring the parameter, not a
        // weaker one. The detail still names the drop so nobody reads the verdict as
        // "this endpoint constrains decoding".
        let v = score_json(
            &[pass("a"), pass("b"), pass("c")],
            JsonProtocol { requested: true, dropped: true },
        )
        .expect("scored");
        assert!(v.value);
        close(v.score, 1.0);
        assert!(v.detail.contains("dropped response_format"), "got: {}", v.detail);
        assert!(v.detail.contains("from the prompt alone"), "got: {}", v.detail);
    }

    #[test]
    fn score_json_records_false_with_the_observed_rate_when_replies_did_not_parse() {
        let v = score_json(
            &[pass("a"), fail("b", "trailing prose after the object"), fail("c", "two objects")],
            JsonProtocol { requested: true, dropped: false },
        )
        .expect("scored");
        assert!(!v.value);
        close(v.score, 1.0 / 3.0);
        assert!(v.detail.contains("trailing prose after the object"), "got: {}", v.detail);
    }

    #[test]
    fn score_json_writes_nothing_when_json_mode_was_never_requested_we_cannot_record_what_we_did_not_test() {
        assert_eq!(score_json(&[pass("trial"), pass("trial"), pass("trial")], JsonProtocol { requested: false, dropped: false }), None);
    }

    #[test]
    fn score_json_strict_accepts_4_of_5_because_the_runner_has_a_repair_turn_behind_it() {
        let v = score_json_strict(&[pass("1"), pass("2"), pass("3"), pass("4"), fail("5", "summary was 90 characters")])
            .expect("scored");
        assert!(v.value);
        close(v.score, 0.8);
    }

    #[test]
    fn score_json_strict_rejects_3_of_5_and_names_the_first_failure() {
        let v = score_json_strict(&[
            pass("1"),
            pass("2"),
            pass("3"),
            fail("4", "unescaped newline in summary"),
            fail("5", "items was a string"),
        ])
        .expect("scored");
        assert!(!v.value);
        assert!(v.detail.contains("unescaped newline in summary"), "got: {}", v.detail);
    }

    // ── tools / tool-select ──────────────────────────────────────────────────────

    fn call(name: &str, args: &str) -> ProbeToolCall {
        ProbeToolCall { name: name.into(), id: None, args: args.into() }
    }

    #[test]
    fn tool_call_problem_accepts_a_single_correct_call_with_the_required_argument() {
        assert_eq!(tool_call_problem(&[call("get_weather", "{\"city\":\"Lisbon\"}")], "get_weather", &["city"]), None);
    }

    #[test]
    fn tool_call_problem_rejects_a_prose_answer() {
        let problem = tool_call_problem(&[], "get_weather", &["city"]).expect("rejected");
        assert!(problem.contains("prose"), "got: {problem}");
    }

    #[test]
    fn tool_call_problem_rejects_the_wrong_tool() {
        assert_eq!(
            tool_call_problem(&[call("send_email", "{}")], "get_weather", &[]),
            Some("called send_email instead of get_weather".into())
        );
    }

    #[test]
    fn tool_call_problem_rejects_a_shotgun_that_calls_everything() {
        let problem =
            tool_call_problem(&[call("get_weather", "{}"), call("send_email", "{}")], "get_weather", &[])
                .expect("rejected");
        assert!(problem.contains("called 2 tools"), "got: {problem}");
    }

    #[test]
    fn tool_call_problem_rejects_arguments_that_are_not_json_and_arguments_that_are_missing() {
        let not_json = tool_call_problem(&[call("get_weather", "city=Lisbon")], "get_weather", &["city"]).expect("rejected");
        assert!(not_json.contains("not JSON"), "got: {not_json}");
        let missing = tool_call_problem(&[call("get_weather", "{\"town\":\"Lisbon\"}")], "get_weather", &["city"]).expect("rejected");
        assert!(missing.contains("without city"), "got: {missing}");
    }

    #[test]
    fn score_tool_select_needs_all_four_a_model_that_picks_right_3_of_4_has_not_earned_the_inbox_widening() {
        let v = score_tool_select(&[
            pass("weather"),
            pass("email"),
            pass("currency"),
            fail("ticket", "called send_email instead of create_ticket"),
        ])
        .expect("scored");
        assert!(!v.value);
        close(v.score, 0.75);
        assert!(v.detail.contains("widening needs all of them"), "got: {}", v.detail);
    }

    #[test]
    fn score_tool_select_records_true_only_at_4_of_4() {
        let v = score_tool_select(&[pass("a"), pass("b"), pass("c"), pass("d")]).expect("scored");
        assert!(v.value);
        close(v.score, 1.0);
    }

    #[test]
    fn score_tools_is_pass_fail_on_the_single_offered_tool() {
        assert!(score_tools(&[pass("trial")]).expect("scored").value);
        assert!(!score_tools(&[fail("t", "answered in prose")]).expect("scored").value);
    }

    // ── instruction-following ────────────────────────────────────────────────────

    #[test]
    fn score_instruction_records_true_only_when_every_exact_output_instruction_came_back_verbatim() {
        let v = score_instruction(&[pass("trial"), pass("trial"), pass("trial")]).expect("scored");
        assert!(v.value);
        close(v.score, 1.0);
    }

    #[test]
    fn score_instruction_records_false_at_2_of_3_and_quotes_what_came_back_instead() {
        let v = score_instruction(&[
            pass("exactly OK"),
            fail("exactly three words", "answered \"Sure! red green blue\""),
            pass("exactly one digit"),
        ])
        .expect("scored");
        assert!(!v.value);
        assert!(v.detail.contains("Sure! red green blue"), "got: {}", v.detail);
    }

    // ── search ───────────────────────────────────────────────────────────────────

    #[test]
    fn citation_problem_accepts_a_real_absolute_url() {
        assert_eq!(citation_problem("https://news.ycombinator.com/item?id=1"), None);
    }

    #[test]
    fn citation_problem_rejects_a_relative_link_a_bare_host_and_the_placeholder_hosts_a_model_reaches_for() {
        assert!(citation_problem("/docs/index.html").as_deref().unwrap_or("").contains("not an absolute URL"));
        assert!(citation_problem("https://intranet/page").as_deref().unwrap_or("").contains("no real host"));
        assert!(citation_problem("https://example.com/story").as_deref().unwrap_or("").contains("placeholder host"));
    }

    #[test]
    fn date_drift_days_measures_the_gap_in_whole_days() {
        assert_eq!(date_drift_days("2026-08-06", *FIXED_NOW), Some(0.0));
        assert_eq!(date_drift_days("2026-08-05", *FIXED_NOW), Some(1.0));
        assert!(date_drift_days("2025-04-01", *FIXED_NOW).expect("parsed") > 400.0);
    }

    #[test]
    fn date_drift_days_answers_none_for_anything_that_is_not_a_plain_iso_date() {
        assert_eq!(date_drift_days("August 6th", *FIXED_NOW), None);
        assert_eq!(date_drift_days("2026-13-45", *FIXED_NOW), None);
    }

    #[test]
    fn quote_appears_matches_through_tags_and_line_wrapping_which_is_the_only_difference_between_markup_and_rendered_text() {
        let page = "<article>\n  <p>The council <b>approved</b> the new ferry timetable on Tuesday evening.</p>\n</article>";
        assert!(quote_appears("The council approved the new ferry timetable on Tuesday evening.", page));
    }

    #[test]
    fn quote_appears_rejects_a_sentence_that_is_not_on_the_page() {
        let page = "<article>\n  <p>The council <b>approved</b> the new ferry timetable on Tuesday evening.</p>\n</article>";
        assert!(!quote_appears("The council rejected the new ferry timetable on Tuesday evening.", page));
    }

    #[test]
    fn quote_appears_rejects_a_quote_too_short_to_be_evidence_of_anything() {
        let page = "<article>\n  <p>The council <b>approved</b> the new ferry timetable on Tuesday evening.</p>\n</article>";
        assert!(!quote_appears("approved", page));
    }

    #[test]
    fn score_search_records_true_off_one_attempt_that_both_named_today_and_quoted_a_page_we_fetched() {
        let trials = [
            pass("a / date"),
            pass("a / citation"),
            fail("b / date", "said today is 2024-06-01"),
            unknown("b / citation"),
        ];
        assert!(score_search(&trials).expect("scored").value);
    }

    #[test]
    fn score_search_writes_nothing_for_a_verified_quote_the_model_could_not_date_that_is_a_good_memory_not_a_search() {
        // deepseek-v4-pro's real shape, and it used to earn a permanent `search:
        // true` on an endpoint that returns no citations at all. Research then ran
        // its search stages natively on a model that never searched.
        assert_eq!(score_search(&[fail("a / date", "said today is 2024-06-01"), pass("a / citation")]), None);
    }

    #[test]
    fn score_search_will_not_pair_a_passing_quote_with_a_passing_date_from_a_different_reply() {
        // Two half-successes are not one success: the attempt that dated correctly
        // cited nothing checkable, and the attempt that quoted a real page did not
        // know what day it was.
        let trials = [
            pass("a / date"),
            unknown("a / citation"),
            fail("b / date", "said today is 2024-06-01"),
            pass("b / citation"),
        ];
        assert_eq!(score_search(&trials), None);
    }

    #[test]
    fn score_search_records_false_only_when_the_model_could_not_name_today_which_needs_no_network_of_ours() {
        let v = score_search(&[
            fail("a / date", "said today is 2024-06-01"),
            fail("a / citation", "the citation is a placeholder host: example.com"),
            fail("b / date", "said today is 2024-06-01"),
            fail("b / citation", "the citation is a placeholder host: example.com"),
        ])
        .expect("scored");
        assert!(!v.value);
        assert!(v.detail.contains("2024-06-01"), "got: {}", v.detail);
    }

    #[test]
    fn score_search_writes_nothing_when_only_the_quote_check_failed_a_403_on_a_cited_page_must_never_refuse_a_working_search_model_forever() {
        assert_eq!(
            score_search(&[
                pass("a / date"),
                fail("a / citation", "the quoted sentence is not on https://news.example.org/x"),
                pass("b / date"),
                unknown("b / citation"),
            ]),
            None
        );
    }

    #[test]
    fn read_search_reply_recovers_the_object_out_of_a_fenced_chatty_reply() {
        let raw = "Sure!\n```json\n{\"date\":\"2026-08-06\",\"url\":\"https://a.example/x\",\"quote\":\"a sentence\"}\n```\nHope that helps.";
        assert_eq!(
            read_search_reply(raw),
            Some(SearchReply {
                date: "2026-08-06".into(),
                url: "https://a.example/x".into(),
                quote: "a sentence".into()
            })
        );
    }

    #[test]
    fn read_search_reply_answers_none_on_prose_and_on_a_wrong_shape() {
        assert_eq!(read_search_reply("I could not find anything."), None);
        assert_eq!(read_search_reply("{\"date\":\"2026-08-06\"}"), None);
    }

    // ── long-context ─────────────────────────────────────────────────────────────

    #[test]
    fn haystack_plants_the_needle_at_the_requested_depth_of_the_filler() {
        let needle = needle_line("GRANITE-FOX-7731");
        let text = haystack(1_100, &needle, 0.9);
        let lines: Vec<&str> = text.split('\n').collect();
        let at = lines.iter().position(|l| *l == needle).expect("the needle is planted");
        assert!(at > 0);
        assert!(at as f64 / lines.len() as f64 > 0.85);
    }

    #[test]
    fn haystack_sizes_the_filler_from_the_token_budget_it_was_given() {
        // TS `.length` is UTF-16 units, and the budget the probe passes is tokens —
        // the comparison is on the same measure the estimate bills.
        let short = crate::body::utf16_len(&haystack(1_100, "needle", 0.5));
        let long = crate::body::utf16_len(&haystack(11_000, "needle", 0.5));
        assert!(long > short * 8);
    }

    #[test]
    fn score_long_context_needs_both_depths_and_says_which_window_was_actually_tested() {
        let both = score_long_context(&[pass("needle at 50%"), pass("needle at 90%")], 25_600, false).expect("scored");
        assert!(both.value);
        assert!(both.detail.contains("25,600"), "got: {}", both.detail);
        let half =
            score_long_context(&[pass("needle at 50%"), fail("needle at 90%", "the passphrase was not in the reply")], 25_600, false)
                .expect("scored");
        assert!(!half.value);
        close(half.score, 0.5);
    }

    // ── code ─────────────────────────────────────────────────────────────────────

    const GOOD_SLUGIFY: &str = "function slugify(input) {
  return String(input).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}";

    #[test]
    fn score_code_is_the_fraction_of_tasks_whose_function_passed_every_assertion() {
        let v = score_code(&[pass("slugify"), pass("mergeRanges")]).expect("scored");
        assert!(v.value);
        close(v.score, 1.0);
        let half = score_code(&[pass("slugify"), fail("mergeRanges", "mergeRanges([[1,2],[2,3]]) returned [[1,2],[2,3]]")])
            .expect("scored");
        assert!(!half.value);
        assert!(half.detail.contains("mergeRanges"), "got: {}", half.detail);
    }

    // not ported: needs a live pool — the three `defaultDeps` tests that read
    // endpoint rows (`reads the SMALLEST advertised window in the pool`, `reads
    // the DEAREST price in the pool`, `answers null for both when nothing is
    // known`) drive `routing_for` against postgres. The edge functions they
    // exercise (`endpoints_for`, `smallest_window`, `price_for`) read real tables
    // and have no scripted seam short of a live pool.

    // ── The driver ───────────────────────────────────────────────────────────────

    const GOOD: &str = "{\"name\":\"talaria\",\"count\":3,\"ok\":true}";

    /// `reply: () => ({ raw: GOOD })` — the fixture half the driver tests use.
    fn good_reply() -> Option<Arc<dyn Fn(&AskSpec) -> Attempt + Send + Sync>> {
        Some(Arc::new(|_spec: &AskSpec| {
            let mut a = attempt();
            a.raw = GOOD.into();
            a
        }))
    }

    #[tokio::test]
    async fn writes_one_probe_fact_per_scored_probe_keyed_endpoint_model() {
        let (deps, written, _asked, _route) = harness(good_reply(), None);
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");
        assert_eq!(report.keys, vec!["pl-main:qwen3-14b"]);
        assert_eq!(report.wrote, 1);
        let w = written.lock().expect("the write recorder is not contended");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].key, "pl-main:qwen3-14b");
        assert_eq!(w[0].cap, "json");
        assert!(w[0].fact.value);
        assert_eq!(w[0].fact.source, "probe");
        assert_eq!(w[0].fact.score, Some(1.0));
        assert_eq!(w[0].fact.at, "2026-08-06T09:00:00.000Z");
        assert!(w[0].fact.detail.as_deref().map(|d| crate::body::utf16_len(d) > 10).unwrap_or(false));
    }

    #[tokio::test]
    async fn reuses_a_capability_an_earlier_run_already_probed_instead_of_buying_it_again() {
        // The saving that matters on a re-test: nine probes on a model tested last
        // month is nine calls for an answer we already wrote down. A probe fact is a
        // property of an `endpoint:model` and does not go stale on its own.
        let (mut deps, written, asked, _route) = harness(good_reply(), None);
        deps.measured = Arc::new(|id: ProbeId| {
            Box::pin(async move {
                (id == ProbeId::Json).then(|| CapabilityFact {
                    value: true,
                    source: "probe".into(),
                    at: "2026-07-01T00:00:00.000Z".into(),
                    detail: Some("measured before".into()),
                    score: Some(1.0),
                })
            })
        });
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");

        assert!(asked.lock().expect("the ask recorder is not contended").is_empty());
        // Nothing is rewritten: the fact already stands, and restamping `at` would
        // make it look freshly measured on every sweep.
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
        assert_eq!(report.wrote, 0);
        // NOT `skipped`. A skip means no fact exists and an admin should conclude
        // nothing; this means the fact exists and still stands.
        match outcome_of(&report, ProbeId::Json) {
            ProbeOutcome::Known { at, verdict, .. } => {
                assert_eq!(at, "2026-07-01T00:00:00.000Z");
                assert!(verdict.value);
            }
            other => panic!("expected known, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn re_measures_when_asked_to_so_a_re_pointed_model_id_can_be_re_established() {
        let (mut deps, written, asked, _route) = harness(good_reply(), None);
        deps.measured = Arc::new(|_id: ProbeId| {
            Box::pin(async {
                Some(CapabilityFact {
                    value: false,
                    source: "probe".into(),
                    at: "2026-07-01T00:00:00.000Z".into(),
                    detail: Some("old".into()),
                    score: Some(0.0),
                })
            })
        });
        let mut run = opts(&[ProbeId::Json], deps);
        run.reprobe = true;
        let report = run_probes(&test_state(), "qwen3-14b", run).await.expect("the run completes");

        assert!(!asked.lock().expect("the ask recorder is not contended").is_empty());
        assert_eq!(report.wrote, 1);
        let w = written.lock().expect("the write recorder is not contended");
        assert!(w[0].fact.value);
        assert_eq!(w[0].fact.source, "probe");
    }

    #[tokio::test]
    async fn never_reuses_a_declared_or_learned_fact_those_are_the_claims_tier_1_exists_to_verify() {
        // `measured` is documented to return only a `probe` fact, and the default
        // implementation enforces it. This is the assertion that a catalog's
        // marketing copy can never stop us checking the model.
        let (mut deps, asked, _written, _route) = harness(good_reply(), None);
        // The real `measured` filters on source; a dep that returned a declared
        // fact would be a bug in `default_deps`, so what is asserted here is that
        // the None it returns for one puts the probe back on the wire.
        deps.measured = Arc::new(|_id: ProbeId| Box::pin(async { None::<CapabilityFact> }));
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");
        assert!(!asked.lock().expect("the ask recorder is not contended").is_empty());
    }

    #[tokio::test]
    async fn writes_nothing_when_a_probe_errors_an_absent_fact_means_unknown_and_unknown_is_safe() {
        // A 401 or a restarting gateway is a fact about the deployment, not about
        // the model. A `json: false` written from one would refuse a working model
        // for good, because probe facts do not expire.
        let (deps, written, _asked, _route) = harness(
            Some(Arc::new(|_spec: &AskSpec| {
                let mut a = attempt();
                a.transport_error = Some("gateway completion 401: bad key".into());
                a
            })),
            None,
        );
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Json, ProbeId::InstructionFollowing], deps))
            .await
            .expect("the run completes");
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
        assert_eq!(report.wrote, 0);
        match outcome_of(&report, ProbeId::Json) {
            ProbeOutcome::Errored { reason, .. } => assert_eq!(reason, "gateway completion 401: bad key"),
            other => panic!("expected errored, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stops_calling_after_a_transport_failure_instead_of_burning_the_rest_of_the_trials() {
        let (deps, _written, asked, _route) = harness(
            Some(Arc::new(|_spec: &AskSpec| {
                let mut a = attempt();
                a.transport_error = Some("connect ECONNREFUSED".into());
                a
            })),
            None,
        );
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::InstructionFollowing], deps))
            .await
            .expect("the run completes");
        assert_eq!(asked.lock().expect("the ask recorder is not contended").len(), 1);
    }

    // not ported: `contains a probe that throws, and still scores the ones around
    // it`. The TS ask edge can REJECT, and runProbes converts the rejection into
    // an `errored` outcome; a Rust ask edge cannot — a failed call is an
    // `Attempt.transport_error`, which the `writes_nothing_when_a_probe_errors`
    // test above covers — and a panic inside a probe would take the process down
    // rather than surface as an outcome. There is no throw to contain, so there is
    // no test for containing one.

    #[tokio::test]
    async fn refuses_to_write_when_the_model_resolves_to_more_than_one_endpoint() {
        // Capability is a property of the ENDPOINT. A bare id served by a pool lands
        // on one member per call, so crediting the result to all of them would give
        // a llama.cpp build the vendor API's tool calling.
        let (deps, written, _asked, _route) = harness(good_reply(), None);
        // AFTER the harness: harness installs the default routing the TS
        // `beforeEach` did, so a test's own shape has to land on top of it.
        set_routing(&["pl-main", "openrouter"], "qwen3-14b", &[]);
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");
        assert_eq!(report.ambiguous, Some(vec!["pl-main:qwen3-14b".to_string(), "openrouter:qwen3-14b".to_string()]));
        assert_eq!(report.wrote, 0);
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
        // The results are still there for a human to read.
        assert!(matches!(outcome_of(&report, ProbeId::Json), ProbeOutcome::Scored { .. }));
    }

    #[tokio::test]
    async fn writes_nothing_when_no_key_can_be_derived_at_all() {
        let (deps, written, _asked, _route) = harness(good_reply(), None);
        // Same: the harness's default has to be cleared from above, not below.
        set_routing(&[], "qwen3-14b", &[]);
        let report = run_probes(&test_state(), "who-is-this", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");
        assert!(report.keys.is_empty());
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
    }

    #[tokio::test]
    async fn probes_a_fleet_persona_through_the_capability_keys_of_its_backing_model() {
        let (deps, written, _asked, _route) = harness(good_reply(), None);
        // No endpoints, the persona's backing keys instead — which only lands
        // after the harness has installed its default.
        set_routing(&[], "qwen3-14b", &["pl-main:qwen3-14b"]);
        let report = run_probes(&test_state(), "assistant-operations", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");
        assert_eq!(report.keys, vec!["pl-main:qwen3-14b"]);
        let w = written.lock().expect("the write recorder is not contended");
        assert_eq!(w[0].key, "pl-main:qwen3-14b");
        assert_eq!(w[0].cap, "json");
    }

    #[tokio::test]
    async fn does_not_turn_a_dropped_parameter_into_a_verdict_about_the_model() {
        let (deps, written, _asked, _route) = harness(
            Some(Arc::new(|_spec: &AskSpec| {
                let mut a = attempt();
                a.raw = GOOD.into();
                a.contract_dropped = true;
                a
            })),
            None,
        );
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");
        let w = written.lock().expect("the write recorder is not contended");
        assert!(w[0].fact.value);
        assert_eq!(w[0].fact.source, "probe");
        assert!(w[0].fact.detail.as_deref().unwrap_or("").contains("dropped response_format"), "got: {:?}", w[0].fact.detail);
    }

    // ── The armed tool probes ─────────────────────────────────────────────────
    //
    // `tool-select` is the fact that widens the Inbox command harness from a
    // regex-chosen single action to the item's whole action list (audit 1.8), and
    // until `TransportRequest` grew a slot for tool DEFINITIONS it skipped on
    // every run of every build — so the widening feature could not fire in
    // production and the admin saw a permanent "skipped" on the probe that would
    // arm it. These are the assertions for the armed path, and the strictness is
    // the point: a wrong `true` here hands a 7B model somebody's ticket.

    #[tokio::test]
    async fn scores_tool_select_from_four_real_calls_and_records_the_fact_that_arms_widening() {
        let (deps, written, asked, _route) = harness(
            None,
            Some(Arc::new(|spec: &ToolAskSpec| ToolAttempt {
                tool_calls: vec![call(correct(&spec.id), "{}")],
                transport_error: None,
            })),
        );
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::ToolSelect], deps))
            .await
            .expect("the run completes");

        assert_eq!(
            *asked.lock().expect("the ask recorder is not contended"),
            vec!["tool-select:weather", "tool-select:email", "tool-select:currency", "tool-select:ticket"]
        );
        match outcome_of(&report, ProbeId::ToolSelect) {
            ProbeOutcome::Scored { verdict, .. } => {
                assert!(verdict.value);
                close(verdict.score, 1.0);
            }
            other => panic!("expected scored, got {other:?}"),
        }
        let w = written.lock().expect("the write recorder is not contended");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].cap, "tool-select");
        assert!(w[0].fact.value);
        assert_eq!(w[0].fact.source, "probe");
        assert_eq!(w[0].fact.score, Some(1.0));
    }

    #[tokio::test]
    async fn refuses_the_fact_on_3_of_4_the_fourth_pick_is_an_action_taken_on_somebody_elses_s_ticket() {
        let (deps, written, _asked, _route) = harness(
            None,
            Some(Arc::new(|spec: &ToolAskSpec| ToolAttempt {
                tool_calls: vec![call(
                    if spec.id == "tool-select:currency" { "send_email" } else { correct(&spec.id) },
                    "{}",
                )],
                transport_error: None,
            })),
        );
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::ToolSelect], deps))
            .await
            .expect("the run completes");

        match outcome_of(&report, ProbeId::ToolSelect) {
            // Recorded as FALSE rather than left unknown: four conclusive trials with a
            // wrong pick in them is a measurement, and the score says how close it got.
            ProbeOutcome::Scored { verdict, .. } => {
                assert!(!verdict.value);
                close(verdict.score, 0.75);
            }
            other => panic!("expected scored, got {other:?}"),
        }
        let w = written.lock().expect("the write recorder is not contended");
        assert_eq!(w[0].cap, "tool-select");
        assert!(!w[0].fact.value);
        assert_eq!(w[0].fact.source, "probe");
    }

    #[tokio::test]
    async fn scores_tools_off_one_offered_definition_and_the_arguments_that_came_back() {
        let (deps, written, _asked, route1) = harness(
            None,
            Some(Arc::new(|_spec: &ToolAskSpec| ToolAttempt {
                tool_calls: vec![call("get_weather", "{\"city\":\"Lisbon\"}")],
                transport_error: None,
            })),
        );
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Tools], deps))
            .await
            .expect("the run completes");
        {
            let w = written.lock().expect("the write recorder is not contended");
            assert_eq!(w[0].cap, "tools");
            assert!(w[0].fact.value);
            assert_eq!(w[0].fact.source, "probe");
        }

        // Prose instead of a call is the failure this probe exists to catch, and it
        // is what a model with no tool support does.
        drop(route1); // the turnstile is re-taken by the second harness below
        let (prose_deps, prose_written, _asked, _route) = harness(
            None,
            Some(Arc::new(|_spec: &ToolAskSpec| ToolAttempt { tool_calls: Vec::new(), transport_error: None })),
        );
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Tools], prose_deps))
            .await
            .expect("the run completes");
        let w = prose_written.lock().expect("the write recorder is not contended");
        assert!(!w[0].fact.value);
        assert!(w[0].fact.detail.as_deref().unwrap_or("").contains("prose"), "got: {:?}", w[0].fact.detail);
    }

    #[tokio::test]
    async fn writes_nothing_when_the_tool_call_never_completed_a_401_is_not_a_model_that_cannot_call_tools() {
        let (deps, written, _asked, _route) = harness(
            None,
            Some(Arc::new(|_spec: &ToolAskSpec| ToolAttempt {
                tool_calls: Vec::new(),
                transport_error: Some("gateway completion 401: bad key".into()),
            })),
        );
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Tools, ProbeId::ToolSelect], deps))
            .await
            .expect("the run completes");
        assert!(matches!(outcome_of(&report, ProbeId::Tools), ProbeOutcome::Errored { .. }));
        assert!(matches!(outcome_of(&report, ProbeId::ToolSelect), ProbeOutcome::Errored { .. }));
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
    }

    #[tokio::test]
    async fn skips_both_tool_probes_on_a_fleet_persona_rather_than_scoring_one() {
        // The persona's tool loop runs inside the agent container: tools we offer
        // are neither guaranteed to reach the model nor observable when called, so
        // the transport refuses the call. A skip writes nothing; scoring it would
        // write `tools: false` — permanently — about a model nobody asked.
        let (mut deps, written, asked, _route) = harness(None, None);
        deps.offers_tool_definitions = Arc::new(|| Box::pin(async { false }));
        let report = run_probes(&test_state(), "assistant-operations", opts(&[ProbeId::Tools, ProbeId::ToolSelect], deps))
            .await
            .expect("the run completes");

        for id in [ProbeId::Tools, ProbeId::ToolSelect] {
            match outcome_of(&report, id) {
                ProbeOutcome::Skipped { reason, .. } => assert!(reason.contains("fleet persona"), "got: {reason}"),
                other => panic!("expected skipped, got {other:?}"),
            }
        }
        assert!(asked.lock().expect("the ask recorder is not contended").is_empty());
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
    }

    #[tokio::test]
    async fn gives_the_structural_reason_vision_cannot_run_advertised_or_not() {
        // It used to skip with "this endpoint does not advertise vision" whenever a
        // catalog said nothing — which was the reason shown for every Claude model,
        // reads as a fact about Claude, and is a fact about a terse catalog. Worse,
        // it hid the real blocker: `Message.content` is a string across the whole
        // tree, so no turn can carry an image part. That is the one thing an admin
        // could act on and it was invisible behind the catalog gate.
        for advertises in [false, true] {
            let (mut deps, _written, _asked, _route) = harness(None, None);
            deps.advertises = Arc::new(move |_id: ProbeId| Box::pin(async move { advertises }));
            let out = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Vision], deps))
                .await
                .expect("the run completes");
            match outcome_of(&out, ProbeId::Vision) {
                ProbeOutcome::Skipped { reason, .. } => assert!(reason.contains("image parts"), "got: {reason}"),
                other => panic!("advertises={advertises}: expected skipped, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn measures_long_context_when_nothing_advertises_a_window_and_says_the_window_was_assumed() {
        // Anthropic's /v1/models returns an id and a display name and nothing else,
        // so this skipped on every Claude model — a permanent hole in the matrix for
        // models with some of the largest windows there are. Nothing here may
        // hardcode a provider's window, so the answer is to measure at the probe's
        // own ceiling and SAY that is what happened.
        let (deps, _written, _asked, _route) = harness(None, None);
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::LongContext], deps))
            .await
            .expect("the run completes");
        match outcome_of(&report, ProbeId::LongContext) {
            ProbeOutcome::Scored { verdict, .. } => {
                assert!(verdict.detail.contains("advertises no window"), "got: {}", verdict.detail);
            }
            other => panic!("expected scored, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn still_skips_a_window_too_small_to_be_called_long() {
        // The one long-context skip that is about the MODEL rather than about a
        // catalog: testing 4k proves nothing about long context.
        let (mut deps, _written, _asked, _route) = harness(None, None);
        deps.context_window = Arc::new(|| Box::pin(async { Some(4_096.0) }));
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::LongContext], deps))
            .await
            .expect("the run completes");
        match outcome_of(&report, ProbeId::LongContext) {
            ProbeOutcome::Skipped { reason, .. } => assert!(reason.contains("below the"), "got: {reason}"),
            other => panic!("expected skipped, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn caps_a_huge_advertised_window_and_says_in_the_detail_what_it_actually_tested() {
        let (mut deps, written, _asked, _route) = harness(None, None);
        deps.context_window = Arc::new(|| Box::pin(async { Some(1_000_000.0) }));
        deps.max_context_tokens = 32_000;
        deps.needle_token = "GRANITE-FOX-7731".into();
        deps.ask = Arc::new(|_spec: AskSpec| {
            let mut a = attempt();
            a.raw = "granite-fox-7731".into();
            Box::pin(async move { a })
        });
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::LongContext], deps))
            .await
            .expect("the run completes");
        let w = written.lock().expect("the write recorder is not contended");
        assert!(w[0].fact.value);
        assert_eq!(w[0].fact.source, "probe");
        // 80% of the 32k cap, not 90% of the million it advertises.
        assert!(w[0].fact.detail.as_deref().unwrap_or("").contains("25,600"), "got: {:?}", w[0].fact.detail);
    }

    #[tokio::test]
    async fn verifies_a_search_citation_by_fetching_the_page_and_calls_an_unreadable_page_inconclusive() {
        let reply = serde_json::json!({
            "date": "2026-08-06",
            "url": "https://news.example-press.org/ferry",
            "quote": "The council approved the new ferry timetable on Tuesday evening after a long debate.",
        })
        .to_string();

        let (mut good, good_written, _asked, route1) = harness(
            Some(Arc::new({
                let reply = reply.clone();
                move |_spec: &AskSpec| {
                    let mut a = attempt();
                    a.raw = reply.clone();
                    a
                }
            })),
            None,
        );
        good.fetch_text = Arc::new(|_url: String| {
            Box::pin(async {
                Some("<p>The council approved the new ferry timetable on Tuesday evening after a long debate.</p>".to_string())
            })
        });
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Search], good))
            .await
            .expect("the run completes");
        {
            let w = good_written.lock().expect("the write recorder is not contended");
            assert_eq!(w[0].cap, "search");
            assert!(w[0].fact.value);
            assert_eq!(w[0].fact.source, "probe");
        }

        drop(route1); // the turnstile is re-taken by the second harness below
        let (mut blocked, blocked_written, _asked, _route) = harness(
            Some(Arc::new(move |_spec: &AskSpec| {
                let mut a = attempt();
                a.raw = reply.clone();
                a
            })),
            None,
        );
        blocked.fetch_text = Arc::new(|_url: String| Box::pin(async { None::<String> }));
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Search], blocked))
            .await
            .expect("the run completes");
        assert!(blocked_written.lock().expect("the write recorder is not contended").is_empty());
        assert!(matches!(outcome_of(&report, ProbeId::Search), ProbeOutcome::Skipped { .. }));
    }

    #[tokio::test]
    async fn records_search_false_when_the_model_cannot_name_today_on_either_trial() {
        let stale = serde_json::json!({
            "date": "2024-06-01",
            "url": "https://example.com/x",
            "quote": "x".repeat(50),
        })
        .to_string();
        let (deps, written, _asked, _route) = harness(
            Some(Arc::new(move |_spec: &AskSpec| {
                let mut a = attempt();
                a.raw = stale.clone();
                a
            })),
            None,
        );
        run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Search], deps))
            .await
            .expect("the run completes");
        let w = written.lock().expect("the write recorder is not contended");
        assert_eq!(w[0].cap, "search");
        assert!(!w[0].fact.value);
        assert_eq!(w[0].fact.source, "probe");
    }

    #[tokio::test]
    async fn is_stable_the_same_recorded_replies_score_identically_twice() {
        fn replies(spec: &AskSpec) -> Attempt {
            let mut a = attempt();
            a.raw = if spec.id.starts_with("json:") {
                GOOD.into()
            } else if spec.id.starts_with("code:slugify") {
                GOOD_SLUGIFY.into()
            } else if spec.id.starts_with("code:") {
                "function mergeRanges(r) { return r }".into()
            } else {
                "OK".into()
            };
            a
        }
        let ids = [ProbeId::Json, ProbeId::Code, ProbeId::InstructionFollowing];
        // Two harnesses, one turnstile: the first guard must be dropped before
        // the second harness() can take it. Routing here is the default on both,
        // so releasing between the installs is safe.
        let (first_deps, first_written, _asked, route1) = harness(Some(Arc::new(replies)), None);
        drop(route1);
        let (second_deps, second_written, _asked, _route) = harness(Some(Arc::new(replies)), None);
        run_probes(&test_state(), "qwen3-14b", opts(&ids, first_deps)).await.expect("the run completes");
        run_probes(&test_state(), "qwen3-14b", opts(&ids, second_deps)).await.expect("the run completes");
        let triple = |w: &Written| (w.cap.clone(), w.fact.value, w.fact.score);
        let first: Vec<(String, bool, Option<f64>)> =
            first_written.lock().expect("the write recorder is not contended").iter().map(triple).collect();
        let second: Vec<(String, bool, Option<f64>)> =
            second_written.lock().expect("the write recorder is not contended").iter().map(triple).collect();
        // Identical TWICE, order included — the write order is part of what
        // stability means for a report an admin diffs by eye.
        assert_eq!(second, first);
        // The expected SCORES, compared by capability: probes run concurrently,
        // so which write lands first is not part of the contract this test
        // guards.
        let by_cap = |v: &[(String, bool, Option<f64>)]| {
            let mut v = v.to_vec();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };
        let expected = vec![
            ("json".to_string(), true, Some(1.0)),
            ("code".to_string(), false, Some(0.5)),
            ("instruction-following".to_string(), false, Some(1.0 / 3.0)),
        ];
        assert_eq!(by_cap(&first), by_cap(&expected));
    }

    #[tokio::test]
    async fn reads_latency_off_the_existing_gateway_pulse_ring_rather_than_timing_its_own_calls() {
        // The TS mocks `gatewayPulse` to a fixed {12,1,140,620}. The Rust ring is
        // the process-global one the real readers share — which is the whole point
        // of the function — so the assertion weakens to: our own recorded calls
        // are in the reading, and an unpriced model carries no cost.
        //
        // The turnstile: gateway::upstream's pulse test asserts EXACT nearest-rank
        // percentiles over this same ring, so the two of us take turns — its
        // writer holds the turnstile for its body, and this one holds it across
        // the seeds and the read.
        let _ring = crate::gateway::upstream::pulse_tests::STAT_RING_TURNSTILE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::gateway::upstream::record_gateway_stat(140, true, "qwen3-14b");
        crate::gateway::upstream::record_gateway_stat(620, false, "qwen3-14b");
        crate::gateway::upstream::record_gateway_stat(200, true, "qwen3-14b");
        let (deps, _written, _asked, _route) = harness(good_reply(), None);
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Json], deps))
            .await
            .expect("the run completes");
        assert!(report.latency.requests >= 3);
        assert!(report.latency.errors >= 1);
        assert!(report.latency.usd.is_none());
    }

    // ── The estimate ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn estimate_reports_calls_and_tokens_per_probe_before_anything_is_spent() {
        let (deps, _written, _asked, _route) = harness(None, None);
        let est = estimate_probes(&test_state(), "qwen3-14b", Some(&[ProbeId::Json, ProbeId::InstructionFollowing]), Some(&deps), false).await;
        assert_eq!(est.calls, 6);
        assert!(est.prompt_tokens > 0);
        assert_eq!(est.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![ProbeId::Json, ProbeId::InstructionFollowing]);
        assert_eq!(est.usd, None);
    }

    #[tokio::test]
    async fn estimate_prices_the_run_when_the_endpoint_has_a_price_using_dollar_per_mtok() {
        let (mut deps, _written, _asked, _route) = harness(None, None);
        deps.price = Arc::new(|| {
            Box::pin(async {
                Some(TokPrice { in_per_mtok: 1.0, out_per_mtok: 2.0 })
            })
        });
        let est = estimate_probes(&test_state(), "qwen3-14b", Some(&[ProbeId::Json]), Some(&deps), false).await;
        let usd = est.usd.expect("priced");
        close(usd, (est.prompt_tokens as f64 * 1.0 + est.completion_tokens as f64 * 2.0) / 1e6);
    }

    #[tokio::test]
    async fn estimate_sizes_long_context_from_the_capped_window_rather_than_from_a_fixture() {
        let (mut deps, _written, _asked, _route) = harness(None, None);
        deps.context_window = Arc::new(|| Box::pin(async { Some(1_000_000.0) }));
        let est = estimate_probes(&test_state(), "qwen3-14b", Some(&[ProbeId::LongContext]), Some(&deps), false).await;
        assert_eq!(est.rows[0].prompt_tokens, 25_600);
        assert_eq!(est.calls, 2);
    }

    #[tokio::test]
    async fn estimate_bills_long_context_at_the_probe_ceiling_when_no_window_is_advertised_because_it_now_runs() {
        // The estimate reads the same edges the run reads. It used to bill zero here
        // on the grounds that the probe would skip; the probe no longer skips, and an
        // estimate that still said zero would understate a real 25,600-token pair of
        // calls.
        let (deps, _written, _asked, _route) = harness(None, None);
        let est = estimate_probes(&test_state(), "qwen3-14b", Some(&[ProbeId::LongContext]), Some(&deps), false).await;
        assert_eq!(est.calls, 2);
        assert!(est.prompt_tokens > 20_000);
    }

    #[tokio::test]
    async fn estimate_charges_nothing_for_a_probe_that_will_skip_a_fleet_candidate_and_an_endpoint_with_no_vision() {
        // The estimate is the sentence in front of a button that spends someone
        // else's inference budget, and billing six calls for three probes that
        // cannot run overstates a probes-only run by a fifth — exactly the kind of
        // number that makes an admin stop trusting the rest of the page.
        let (mut deps, _written, _asked, _route) = harness(None, None);
        deps.offers_tool_definitions = Arc::new(|| Box::pin(async { false }));
        let est =
            estimate_probes(&test_state(), "assistant-operations", Some(&[ProbeId::Tools, ProbeId::ToolSelect, ProbeId::Vision]), Some(&deps), false).await;
        assert_eq!(est.calls, 0);
        assert_eq!(est.usd, None);
    }

    #[tokio::test]
    async fn estimate_bills_the_tool_probes_now_they_are_armed_because_that_is_the_number_an_admin_decides_to_spend() {
        // The other half of the same honesty: a run that WILL make five tool calls
        // must say five. `ask_with_images` is still shut, so vision alone stays free.
        let (mut deps, _written, _asked, _route) = harness(None, None);
        deps.advertises = Arc::new(|_id: ProbeId| Box::pin(async { true }));
        let est =
            estimate_probes(&test_state(), "qwen3-14b", Some(&[ProbeId::Tools, ProbeId::ToolSelect, ProbeId::Vision]), Some(&deps), false).await;
        assert_eq!(est.calls, 5);
        assert_eq!(est.rows.iter().find(|r| r.id == ProbeId::Tools).map(|r| r.calls), Some(1));
        assert_eq!(est.rows.iter().find(|r| r.id == ProbeId::ToolSelect).map(|r| r.calls), Some(4));
        assert_eq!(est.rows.iter().find(|r| r.id == ProbeId::Vision).map(|r| r.calls), Some(0));
    }

    #[tokio::test]
    async fn estimate_matches_the_calls_the_run_actually_makes_over_every_probe_that_can_run_here() {
        // The estimate and the run read the SAME edges, so this is a real invariant
        // rather than two constants agreeing. Counted against the asks both the
        // text and the tool channels record.
        let (mut deps, _written, asked, _route) = harness(
            Some(Arc::new(|_spec: &AskSpec| {
                let mut a = attempt();
                a.raw = "OK".into();
                a
            })),
            Some(Arc::new(|_spec: &ToolAskSpec| ToolAttempt {
                tool_calls: vec![call("get_weather", "{\"city\":\"Lisbon\"}")],
                transport_error: None,
            })),
        );
        deps.context_window = Arc::new(|| Box::pin(async { Some(32_000.0) }));
        deps.advertises = Arc::new(|_id: ProbeId| Box::pin(async { true }));
        let est = estimate_probes(&test_state(), "qwen3-14b", None, Some(&deps), false).await;
        run_probes(&test_state(), "qwen3-14b", opts(&[], deps)).await.expect("the run completes");
        assert_eq!(asked.lock().expect("the ask recorder is not contended").len(), est.calls as usize);
    }

    // ── The production `ask`: run_harness with the candidate pinned ───────────────

    fn transport_reply(text: &str, tool_calls: Option<Vec<ToolCall>>, contract_dropped: bool) -> TransportReply {
        TransportReply {
            kind: TransportKind::Gateway,
            text: text.into(),
            tool_names: Vec::new(),
            tool_calls,
            usage: None,
            contract_dropped,
        }
    }

    fn prompt() -> Vec<Message> {
        vec![Message::user("return the object")]
    }

    /// What the scripted transport saw: model, JSON mode, and the tool definitions
    /// by name and parameters (`ToolDefinition` is not `Serialize`, so the pairs
    /// stand in for the TS's deep-equal on the definition objects).
    type SeenRequest = (String, bool, Vec<(String, Value)>);

    #[tokio::test]
    async fn runner_ask_pins_the_candidate_and_asks_for_json_at_the_protocol_level() {
        let (seen, base): (Arc<Mutex<Vec<SeenRequest>>>, TransportFn) = {
            let seen: Arc<Mutex<Vec<SeenRequest>>> = Arc::new(Mutex::new(Vec::new()));
            let base: TransportFn = {
                let seen = seen.clone();
                Arc::new(move |req: TransportRequest| {
                    let seen = seen.clone();
                    Box::pin(async move {
                        seen.lock().expect("the request recorder is not contended").push((
                            req.model.clone(),
                            req.json_mode,
                            req.tool_defs.iter().map(|d| (d.name.clone(), d.parameters.clone())).collect(),
                        ));
                        Ok(transport_reply(GOOD, None, false))
                    })
                })
            };
            (seen, base)
        };
        let ask = runner_ask(&test_state(), "vendor/frontier-1", base);
        let a = ask(AskSpec { id: "json:trivial".into(), messages: prompt(), schema: Some(JSON_TRIVIAL.clone()) }).await;
        let seen = seen.lock().expect("the request recorder is not contended");
        assert_eq!(seen[0].0, "vendor/frontier-1");
        assert!(seen[0].1, "the runner asks for JSON at the protocol level");
        assert!(a.contract_held);
        assert!(a.json_requested);
        assert!(!a.contract_dropped);
        assert!(a.transport_error.is_none());
        assert!(a.raw.contains("talaria"), "got: {}", a.raw);
    }

    #[tokio::test]
    async fn runner_ask_measures_the_first_attempt_no_repair_turn_so_the_score_is_the_contract_rate() {
        let calls = Arc::new(Mutex::new(0));
        let base: TransportFn = {
            let calls = calls.clone();
            Arc::new(move |_req: TransportRequest| {
                let calls = calls.clone();
                Box::pin(async move {
                    *calls.lock().expect("the call counter is not contended") += 1;
                    Ok(transport_reply("Sure! Here is the object you asked for.", None, false))
                })
            })
        };
        let ask = runner_ask(&test_state(), "vendor/frontier-1", base);
        let a = ask(AskSpec { id: "json:trivial".into(), messages: prompt(), schema: Some(JSON_TRIVIAL.clone()) }).await;
        assert_eq!(*calls.lock().expect("the call counter is not contended"), 1);
        assert!(!a.contract_held);
    }

    #[tokio::test]
    async fn runner_ask_carries_the_gateway_contract_drop_through_which_is_the_whole_audit_1_2_signal() {
        let base: TransportFn = Arc::new(|_req: TransportRequest| {
            Box::pin(async { Ok(transport_reply(GOOD, None, true)) })
        });
        let ask = runner_ask(&test_state(), "vendor/frontier-1", base);
        let a = ask(AskSpec { id: "json:trivial".into(), messages: prompt(), schema: Some(JSON_TRIVIAL.clone()) }).await;
        assert!(a.contract_dropped);
        assert!(a.contract_held);
    }

    #[tokio::test]
    async fn runner_ask_reports_a_transport_throw_as_a_transport_error_not_as_a_model_that_answered_badly() {
        let base: TransportFn = Arc::new(|_req: TransportRequest| {
            Box::pin(async { Err("gateway completion 401: bad key".to_string()) })
        });
        let ask = runner_ask(&test_state(), "vendor/frontier-1", base);
        let a = ask(AskSpec { id: "json:trivial".into(), messages: prompt(), schema: Some(JSON_TRIVIAL.clone()) }).await;
        assert_eq!(a.transport_error.as_deref(), Some("gateway completion 401: bad key"));
        assert!(!a.contract_held);
    }

    #[tokio::test]
    async fn runner_ask_hands_a_text_probe_the_reply_verbatim_because_exactly_ok_means_exactly() {
        let base: TransportFn = Arc::new(|_req: TransportRequest| {
            Box::pin(async { Ok(transport_reply("  OK\n", None, false)) })
        });
        let ask = runner_ask(&test_state(), "vendor/frontier-1", base);
        let a = ask(AskSpec { id: "instruction:exactly OK".into(), messages: prompt(), schema: None }).await;
        assert_eq!(a.raw, "  OK\n");
        assert!(!a.json_requested);
    }

    // ── The tool ask: the same runner, with definitions on the request ─────────

    #[tokio::test]
    async fn runner_tool_ask_puts_the_definitions_on_the_request_and_reports_the_calls_back() {
        let seen: Arc<Mutex<Vec<SeenRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let base: TransportFn = {
            let seen = seen.clone();
            Arc::new(move |req: TransportRequest| {
                let seen = seen.clone();
                Box::pin(async move {
                    seen.lock().expect("the request recorder is not contended").push((
                        req.model.clone(),
                        req.json_mode,
                        req.tool_defs.iter().map(|d| (d.name.clone(), d.parameters.clone())).collect(),
                    ));
                    Ok(transport_reply(
                        "",
                        Some(vec![ToolCall {
                            name: "get_weather".into(),
                            id: None,
                            args: "{\"city\":\"Lisbon\"}".into(),
                        }]),
                        false,
                    ))
                })
            })
        };
        let ask = runner_tool_ask(&test_state(), "vendor/frontier-1", base);
        let a = ask(ToolAskSpec { id: "tools".into(), messages: prompt(), tools: vec![weather_tool()] }).await;

        let seen = seen.lock().expect("the request recorder is not contended");
        assert_eq!(seen[0].0, "vendor/frontier-1");
        let weather = weather_tool();
        assert_eq!(seen[0].2, vec![("get_weather".to_string(), weather.parameters.clone())]);
        // A tool-calling turn usually returns EMPTY content, which every text
        // contract in the tree reads as a failure — so the probe grades the calls,
        // not the value, and a failed contract here is not a failed trial.
        assert!(!seen[0].1, "a tool-calling turn is not a JSON turn");
        assert_eq!(a.tool_calls.len(), 1);
        assert_eq!(a.tool_calls[0].name, "get_weather");
        assert_eq!(a.tool_calls[0].args, "{\"city\":\"Lisbon\"}");
        assert!(a.transport_error.is_none());
    }

    #[tokio::test]
    async fn runner_tool_ask_treats_a_transport_that_reports_no_tool_call_channel_as_an_error() {
        // ABSENT IS NOT EMPTY. The dispatcher cannot produce this — the fleet path
        // refuses a request carrying definitions — but a bespoke transport could,
        // and reading None as "called nothing" would write `tools: false`
        // forever about a model that was never offered a tool.
        let base: TransportFn = Arc::new(|_req: TransportRequest| {
            Box::pin(async { Ok(transport_reply("sure, it is sunny", None, false)) })
        });
        let ask = runner_tool_ask(&test_state(), "vendor/frontier-1", base);
        let a = ask(ToolAskSpec { id: "tools".into(), messages: prompt(), tools: vec![weather_tool()] }).await;
        assert!(a.tool_calls.is_empty());
        assert!(a.transport_error.as_deref().is_some_and(|e| e.contains("without reporting any tool calls")), "got: {:?}", a.transport_error);
    }

    #[tokio::test]
    async fn runner_tool_ask_reports_a_refusal_from_the_transport_as_a_transport_error_which_voids_the_probe() {
        let base: TransportFn = Arc::new(|_req: TransportRequest| {
            Box::pin(async { Err("its tool loop runs inside the agent container".to_string()) })
        });
        let ask = runner_tool_ask(&test_state(), "assistant-operations", base);
        let a = ask(ToolAskSpec { id: "tools".into(), messages: prompt(), tools: vec![weather_tool()] }).await;
        assert!(a.transport_error.as_deref().is_some_and(|e| e.contains("tool loop runs inside the agent")), "got: {:?}", a.transport_error);
    }

    // ── The real deps, in the shape the server will build them ───────────────────
    //
    // The three that read endpoint rows out of postgres are noted up with the
    // scorer tests; these are the two that do not.

    #[tokio::test]
    async fn default_deps_opens_every_ask_channel_so_no_capability_goes_unmeasured_for_want_of_a_seam() {
        // Both were shut once and each left a permanent hole in the matrix.
        // `ask_with_tools` carries real definitions through the transport request
        // and reads back what was called. `ask_with_images` hands the multimodal
        // body to the gateway image seam — which measures the MODEL without
        // widening `Message.content` tree-wide, the change that argument was
        // really about. The TS's `typeof === 'function'` assertions on the other
        // two edges are tautologies in Rust — a struct field of function type
        // cannot be absent — so the one that was ever false is the one asserted.
        let deps = default_deps(&test_state(), "qwen3-14b");
        assert!(deps.ask_with_images.is_some(), "the image channel opens by default");
    }

    #[tokio::test]
    async fn default_deps_caps_context_spend_by_default_a_200k_window_probed_at_90_percent_is_dollars_not_cents() {
        assert_eq!(default_deps(&test_state(), "qwen3-14b").max_context_tokens, DEFAULT_MAX_CONTEXT_TOKENS);
        assert!(MIN_LONG_CONTEXT_TOKENS < DEFAULT_MAX_CONTEXT_TOKENS);
    }

    // ── The wall clock ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn gives_up_on_a_transport_that_never_settles_and_writes_nothing() {
        // Tier 2 races every case; tier 1 raced nothing. A provider that accepted the
        // connection and went away left the run awaiting a call that never
        // settled — holding a run slot forever, unreachable by Stop (honored only
        // between tiers). With eight candidates able to run at once, a few hung calls
        // take slots permanently.
        let (mut deps, written, _asked, _route) = harness(None, None);
        deps.ask = Arc::new(|_spec: AskSpec| Box::pin(std::future::pending::<Attempt>()));
        let mut run = opts(&[ProbeId::InstructionFollowing], deps);
        run.timeout_ms = Some(30);
        let report = run_probes(&test_state(), "qwen3-14b", run).await.expect("the run completes");

        assert!(matches!(outcome_of(&report, ProbeId::InstructionFollowing), ProbeOutcome::Errored { .. }));
        // A timeout measured NOTHING about the model, so by rule 2 no fact is stored.
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
    }

    // ── An endpoint that refuses the image itself ────────────────────────────────

    #[tokio::test]
    async fn records_the_model_as_unable_to_take_images_not_the_deployment_as_broken() {
        // OpenRouter answers a text-only model with `404 No endpoints found that
        // support image input`. That is not a broken gateway — it is the deployment
        // saying plainly what this model can be sent, which is exactly what a
        // capability key (`endpoint:model`) addresses. It used to land in `errored`,
        // which writes nothing and reads to an admin as "something is wrong".
        let (mut deps, written, _asked, _route) = harness(None, None);
        deps.ask_with_images = Some(Arc::new(|_spec: ImageAskSpec| {
            let mut a = attempt();
            a.transport_error =
                Some("gateway completion 404: {\"error\":{\"message\":\"No endpoints found that support image input\"}}".into());
            Box::pin(async move { a })
        }));
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Vision], deps))
            .await
            .expect("the run completes");

        assert!(matches!(outcome_of(&report, ProbeId::Vision), ProbeOutcome::Scored { .. }));
        let w = written.lock().expect("the write recorder is not contended");
        assert_eq!(w[0].cap, "vision");
        assert!(!w[0].fact.value);
    }

    #[tokio::test]
    async fn leaves_an_ordinary_outage_as_an_error_because_nothing_was_measured() {
        let (mut deps, written, _asked, _route) = harness(None, None);
        deps.ask_with_images = Some(Arc::new(|_spec: ImageAskSpec| {
            let mut a = attempt();
            a.transport_error = Some("gateway completion 503: upstream restarting".into());
            Box::pin(async move { a })
        }));
        let report = run_probes(&test_state(), "qwen3-14b", opts(&[ProbeId::Vision], deps))
            .await
            .expect("the run completes");

        assert!(matches!(outcome_of(&report, ProbeId::Vision), ProbeOutcome::Errored { .. }));
        assert!(written.lock().expect("the write recorder is not contended").is_empty());
    }
}
