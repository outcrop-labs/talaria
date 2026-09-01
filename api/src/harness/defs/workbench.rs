// THE CODING HARNESSES — three of them, one per Workbench effort slot. Port
// of harness/defs/workbench.ts.
//
// WHY THEY EXIST. `MODEL_ROLES` has carried `code-light`, `code-standard` and
// `code-heavy` since the Workbench shipped, and the fitness matrix printed all
// three as "No harness in this install is bound to Workbench · X effort, so a
// sweep can say nothing about a model for it." That sentence was accurate and
// useless: an admin assigning a coder had no evidence to assign on, from a
// page whose entire job is to give them some.
//
// WHAT THEY MEASURE, and it is narrower than "is this a good coding model".
// The role's own declaration says what matters — `requires: ['code',
// 'tools']`, with the hint spelling out why: "without tool calling the run
// does not degrade, it does nothing while reporting that it worked". So the
// question is:
//
//     given a repository, a failing test and file tools, does this model
//     LOCATE the defect, EDIT the right file, and CHECK its own work?
//
// A model that answers the bug in prose has failed. A model that writes a
// plausible patch to the wrong file has failed. A model that fixes it and
// never runs the tests has done the job and cannot know it. Those three are
// the failures a Workbench run actually hits, and none of them is visible
// from a reply's text.
//
// WHY THREE HARNESSES AND NOT ONE WITH A PARAMETER. A harness has ONE
// `ModelSpec`, and the scorer binds a harness to a slot by running the REAL
// resolver over it. One harness declaring `role: 'code-standard'` binds one
// column and leaves the other two exactly as empty as they were. Three
// definitions over one shared builder is what actually fills the row — and it
// is honest besides: the three slots are three different purchases, and an
// admin comparing them wants the same task run against each.
//
// WHAT THIS IS A PROXY FOR, AND WHAT IT IS NOT A REPLAY OF. Production does
// NOT run this loop. A Workbench run hands the model to an EXTERNAL CODING
// AGENT running in a container with its own prompt, its own tool surface and
// its own turn budget; Talaria chooses the MODEL through the `code-*` roles
// and controls nothing else about that loop. So what these three harnesses
// measure is the model's own coding behaviour on a surface Talaria drives:
// locate the defect, edit the right file, check the work. That is genuinely
// predictive — a model that patches the wrong file here will patch the wrong
// file there — and it is NOT the same claim as "this model works under an
// external harness". A green cell is evidence about the model, not a
// rehearsal of the production path, and no fixture below should ever be
// written as though it were.
//
// THE TASKS ARE THE SAME ACROSS ALL THREE, deliberately. The efforts differ
// in what an admin SPENDS, not in what the job is; running a harder suite
// against the heavy slot would make the three columns incomparable, which is
// the one thing a matrix must not be.
//
// THE DRY RUN CROSSED with the fitness plane's `EvalCase`/`DryRunDecl` slots;
// only the EXECUTOR that replays it stays behind (see define.rs's header), and
// this is the one def whose dry run is more than a turn count: the workspace
// is the TASK — the fixture's own files, and the oracle that decides whether
// the suite passes against what the model left. The oracles are wired into
// that declaration rather than re-derived (they are pure functions of the
// files); the numbers are preserved with it: a bench runs TWELVE turns, and
// twelve has a history worth recording — six filed nine of one model's
// workbench cases as OUR gap ("the turn budget ran out while the model was
// still working"), ten still filed three across a twelve-model archive (p50
// is 6 tool calls, p90 is 8, the tail reaches 13, and a model at the top of
// that distribution is still editing when the loop stops), and twelve is the
// turn ceiling, so the next honest move is a smaller task rather than a
// bigger budget. A bench benches the FIVE sandbox tools (`list_files`,
// `read_file`, `search`, `write_file`, `run_tests`) — and declares a WORKSPACE
// rather than a Talaria world, which is why the sandbox itself
// (fitness/toolbox/hermes-tools) crosses with the fitness plane and not with
// this file.

use std::sync::{Arc, OnceLock};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::body::js_string;
use crate::harness::define::{
    CheckCtx, CheckResult, DryRunDecl, EvalBand, EvalCase, GuardDecl, HarnessDefinition, Message,
    OnFailure, Output, RenderContext, RoleFloor, WorkspaceFile, WorkspaceSpec, define_harness,
};
use crate::harness::transport::ToolPolicy;
use crate::harness_model::ModelSpec;

// ── The shapes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchTaskInput {
    /// What the agent has been asked to do, as a work session would phrase it.
    pub task: String,
    /// The repository it starts from.
    pub files: Vec<WorkbenchFile>,
    /// The failing test, as the runner first reported it.
    pub failure: String,
}

// ── The oracles ──────────────────────────────────────────────────────────────

/// THE ORACLE, per task: applied to the files as the model left them, `None`
/// is a green suite. A regular function rather than a closure because the
/// task table holds it as data.
type Oracle = fn(&[WorkbenchFile]) -> Option<String>;

fn file_at<'a>(files: &'a [WorkbenchFile], path: &str) -> &'a str {
    files
        .iter()
        .find(|f| f.path == path)
        .map(|f| f.content.as_str())
        .unwrap_or("")
}

// ── Task 1: an off-by-one in a pagination helper ─────────────────────────────
//
// The classic. Small, unambiguous, one line to change, and the test names the
// expected value — so a model that reads the test knows the answer without
// having to reason about pagination at all. This is the EASY band's floor.

const PAGINATE: &str = r#"export function pageRange(page, perPage, total) {
  const start = page * perPage
  const end = Math.min(start + perPage, total)
  return { start, end }
}
"#;

const PAGINATE_TEST: &str = r#"import { pageRange } from './paginate.js'
import { test, assert } from './harness.js'

// Pages are ONE-BASED everywhere in this codebase.
test('page 1 starts at the beginning', () => {
  assert.deepEqual(pageRange(1, 20, 100), { start: 0, end: 20 })
})
test('page 3 of 20-per-page starts at 40', () => {
  assert.deepEqual(pageRange(3, 20, 100), { start: 40, end: 60 })
})
"#;

fn one_based_paging() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"\(\s*page\s*-\s*1\s*\)\s*\*\s*perPage|perPage\s*\*\s*\(\s*page\s*-\s*1\s*\)|start\s*=\s*page\s*\*\s*perPage\s*-\s*perPage")
            .unwrap()
    })
}

fn paginate_oracle(files: &[WorkbenchFile]) -> Option<String> {
    let src = file_at(files, "src/paginate.js");
    if src.trim().is_empty() {
        return Some("src/paginate.js is empty or missing".into());
    }
    // The fix is one-based paging: `(page - 1) * perPage`. Accept any spelling
    // that computes it — a benchmark that demanded one exact diff would
    // measure obedience rather than capability.
    if !one_based_paging().is_match(src) {
        return Some(
            "pageRange(1, 20, 100) returned { start: 20, end: 40 }, expected { start: 0, end: 20 } — pages are one-based"
                .into(),
        );
    }
    if !src.contains("Math.min") {
        return Some(
            "pageRange(5, 20, 90) returned { end: 100 }, expected 90 — end must be clamped to total"
                .into(),
        );
    }
    None
}

// ── Task 2: the bug is NOT in the file the task names ────────────────────────
//
// The task says the total is wrong on the invoice. It is — because the
// rounding helper in a DIFFERENT file truncates instead of rounding. A model
// that edits the file it was pointed at makes the symptom worse and leaves
// the defect. This is what `search` and `list_files` are for, and it is the
// standard band's discriminator.

const INVOICE: &str = r#"import { money } from './money.js'

export function invoiceTotal(lines) {
  let sum = 0
  for (const line of lines) sum += money(line.unitPrice * line.quantity)
  return money(sum)
}
"#;

const MONEY: &str = r#"// Every monetary value in the system passes through here.
export function money(n) {
  return Math.trunc(n * 100) / 100
}
"#;

const INVOICE_TEST: &str = r#"import { invoiceTotal } from './invoice.js'
import { test, assert } from './harness.js'

test('rounds each line to the nearest cent, not down', () => {
  assert.equal(invoiceTotal([{ unitPrice: 0.335, quantity: 1 }]), 0.34)
})
test('a three-line invoice totals correctly', () => {
  assert.equal(invoiceTotal([{ unitPrice: 1.005, quantity: 1 }, { unitPrice: 2.005, quantity: 1 }, { unitPrice: 0.005, quantity: 1 }]), 3.02)
})
"#;

fn invoice_oracle(files: &[WorkbenchFile]) -> Option<String> {
    let money = file_at(files, "src/money.js");
    let invoice = file_at(files, "src/invoice.js");
    if money.trim().is_empty() {
        return Some("src/money.js is empty or missing".into());
    }
    if money.contains("Math.trunc") {
        return Some(
            "invoiceTotal([{ unitPrice: 0.335, quantity: 1 }]) returned 0.33, expected 0.34 — money() truncates where it should round"
                .into(),
        );
    }
    if !money.contains("Math.round") {
        return Some(
            "invoiceTotal([{ unitPrice: 0.335, quantity: 1 }]) returned 0.33, expected 0.34".into(),
        );
    }
    // Patching the symptom in the caller leaves every other consumer of
    // money() still truncating, which is the failure this task is built
    // around.
    if invoice.contains("Math.round") || invoice.contains("toFixed") {
        return Some(
            "src/invoice.js now rounds for itself — every other caller of money() still truncates, so the defect is still there"
                .into(),
        );
    }
    None
}

// ── Task 3: a fix that must not break the passing tests ──────────────────────
//
// The reported bug is real and the obvious fix breaks a documented behaviour
// two tests already cover. The hard band's question: does this model read the
// rest of the suite before it changes shared code?

const SLUG: &str = r#"export function slug(title) {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
}
"#;

const SLUG_TEST: &str = r#"import { slug } from './slug.js'
import { test, assert } from './harness.js'

test('collapses punctuation into single hyphens', () => {
  assert.equal(slug('Hello, World!'), 'hello-world')
})
test('trims leading and trailing hyphens', () => {
  assert.equal(slug('  spaced  '), 'spaced')
})
// Accented characters must survive as their base letter, not be eaten.
test('folds accents rather than dropping them', () => {
  assert.equal(slug('Café Münster'), 'cafe-munster')
})
"#;

fn slug_normalize() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"normalize\(\s*['"]NF[KD]D['"]\s*\)"#).unwrap())
}

fn slug_trim_kept() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\^-\|-\$|replace\(/\^-").unwrap())
}

fn slug_oracle(files: &[WorkbenchFile]) -> Option<String> {
    let src = file_at(files, "src/slug.js");
    if src.trim().is_empty() {
        return Some("src/slug.js is empty or missing".into());
    }
    // The second two are the TS's own redundancy kept verbatim: a literal
    // `̀-ͯ` in the source contains `u0300` as a substring, so the
    // third test subsumes the second. Deleting it here would be a tidy-up
    // that makes the port harder to diff against its source for no gain.
    let folds =
        slug_normalize().is_match(src) || src.contains("\\u0300-\\u036f") || src.contains("u0300");
    if !folds {
        return Some(
            "slug('Café Münster') returned 'caf-m-nster', expected 'cafe-munster' — accents are being dropped rather than folded"
                .into(),
        );
    }
    // The two behaviours that already worked. A model that widens the
    // character class without re-reading the suite loses one of them.
    if !slug_trim_kept().is_match(src) {
        return Some(
            "slug('  spaced  ') returned '-spaced-', expected 'spaced' — the leading/trailing hyphen trim was lost"
                .into(),
        );
    }
    if !src.contains('+') {
        return Some(
            "slug('Hello, World!') returned 'hello--world', expected 'hello-world' — punctuation runs are no longer collapsed"
                .into(),
        );
    }
    None
}

// ── The task table ───────────────────────────────────────────────────────────

/// A test harness stub, so the repo reads like a real one and `list_files`
/// shows something other than the two files under test.
const HARNESS_JS: &str = r#"// Minimal test harness: test(name, fn) and assert.equal / assert.deepEqual.
export const test = (name, fn) => fn()
export const assert = {
  equal: (a, b) => { if (a !== b) throw new Error(`expected ${b}, got ${a}`) },
  deepEqual: (a, b) => { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`) },
}
"#;

const README: &str = r#"# widgets

A small utility package. Run the suite with `npm test`.
Every monetary value passes through `money()` in src/money.js.
"#;

struct Task {
    name: &'static str,
    band: EvalBand,
    input: WorkbenchTaskInput,
    oracle: Oracle,
    /// The file a correct fix has to change. Unread by the fixtures — the
    /// oracle's verdict subsumes it — but kept because the CONSISTENCY TEST
    /// leans on it: the pristine file must fail the oracle and a correct fix
    /// to exactly this path must pass it, which is what makes the field true
    /// rather than decorative.
    fix_in: &'static str,
}

/// The workspace half of a workbench dry run, crossed ahead of the executor
/// that reads it (see the header): the pristine files the run starts from,
/// the oracle that scores what the model leaves behind, and the file a
/// correct fix lands in — pristine must FAIL that oracle and a fix to
/// exactly that path must PASS it, which is the consistency the fixtures
/// lean on. The fitness plane wires this into the sandbox when it crosses.
pub struct WorkbenchWorkspace {
    pub name: &'static str,
    pub fix_in: &'static str,
    pub files: Vec<WorkbenchFile>,
    pub oracle: Oracle,
}

pub fn workspaces() -> Vec<WorkbenchWorkspace> {
    tasks()
        .into_iter()
        .map(|t| WorkbenchWorkspace {
            name: t.name,
            fix_in: t.fix_in,
            files: t.input.files,
            oracle: t.oracle,
        })
        .collect()
}

fn files(entries: &[(&str, &str)]) -> Vec<WorkbenchFile> {
    entries
        .iter()
        .map(|(path, content)| WorkbenchFile {
            path: (*path).into(),
            content: (*content).into(),
        })
        .collect()
}

fn tasks() -> Vec<Task> {
    vec![
        Task {
            name: "fixes an off-by-one the test spells out",
            band: EvalBand::Easy,
            fix_in: "src/paginate.js",
            oracle: paginate_oracle,
            input: WorkbenchTaskInput {
                task: "The pagination helper returns the wrong range for page 1 — the first page skips the first 20 rows. Fix it and make the suite green.".into(),
                failure: "src/paginate.test.js › page 1 starts at the beginning\n  expected {\"start\":0,\"end\":20}, got {\"start\":20,\"end\":40}".into(),
                files: files(&[
                    ("README.md", README),
                    ("src/harness.js", HARNESS_JS),
                    ("src/paginate.js", PAGINATE),
                    ("src/paginate.test.js", PAGINATE_TEST),
                ]),
            },
        },
        Task {
            name: "fixes the defect, not the file it was pointed at",
            band: EvalBand::Standard,
            fix_in: "src/money.js",
            oracle: invoice_oracle,
            input: WorkbenchTaskInput {
                task: "Invoice totals come out a cent low on some lines. The failing test is in src/invoice.test.js. Fix it and make the suite green.".into(),
                failure: "src/invoice.test.js › rounds each line to the nearest cent, not down\n  expected 0.34, got 0.33".into(),
                files: files(&[
                    ("README.md", README),
                    ("src/harness.js", HARNESS_JS),
                    ("src/invoice.js", INVOICE),
                    ("src/money.js", MONEY),
                    ("src/invoice.test.js", INVOICE_TEST),
                ]),
            },
        },
        Task {
            name: "fixes the reported bug without breaking what already worked",
            band: EvalBand::Hard,
            fix_in: "src/slug.js",
            oracle: slug_oracle,
            input: WorkbenchTaskInput {
                task: "Slugs drop accented characters instead of folding them — \"Café Münster\" becomes \"caf-m-nster\". Fix it and make the suite green.".into(),
                failure: "src/slug.test.js › folds accents rather than dropping them\n  expected cafe-munster, got caf-m-nster".into(),
                files: files(&[
                    ("README.md", README),
                    ("src/harness.js", HARNESS_JS),
                    ("src/slug.js", SLUG),
                    ("src/slug.test.js", SLUG_TEST),
                ]),
            },
        },
    ]
}

// ── The prompt ───────────────────────────────────────────────────────────────

const SYSTEM_LINES: [&str; 5] = [
    "You are working in a checked-out repository through file tools. This is real work, not a question: nothing you say changes the code, only what you write with `write_file` does.",
    "Work like a developer at a desk: look at the tree, read the code AND the test that is failing, make the change, run the tests, read the result, keep going until they pass.",
    "Read a file before you replace it — `write_file` overwrites the whole file, so an edit written from memory silently deletes whatever you did not remember.",
    "Fix the DEFECT, not the symptom: if the failing test points at one file and the cause is in another, change the cause.",
    "When the suite is green, reply with one short line saying what was wrong and what you changed.",
];

// ── The three checks, one per thing a coding run can get wrong ───────────────

/// Every tool this sandbox offers, so the gap check below can tell a model
/// that NARRATED a call from one that never made one.
const WORKBENCH_TOOLS: [&str; 5] = [
    "list_files",
    "read_file",
    "search",
    "write_file",
    "run_tests",
];

/// A FOREIGN CALL SYNTAX, which is the same failure as our own narration
/// wearing a different coat. One model emitted
/// `call:file_control:list_files{path: "."}` — its own invented format, on
/// turn one, imitating nothing of ours. The loop cannot parse it, so the call
/// never happened as far as the sandbox is concerned.
///
/// That is still OUR gap and not the model's failure: the model tried to use
/// the tool channel and this build could not receive it. Detecting it needs
/// only the tool NAME plus a bracket nearby — the tool name, then at most a
/// few closing/whitespace characters, then an OPENING bracket, which is what
/// every call syntax has and what prose about a tool does not. It catches the
/// invented `list_files{path: "."}` and the older
/// `(called write_file)\n{"path": …}` alike, and leaves "you should
/// run_tests after fixing this" as the plain failure it is.
fn named_calls(value: &str) -> Vec<&'static str> {
    static PATTERNS: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        WORKBENCH_TOOLS
            .iter()
            .map(|t| {
                (
                    *t,
                    // The tool names are plain identifiers; nothing to escape.
                    Regex::new(&format!(r#"{}[\s)\]"']{{0,4}}[({{\[]"#, t)).unwrap(),
                )
            })
            .collect()
    });
    patterns
        .iter()
        .filter(|(_, re)| re.is_match(value))
        .map(|(t, _)| *t)
        .collect()
}

/// DID THE RUN ACTUALLY GET A TOOL LOOP, before any assertion about what the
/// model did with one.
///
/// THREE OUTCOMES, and the middle one is the whole point:
///
///    Pass   calls reached the sandbox; ask the real question.
///    Gap    no call reached us, but the reply NAMES a tool we offered — so
///           the model tried and the loop did not receive it. That is our
///           defect, and it has happened twice: `[tool] write_file({...})`
///           and then `(called write_file)` were both written into the
///           assistant's own prose, and models reproduced whichever they were
///           shown instead of emitting a structured call. 34 replies in one
///           sweep came back containing our narration verbatim, and every one
///           was scored as a model that "read the repository and never wrote a
///           file".
///    Fail   no calls and no mention of one: it answered a coding task in
///           prose, which is a real failure and changes nothing in the
///           repository.
///
/// The gap branch cannot mask a genuine failure — a model that never intended
/// to call a tool does not name one — and it stops us billing a model for a
/// channel we did not give it. A BARE MENTION IS NOT AN ATTEMPTED CALL: the
/// first version matched `value.includes(toolName)`, which turned "you should
/// run_tests after fixing this" — a model explaining rather than acting, a
/// real failure — into a gap. A gap branch that launders real failures is
/// worse than no gap branch, so the syntax check above is the only one: a
/// call has a bracket after it.
fn used_tools(value: &str, ctx: &CheckCtx) -> CheckResult {
    if !ctx.calls.is_empty() {
        return CheckResult::Pass;
    }
    let foreign = named_calls(value);
    if !foreign.is_empty() {
        return CheckResult::Gap(format!(
            "the reply calls {} in a syntax this build does not parse — the call never reached the sandbox, so this run cannot be scored",
            foreign.join(", ")
        ));
    }
    CheckResult::Fail(
        "called no tool at all — it answered a coding task in prose, which changes nothing".into(),
    )
}

/// FIXED: the bug is gone, judged by the task's own oracle against the files
/// as the model left them (`ctx.failure` — the world half of the dry run,
/// modeled on `CheckCtx`). Not "the diff matched": a real fix can be made in
/// more than one place.
fn check_suite_green(value: &str, ctx: &CheckCtx) -> CheckResult {
    let gate = used_tools(value, ctx);
    if gate != CheckResult::Pass {
        return gate;
    }
    if ctx.successful("write_file").is_empty() {
        return CheckResult::Fail(
            "read the repository and never wrote a file, so nothing was fixed".into(),
        );
    }
    match ctx.failure() {
        Some(failure) => CheckResult::Fail(format!("the suite is still red: {failure}")),
        None => CheckResult::Pass,
    }
}

/// CAREFUL: it read a file before replacing it. `write_file` overwrites the
/// whole file, so an edit written from memory silently deletes whatever the
/// model did not remember — on a real repository that is the expensive
/// mistake, and it is invisible in a green suite.
fn check_read_before_replace(value: &str, ctx: &CheckCtx) -> CheckResult {
    let gate = used_tools(value, ctx);
    if gate != CheckResult::Pass {
        return gate;
    }
    let wrote: Vec<(usize, &crate::harness::define::CheckCall)> = ctx
        .calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.tool == "write_file" && !c.errored)
        .collect();
    if wrote.is_empty() {
        return CheckResult::Fail(
            "read the repository and never wrote a file, so nothing was fixed".into(),
        );
    }
    let mut blind: Vec<String> = Vec::new();
    for (at, w) in wrote {
        // `String(w.args.path ?? '')`: absent and null are both the empty
        // string, anything else is JS `String()` of it.
        let path = match w.args.get("path") {
            None | Some(Value::Null) => String::new(),
            Some(v) => js_string(v),
        };
        // A `search` hit counts as having seen the file: it returns the
        // matching lines, which is how a developer actually locates a
        // one-line fix. The read must name the SAME path — strict equality
        // in the TS, so a string is a string and anything else is not.
        let seen = ctx.calls.iter().position(|c| {
            (c.tool == "read_file" && c.args.get("path") == Some(&Value::String(path.clone())))
                || c.tool == "search"
        });
        // By position rather than the TS's identity `indexOf`, which on two
        // identical write calls would hand the second one the first's index;
        // the question is whether a read preceded THIS write.
        if seen.is_none_or(|s| s > at) {
            blind.push(path);
        }
    }
    if blind.is_empty() {
        CheckResult::Pass
    } else {
        let quoted = blind
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        CheckResult::Fail(format!(
            "replaced {quoted} without reading it first — write_file overwrites the whole file"
        ))
    }
}

/// VERIFIED: it ran the tests. A model that fixes the bug and never checks
/// has done the work and cannot know it, which on a real ticket reads as
/// success it never confirmed.
fn check_verified(value: &str, ctx: &CheckCtx) -> CheckResult {
    let gate = used_tools(value, ctx);
    if gate != CheckResult::Pass {
        return gate;
    }
    if ctx.successful("write_file").is_empty() {
        return CheckResult::Fail(
            "read the repository and never wrote a file, so nothing was fixed".into(),
        );
    }
    // A RUN WE CUT SHORT CANNOT BE ASKED ABOUT ITS ORDER. `exhausted` means
    // the model was still working when the turn budget ran out, so "did it
    // verify AFTER its last edit" has no answer — the sequence it would have
    // finished with never happened. Scoring it said the model verified a
    // state it then changed, which is a description of something that did not
    // occur. This is our budget, not its mistake, and it goes in the gap list
    // where someone can decide whether the ceiling is enough.
    if ctx.exhausted {
        return CheckResult::Gap(
            "the turn budget ran out while the model was still working, so \"did it re-run the tests after its last edit\" cannot be asked of this run"
                .into(),
        );
    }
    if !ctx.any_call("run_tests") {
        return CheckResult::Fail(
            "never ran the tests, so it reported work it had no way to verify".into(),
        );
    }
    // Running them BEFORE the last edit and not after is the same failure
    // wearing a green tick: the model verified a state it then changed. Both
    // positions count errored calls too — the TS's `lastIndexOf` never
    // filtered on `error`, and a refused write is still a write that
    // happened.
    let last_write = ctx.calls.iter().rposition(|c| c.tool == "write_file");
    let last_run = ctx.calls.iter().rposition(|c| c.tool == "run_tests");
    match (last_run, last_write) {
        (Some(run), Some(write)) if run > write => CheckResult::Pass,
        _ => CheckResult::Fail(
            "ran the tests and then edited again without re-running them, so the last change was never verified"
                .into(),
        ),
    }
}

// ── Eval fixtures ────────────────────────────────────────────────────────────

fn input_json(input: &WorkbenchTaskInput) -> Value {
    serde_json::to_value(input).unwrap()
}

/// One fixture: the reply, the calls and the world a dry run left behind,
/// judged by agreement with the label.
pub struct WorkbenchFixture {
    pub name: String,
    pub band: EvalBand,
    pub input: Value,
    pub check: fn(&str, &CheckCtx) -> CheckResult,
}

/// NINE FIXTURES — three tasks × the three things a coding run can get wrong.
/// They are not one assertion split for the sake of a count — they fail
/// independently and mean different things, and a model can do any two
/// without the third. All three fail when nothing was called at all, which is
/// what keeps them out of the sweep's garbage census.
pub fn fixtures() -> Vec<WorkbenchFixture> {
    let mut out = Vec::new();
    for task in tasks() {
        let input = input_json(&task.input);
        out.push(WorkbenchFixture {
            name: format!("{} — the suite goes green", task.name),
            band: task.band,
            input: input.clone(),
            check: check_suite_green,
        });
        out.push(WorkbenchFixture {
            name: format!("{} — reads a file before replacing it", task.name),
            band: task.band,
            input: input.clone(),
            check: check_read_before_replace,
        });
        out.push(WorkbenchFixture {
            name: format!("{} — runs the tests before it calls it done", task.name),
            band: task.band,
            input,
            check: check_verified,
        });
    }
    out
}

// ── The three definitions ────────────────────────────────────────────────────

/// Three definitions over one builder. Same tasks, same assertions, three
/// `ModelSpec`s — which is what binds the three Workbench columns.
fn workbench_harness(
    role: &'static str,
    id: &'static str,
    label: &'static str,
    effort: &'static str,
) -> HarnessDefinition {
    let mut d = define_harness(HarnessDefinition::new(
        id,
        label,
        // The effort is interpolated once, into the one sentence that names
        // the job; the tasks and assertions are byte-identical across the
        // three, which is what makes the columns comparable.
        Box::leak(format!("Drives a coding harness at {effort} effort: reads the repository, edits files, and runs the tests until they pass.").into_boxed_str()),
        // THE BINDING. The scorer runs the real resolver over this spec, so
        // naming the role here is what puts a verdict in the Workbench column
        // an admin is actually assigning. One harness declaring
        // `code-standard` would bind one column and leave the other two
        // exactly as empty as they were.
        ModelSpec {
            pin: None,
            role: Some(role),
            chain: None,
            user_id: None,
        },
        // The prompt a coding harness gets: the task, the failure, and the
        // standing instruction to work through the tools rather than to
        // answer in prose. THE FILES ARE NOT IN THE PROMPT — they reach the
        // run as the dry run's workspace (the task's own files, the task's
        // own oracle), because a model that is handed the tree in prose
        // answers in prose, which is the failure this harness exists to
        // catch. No trust clause: nothing here is a stranger's text yet —
        // what the model reads back comes through `read_file` from its own
        // repository.
        Arc::new(|input: &Value, _ctx: &RenderContext| {
            let wi: WorkbenchTaskInput =
                serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            Ok(vec![
                Message::system(SYSTEM_LINES.join("\n")),
                Message::user(format!(
                    "Task: {}\n\nThe test runner reports:\n{}",
                    wi.task, wi.failure
                )),
            ])
        }),
        Output::Text {
            // `raw.trim() || null` — the empty reply is a failure the caller
            // already knows how to hold: a workbench run that produced
            // nothing leaves the branch untouched, which is the safe end
            // state and the one the session loop already handles.
            clean: Some(Arc::new(|raw: &str| {
                Ok((!raw.trim().is_empty()).then(|| Value::String(raw.trim().to_string())))
            })),
            verify: None,
        },
        OnFailure::Null,
    ));
    // The role's own declaration, restated where the fitness matrix reads it.
    // `tools` is not a quality bar here — without it the run does nothing
    // while reporting that it worked.
    d.requires = vec!["code", "tools"];
    // NOTHING REFUSES. A weaker coder makes a worse change and a human
    // reviews every one of them before it merges — the Workbench's whole
    // lifecycle is platform-owned for exactly that reason. Refusing would
    // take the Workbench away from every self-host whose model nobody has
    // probed.
    d.floor = RoleFloor::runs_anyway(
        "A weaker model makes smaller, clumsier changes and leans harder on review; it never merges anything by itself, because branches and PRs are the platform’s to drive.",
    );
    // Credentials only, and it matters here: a repository is one of the
    // likelier places for a key to be sitting, and a model that quotes one
    // back into its summary would put it in the run's own record.
    // `zero_tool_claim` is deliberately absent — the fixtures carry that
    // question with the calls record, which knows far more than a guard on
    // the reply's prose could.
    d.guard = Some(GuardDecl {
        rules: Some(vec!["secret_leak", "pii_leak"]),
        redact: true,
    });
    // THE TOOL LOOP IS THE JOB. Declared for the same reason work-session
    // declares it — a coding harness that cannot call tools cannot read a
    // file. No hold: there is no sweep budgeting this turn, and no caller
    // waiting on a clock.
    d.tools = Some(ToolPolicy::Own);
    // THE DRY RUN IS THE WORKSPACE, and the workspace is the TASK: the
    // fixture's own files and the task's own oracle, which is why it is built
    // from the INPUT rather than held as a constant — the repository and the
    // oracle that decides whether its tests pass are properties of the case,
    // not of the def. TWELVE TURNS, and the number's evidence is recorded in
    // this file's header: six filed nine of one model's cases as OUR gap, ten
    // still filed three across a twelve-model archive (p50 is 6 tool calls,
    // p90 is 8, the tail reaches 13), and twelve is the turn ceiling — the last
    // raise available without a cost decision, and if twelve still gaps the
    // honest next move is a smaller task, not a bigger budget. No `tools` list
    // beside it: a def that declares the workspace has the coding surface, and
    // the five sandbox tools come with the surface rather than with the
    // declaration.
    d.dry_run = Some(DryRunDecl {
        tools: Vec::new(),
        max_turns: Some(12),
        world: None,
        credentials: None,
        workspace: Some(Arc::new(|input: &Value| {
            // `input.files`, and the oracle found by
            // `TASKS.find(t => t.input.task === input.task)?.oracle` — matched
            // on the task string because the oracle is the task's own. A find
            // that misses reads as the TS's `?? null` did: no oracle, so no
            // failure the world can ever report. A value that is not one of
            // this table's inputs cannot arrive from the fixture fold (each
            // case's input IS a task's) and reads the same way here.
            let task: WorkbenchTaskInput = serde_json::from_value(input.clone()).unwrap_or(
                WorkbenchTaskInput {
                    task: String::new(),
                    files: Vec::new(),
                    failure: String::new(),
                },
            );
            let oracle = tasks()
                .into_iter()
                .find(|t| t.input.task == task.task)
                .map(|t| t.oracle);
            WorkspaceSpec {
                files: task
                    .files
                    .into_iter()
                    .map(|f| WorkspaceFile {
                        path: f.path,
                        content: f.content,
                    })
                    .collect(),
                // The oracle is declared over `WorkbenchFile`, this file's own
                // table type; the fitness surface hands the sandbox
                // `WorkspaceFile`. The same two fields, converted at the
                // boundary rather than re-derived.
                passes: Arc::new(move |files: &[WorkspaceFile]| {
                    let Some(oracle) = oracle else {
                        return None;
                    };
                    let files: Vec<WorkbenchFile> = files
                        .iter()
                        .map(|f| WorkbenchFile {
                            path: f.path.clone(),
                            content: f.content.clone(),
                        })
                        .collect();
                    oracle(&files)
                }),
            }
        })),
    });
    // THE FIXTURE TABLE — the same nine fixtures on all three defs, exactly as
    // the TS attached `TASKS.flatMap(fixturesFor)` to each of the three: the
    // efforts differ in what an admin SPENDS, not in what the job is, and a
    // suite that drifted between the columns would make them incomparable.
    // The names are built from the task table's own names, so they are leaked
    // into 'static — the same leak the job line above takes, for the same
    // reason: a definition is built once and lives for the process. The fold
    // re-types the value the way every text def's does (the run's value is the
    // trimmed reply, what the TS check received), and passes the CONTEXT
    // through untouched — the calls, the world's failure and the exhausted
    // flag are the whole assertion here.
    d.evals = fixtures()
        .into_iter()
        .map(|f| {
            let WorkbenchFixture {
                name,
                band,
                input,
                check,
            } = f;
            EvalCase::new(
                Box::leak(name.into_boxed_str()),
                input,
                Arc::new(move |v: &Value, ctx: &CheckCtx| match serde_json::from_value::<String>(v.clone()) {
                    Ok(reply) => check(&reply, ctx),
                    Err(e) => CheckResult::Fail(format!("the fixture check threw on the value: {e}")),
                }),
            )
            .band(band)
        })
        .collect();
    d
}

pub fn workbench_light_harness() -> HarnessDefinition {
    workbench_harness(
        "code-light",
        "workbench:light",
        "Workbench — light effort",
        "light",
    )
}

pub fn workbench_standard_harness() -> HarnessDefinition {
    workbench_harness(
        "code-standard",
        "workbench:standard",
        "Workbench — standard effort",
        "standard",
    )
}

pub fn workbench_heavy_harness() -> HarnessDefinition {
    workbench_harness(
        "code-heavy",
        "workbench:heavy",
        "Workbench — heavy effort",
        "heavy",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::define::{CheckCall, is_gap};
    use crate::harness::recorded::{
        RecordedRun as Recorder, RecordedWorld as World, recorded_run, replies,
    };
    use crate::harness::run::{HarnessResult, RunContext, execute};
    use serde_json::json;

    fn call(tool: &str, args: Value) -> CheckCall {
        CheckCall {
            tool: tool.into(),
            errored: false,
            args,
        }
    }

    /// A context standing in for a completed dry run. `failure` is what the
    /// fixture's oracle said about the files as the model left them.
    fn ctx(calls: Vec<CheckCall>, failure: Option<&str>) -> CheckCtx {
        CheckCtx {
            calls,
            world: failure.map(|f| serde_json::json!({ "failure": f })),
            exhausted: false,
        }
    }

    /// Run a task's own workspace through its oracle with some files
    /// replaced — which is how a real fix reaches it. Reads the pub
    /// `workspaces()` the fitness plane will consume, so what is asserted
    /// here is the same table the dry run will score against.
    fn oracle_for(task_name: &str) -> impl Fn(&[(&str, &str)]) -> Option<String> + '_ {
        let task = workspaces()
            .into_iter()
            .find(|t| t.name == task_name)
            .unwrap_or_else(|| panic!("no task called {task_name}"));
        move |patch: &[(&str, &str)]| {
            let files: Vec<WorkbenchFile> = task
                .files
                .iter()
                .map(|f| WorkbenchFile {
                    path: f.path.clone(),
                    content: patch
                        .iter()
                        .find(|(p, _)| *p == f.path)
                        .map(|(_, c)| (*c).into())
                        .unwrap_or_else(|| f.content.clone()),
                })
                .collect();
            (task.oracle)(&files)
        }
    }

    // ── The three bindings ───────────────────────────────────────────────────

    #[test]
    fn the_three_efforts_bind_the_three_roles() {
        // Naming the role is the whole mechanism: the scorer runs the real
        // resolver over `model`, so this is what fills the three columns.
        let light = workbench_light_harness();
        let standard = workbench_standard_harness();
        let heavy = workbench_heavy_harness();
        assert_eq!(light.model.role, Some("code-light"));
        assert_eq!(standard.model.role, Some("code-standard"));
        assert_eq!(heavy.model.role, Some("code-heavy"));
        assert_eq!(light.id, "workbench:light");
        assert_eq!(standard.id, "workbench:standard");
        assert_eq!(heavy.id, "workbench:heavy");
    }

    #[test]
    fn nine_fixtures_three_per_task_across_the_bands() {
        // The suite is built from the ONE shared task table, three fixtures
        // per task — which is what makes the three Workbench columns
        // comparable, and what this pins so a future parameterization of one
        // effort cannot drift the suite silently.
        let fx = fixtures();
        assert_eq!(fx.len(), 9);
        for band in [EvalBand::Easy, EvalBand::Standard, EvalBand::Hard] {
            assert_eq!(fx.iter().filter(|f| f.band == band).count(), 3);
        }
        for task in tasks() {
            assert!(fx.iter().any(|f| f.name.starts_with(task.name)));
        }
    }

    #[test]
    fn refuse_nothing_a_weaker_coder_is_reviewed_not_blocked() {
        for d in [
            workbench_light_harness(),
            workbench_standard_harness(),
            workbench_heavy_harness(),
        ] {
            assert!(!d.floor.refuse_below, "{}", d.id);
            assert!(d.model.role.is_some());
        }
    }

    // ── The task oracles ─────────────────────────────────────────────────────
    //
    // What is asserted here is the half a sweep cannot: that the ORACLES are
    // right. An oracle that accepts a broken repository would credit every
    // model with a fix; one that rejects a real fix would fail every model.

    #[test]
    fn paginate_rejects_the_bug_and_accepts_any_one_based_fix() {
        let oracle = oracle_for("fixes an off-by-one the test spells out");
        assert!(oracle(&[]).unwrap().contains("one-based"));
        // Two spellings of the same fix, because a benchmark that demands one
        // exact diff measures obedience rather than capability.
        let a = "export function pageRange(page, perPage, total) {\n  const start = (page - 1) * perPage\n  const end = Math.min(start + perPage, total)\n  return { start, end }\n}\n";
        let b = "export function pageRange(page, perPage, total) {\n  const start = perPage * (page - 1)\n  const end = Math.min(start + perPage, total)\n  return { start, end }\n}\n";
        assert_eq!(oracle(&[("src/paginate.js", a)]), None);
        assert_eq!(oracle(&[("src/paginate.js", b)]), None);
        // And it still checks the behaviour that already worked.
        let no_clamp = "export function pageRange(page, perPage, total) {\n  const start = (page - 1) * perPage\n  return { start, end: start + perPage }\n}\n";
        assert!(
            oracle(&[("src/paginate.js", no_clamp)])
                .unwrap()
                .contains("clamped")
        );
    }

    #[test]
    fn invoice_accepts_the_fix_at_the_cause_and_rejects_it_at_the_symptom() {
        // The whole point of this task. Patching the caller leaves every other
        // consumer of money() still truncating, so the defect is still there.
        let oracle = oracle_for("fixes the defect, not the file it was pointed at");
        assert!(oracle(&[]).unwrap().contains("truncates"));

        let fixed_money = "// Every monetary value in the system passes through here.\nexport function money(n) {\n  return Math.round(n * 100) / 100\n}\n";
        assert_eq!(oracle(&[("src/money.js", fixed_money)]), None);

        let patched_caller = "import { money } from './money.js'\n\nexport function invoiceTotal(lines) {\n  let sum = 0\n  for (const line of lines) sum += Math.round(line.unitPrice * line.quantity * 100) / 100\n  return sum\n}\n";
        assert!(
            oracle(&[("src/invoice.js", patched_caller)])
                .unwrap()
                .contains("truncates")
        );
        // Even WITH money() fixed, rounding in the caller as well is flagged:
        // it is the shape of a symptom patch and the next reader has to
        // disprove it.
        assert!(
            oracle(&[
                ("src/money.js", fixed_money),
                ("src/invoice.js", patched_caller)
            ])
            .unwrap()
            .contains("every other caller")
        );
    }

    #[test]
    fn slug_accepts_an_accent_fold_and_rejects_one_that_breaks_a_passing_test() {
        let oracle = oracle_for("fixes the reported bug without breaking what already worked");
        assert!(oracle(&[]).unwrap().contains("accents"));

        let good = "export function slug(title) {\n  return title\n    .normalize('NFKD')\n    .replace(/[\\u0300-\\u036f]/g, '')\n    .toLowerCase()\n    .replace(/[^a-z0-9]+/g, '-')\n    .replace(/^-|-$/g, '')\n}\n";
        assert_eq!(oracle(&[("src/slug.js", good)]), None);

        // Folds the accents and drops the trim — the regression this task
        // exists for.
        let lost_trim = "export function slug(title) {\n  return title\n    .normalize('NFKD')\n    .replace(/[\\u0300-\\u036f]/g, '')\n    .toLowerCase()\n    .replace(/[^a-z0-9]+/g, '-')\n}\n";
        assert!(
            oracle(&[("src/slug.js", lost_trim)])
                .unwrap()
                .contains("trim")
        );
    }

    #[test]
    fn an_empty_file_is_a_failure_not_a_pass() {
        // `write_file` replaces a whole file; a model that writes nothing
        // must not score a green suite for having deleted the defect along
        // with the code. `fix_in` is what makes each row's target explicit.
        for (name, fix_in) in [
            ("fixes an off-by-one the test spells out", "src/paginate.js"),
            (
                "fixes the defect, not the file it was pointed at",
                "src/money.js",
            ),
            (
                "fixes the reported bug without breaking what already worked",
                "src/slug.js",
            ),
        ] {
            let verdict = oracle_for(name)(&[(fix_in, "")]).unwrap();
            assert!(verdict.contains("empty or missing"), "{name}: {verdict}");
        }
    }

    // ── The fixtures ─────────────────────────────────────────────────────────

    fn by(name: &str) -> WorkbenchFixture {
        fixtures()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no workbench fixture called \"{name}\""))
    }

    fn check_of(name: &str) -> fn(&str, &CheckCtx) -> CheckResult {
        by(name).check
    }

    #[test]
    fn all_pass_a_run_that_read_fixed_and_verified() {
        let clean = vec![
            call("list_files", json!({})),
            call("read_file", json!({ "path": "src/paginate.js" })),
            call("write_file", json!({ "path": "src/paginate.js" })),
            call("run_tests", json!({})),
        ];
        let ctx = ctx(clean, None);
        for name in [
            "fixes an off-by-one the test spells out — the suite goes green",
            "fixes an off-by-one the test spells out — reads a file before replacing it",
            "fixes an off-by-one the test spells out — runs the tests before it calls it done",
        ] {
            assert_eq!(
                check_of(name)("fixed the off-by-one", &ctx),
                CheckResult::Pass,
                "{name}"
            );
        }
    }

    #[test]
    fn all_fail_a_model_that_answered_in_prose() {
        // The garbage census enforces this too; asserting it here as well is
        // cheap and names the reason.
        let ctx = ctx(Vec::new(), None);
        for name in [
            "fixes an off-by-one the test spells out — the suite goes green",
            "fixes an off-by-one the test spells out — reads a file before replacing it",
            "fixes an off-by-one the test spells out — runs the tests before it calls it done",
        ] {
            let verdict = check_of(name)("The bug is that pages are one-based.", &ctx);
            assert!(
                matches!(&verdict, CheckResult::Fail(m) if m.contains("no tool at all")),
                "{name}: {verdict:?}"
            );
        }
    }

    #[test]
    fn the_three_failures_fire_independently() {
        let green = check_of("fixes an off-by-one the test spells out — the suite goes green");
        let careful =
            check_of("fixes an off-by-one the test spells out — reads a file before replacing it");
        let verified = check_of(
            "fixes an off-by-one the test spells out — runs the tests before it calls it done",
        );

        // FIXED but not carefully: replaced a file it never read.
        let blind = vec![
            call("list_files", json!({})),
            call("write_file", json!({ "path": "src/paginate.js" })),
            call("run_tests", json!({})),
        ];
        assert_eq!(green("done", &ctx(blind.clone(), None)), CheckResult::Pass);
        assert!(
            matches!(careful("done", &ctx(blind, None)), CheckResult::Fail(m) if m.contains("without reading it first"))
        );

        // CAREFUL but not fixed: the oracle still reports a failure.
        let clean = vec![
            call("read_file", json!({ "path": "src/paginate.js" })),
            call("write_file", json!({ "path": "src/paginate.js" })),
        ];
        let red = ctx(
            vec![
                call("list_files", json!({})),
                call("read_file", json!({ "path": "src/paginate.js" })),
                call("write_file", json!({ "path": "src/paginate.js" })),
            ],
            Some("pageRange(1, 20, 100) returned { start: 20 }"),
        );
        assert!(matches!(green("done", &red), CheckResult::Fail(m) if m.contains("still red")));
        assert_eq!(careful("done", &red), CheckResult::Pass);

        // FIXED and careful but never verified.
        assert_eq!(green("done", &ctx(clean.clone(), None)), CheckResult::Pass);
        assert!(
            matches!(verified("done", &ctx(clean, None)), CheckResult::Fail(m) if m.contains("never ran the tests"))
        );
    }

    #[test]
    fn catches_a_run_that_verified_and_then_edited_again() {
        // A green tick on a state the model then changed is the same failure
        // as not having checked at all.
        let verified = check_of(
            "fixes an off-by-one the test spells out — runs the tests before it calls it done",
        );
        let stale = vec![
            call("read_file", json!({ "path": "src/paginate.js" })),
            call("write_file", json!({ "path": "src/paginate.js" })),
            call("run_tests", json!({})),
            call("write_file", json!({ "path": "src/paginate.js" })),
        ];
        assert!(
            matches!(verified("done", &ctx(stale, None)), CheckResult::Fail(m) if m.contains("never verified"))
        );
    }

    #[test]
    fn counts_a_search_hit_as_having_seen_the_file() {
        // Locating a one-line fix with `search` and editing from the hit is
        // how a developer actually works; demanding a full read would fail
        // correct runs.
        let careful =
            check_of("fixes an off-by-one the test spells out — reads a file before replacing it");
        let searched = vec![
            call("search", json!({ "query": "pageRange" })),
            call("write_file", json!({ "path": "src/paginate.js" })),
            call("run_tests", json!({})),
        ];
        assert_eq!(careful("done", &ctx(searched, None)), CheckResult::Pass);
    }

    // ── A run that never got its tool loop ───────────────────────────────────

    #[test]
    fn reports_our_gap_when_the_model_named_a_tool_we_never_received_a_call_for() {
        // THE FAILURE MODE THIS EXISTS FOR, twice over. `[tool]
        // write_file({...})` and then `(called write_file)` were both written
        // into the assistant's own prose because the message shape had no
        // tool channel, and models reproduced whichever they were shown
        // instead of emitting a structured call. 34 replies in one sweep
        // contained our narration verbatim — every one scored as a model that
        // "read the repository and never wrote a file". A reply that NAMES a
        // tool we offered, with no call recorded, is our defect.
        let first = check_of("fixes an off-by-one the test spells out — the suite goes green");
        let verdict = first(
            "Fixing it now.\n(called write_file)\n{\"path\":\"src/paginate.js\"}",
            &ctx(Vec::new(), None),
        );
        let gap = is_gap(&verdict).expect("a gap");
        assert!(gap.contains("a syntax this build does not parse"), "{gap}");
    }

    #[test]
    fn reports_a_foreign_call_syntax_as_our_gap() {
        // A model's own invented format — `call:file_control:list_files{path:
        // "."}` — imitating nothing of ours. The loop cannot parse it, so the
        // call never happened as far as the sandbox knows. The model tried to
        // use the tool channel and this build could not receive it.
        let first = check_of("fixes an off-by-one the test spells out — the suite goes green");
        let verdict = first(
            "I will investigate.\ncall:file_control:list_files{path: \".\"}",
            &ctx(Vec::new(), None),
        );
        assert!(is_gap(&verdict).is_some(), "{verdict:?}");
    }

    #[test]
    fn does_not_mistake_prose_about_a_tool_for_a_call() {
        // A model explaining that it would run the tests has not called
        // anything, and that is a real failure — the gap branch must not
        // launder it. A call has a bracket after it.
        let first = check_of("fixes an off-by-one the test spells out — the suite goes green");
        let verdict = first(
            "You should run_tests after fixing the off-by-one in paginate.",
            &ctx(Vec::new(), None),
        );
        assert!(matches!(verdict, CheckResult::Fail(_)), "{verdict:?}");
    }

    #[test]
    fn reports_an_exhausted_run_as_our_budget_not_as_bad_sequencing() {
        // "Did it re-run the tests after its last edit" has no answer for a
        // run that was still working when the turn budget ended — the
        // sequence it would have finished with never happened.
        let verified = check_of(
            "fixes an off-by-one the test spells out — runs the tests before it calls it done",
        );
        let calls = vec![
            call("write_file", json!({})),
            call("run_tests", json!({})),
            call("write_file", json!({})),
        ];
        let mut world = ctx(calls.clone(), None);
        world.exhausted = true;
        let verdict = verified("Still working on it.", &world);
        let gap = is_gap(&verdict).expect("a gap");
        assert!(gap.contains("turn budget ran out"), "{gap}");
        // The same call order on a run that FINISHED is a real failure.
        assert!(
            matches!(verified("Done.", &ctx(calls, None)), CheckResult::Fail(m) if m.contains("without re-running them"))
        );
    }

    // ── The def, driven through the runner against a recorded world ──────────

    async fn run(
        def: &HarnessDefinition,
        input: &Value,
        r: &Recorder,
    ) -> Result<HarnessResult, crate::harness::run::HarnessError> {
        let ctx = RunContext {
            caller: "test:workbench".into(),
            deps: Some(r.deps()),
            ..Default::default()
        };
        execute(&r.deps(), def, input, ctx, None).await
    }

    #[tokio::test]
    async fn a_turn_is_a_system_brief_and_a_task_with_live_tools() {
        let def = workbench_standard_harness();
        let task = tasks().remove(0);
        let r = recorded_run(World {
            replies: replies(&[
                "Off-by-one in pageRange: switched to (page - 1) * perPage; suite green.",
            ]),
            ..Default::default()
        });
        let res = run(&def, &input_json(&task.input), &r).await.unwrap();
        assert!(res.answered && res.schema_valid, "{:?}", res.error);
        let req = r.req_at(0);
        // The brief, then the task and the runner's report — and no files:
        // the tree reaches the run as the workspace, not as prose.
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role.as_str(), "system");
        assert!(
            req.messages[0]
                .content
                .starts_with("You are working in a checked-out repository through file tools.")
        );
        assert!(
            req.messages[0]
                .content
                .ends_with("reply with one short line saying what was wrong and what you changed.")
        );
        assert_eq!(req.messages[1].role.as_str(), "user");
        assert_eq!(
            req.messages[1].content,
            format!(
                "Task: {}\n\nThe test runner reports:\n{}",
                task.input.task, task.input.failure
            )
        );
        assert!(!req.messages[1].content.contains("export function"));
        // The loop is live; no hold, no temperature — nothing budgets this
        // turn from the harness side.
        assert_eq!(req.tools, Some(ToolPolicy::Own));
        assert_eq!(req.hold_ms, None);
        assert_eq!(req.temperature, None);
    }

    #[tokio::test]
    async fn a_run_that_produced_nothing_lands_on_null() {
        let def = workbench_heavy_harness();
        let r = recorded_run(World {
            replies: replies(&["   "]),
            ..Default::default()
        });
        let task = tasks().remove(1);
        let res = run(&def, &input_json(&task.input), &r).await.unwrap();
        // The caller keeps what it had: no value, and the branch untouched —
        // the safe end state.
        assert_eq!(res.value, None);
        assert!(!res.schema_valid);
        assert!(matches!(def.on_failure, OnFailure::Null));
    }
}
