// THE CODE PROBE'S EXECUTOR — the code task, its extraction and its runner:
// `CodeTask`, `CODE_TASKS`, `extract_code`, `same_value`, `CODE_TIMEOUT_MS`
// and `run_code_task`. It lives in its own module rather than inside probes
// because it is the one place in the whole fitness family that executes a
// program a MODEL wrote, and that deserves a boundary you can name in one
// line.
//
// BOA, IN A FRESH CONTEXT. The candidate runs in a fresh boa context, narrow
// by construction: a new boa context holds the ECMAScript builtins and
// nothing else, so there is no `require`, no `process`, no `fetch`, no timers,
// and no host object is ever constructed for candidate code to reach. The
// boundary is deliberate: arguments cross as a JSON string and results come
// back as a JSON string, and the context holds nothing but a stubbed
// `console`.
//
// THE TIMEOUT LIVES ON A THREAD, because boa has no wall-clock kill. The whole
// task — definition plus every assertion — is evaluated inside one spawned
// thread and the caller waits 250ms for its answer, counted from the moment
// the engine is built (construction is our cost; the window times the
// candidate's code). A wrong `while (true) {}` costs its 250ms and no more;
// the leaked thread is reaped shortly after by the runtime limits below, so a
// run cannot shed unbounded threads the way a run without limits could.
//
// THIS IS NOT A SECURITY SANDBOX and must never be described as one. It is here
// because the alternative — grading code by asking another model whether it
// looks right — is not a measurement at all, and the input is a function this
// same admin just asked their own configured model to write.

use std::sync::{LazyLock, mpsc};
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

/// One code task: a precise contract plus the assertions it must satisfy. The
/// assertions ARE the grade — no second model reads the code and forms an
/// opinion about it, which is the whole difference between a probe and a vibe.
pub struct CodeTask {
    pub name: &'static str,
    /// The function the model must define at top level. (`fn` is a keyword.)
    pub fn_: &'static str,
    pub prompt: &'static str,
    /// Each is `[arguments, expected]`. Compared structurally.
    pub cases: Vec<CodeCase>,
}

pub struct CodeCase {
    pub args: Vec<Value>,
    pub expect: Value,
}

pub static CODE_TASKS: LazyLock<Vec<CodeTask>> = LazyLock::new(|| {
    vec![
        CodeTask {
            name: "slugify",
            fn_: "slugify",
            prompt: "Write a JavaScript function `slugify(input)` and nothing else. It lowercases the input, replaces every run of \
                    characters that are not a-z or 0-9 with a single \"-\", and removes any leading or trailing \"-\". \
                    Reply with the function source only.",
            cases: vec![
                CodeCase {
                    args: vec![Value::from("Hello, World!")],
                    expect: Value::from("hello-world"),
                },
                CodeCase {
                    args: vec![Value::from("  A  B  ")],
                    expect: Value::from("a-b"),
                },
                CodeCase {
                    args: vec![Value::from("Talaria Harness 2.0")],
                    expect: Value::from("talaria-harness-2-0"),
                },
                CodeCase {
                    args: vec![Value::from("")],
                    expect: Value::from(""),
                },
                CodeCase {
                    args: vec![Value::from("---already---slugged---")],
                    expect: Value::from("already-slugged"),
                },
            ],
        },
        CodeTask {
            name: "mergeRanges",
            fn_: "mergeRanges",
            prompt: "Write a JavaScript function `mergeRanges(ranges)` and nothing else. `ranges` is an array of [start, end] number \
                    pairs. It returns a new array of merged, non-overlapping pairs sorted by start; ranges that touch at an endpoint \
                    (for example [1,2] and [2,3]) merge into one. It must not modify the input. Reply with the function source only.",
            cases: vec![
                CodeCase {
                    args: vec![serde_json::json!([[1, 3], [2, 6], [8, 10]])],
                    expect: serde_json::json!([[1, 6], [8, 10]]),
                },
                CodeCase {
                    args: vec![serde_json::json!([[8, 10], [1, 3]])],
                    expect: serde_json::json!([[1, 3], [8, 10]]),
                },
                CodeCase {
                    args: vec![serde_json::json!([[1, 2], [2, 3]])],
                    expect: serde_json::json!([[1, 3]]),
                },
                CodeCase {
                    args: vec![serde_json::json!([])],
                    expect: serde_json::json!([]),
                },
            ],
        },
    ]
});

/// The code the model wrote, with the two wrappers it habitually adds removed.
///
/// Fences and a leading `export` are formatting, not contract: a model that
/// produced a correct `slugify` and typed `export` in front of it solved the
/// problem, and failing it for that would be scoring our extractor. Everything
/// else — a class, a default export, an async function — is left alone and
/// fails honestly in the evaluator below.
pub fn extract_code(raw: &str) -> String {
    static FENCED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)```(?:[a-zA-Z]*)\n(.*?)```").expect("a static regex compiles")
    });
    // The keyword tail is a CAPTURE rather than a look-ahead (the regex crate
    // has no look-ahead) — the output bytes are the same either way.
    static EXPORT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^[ \t]*export\s+((?:async\s+)?function\b|const\b|let\b|var\b)")
            .expect("a static regex compiles")
    });
    let body = FENCED
        .captures(raw)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
        .unwrap_or(raw);
    EXPORT.replace_all(body, "$1").into_owned()
}

/// Structural equality over the JSON-shaped values these assertions produce.
/// `JSON.stringify` is not enough (key order, and `1` vs `1.0` round-trips)
/// and a deep-equal library is not worth a dependency for numbers, strings and
/// arrays.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        // serde_json keeps 1 and 1.0 in different variants; the candidate's
        // answers arrive as JS-canonical JSON text, but `expect` was built by
        // hand, so compare numerically rather than by variant.
        (Value::Number(x), Value::Number(y)) => match (x.as_u64(), y.as_u64()) {
            (Some(p), Some(q)) => p == q,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_value(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_value(v, w)))
        }
        _ => a == b,
    }
}

/// How long the candidate's SCRIPT — definition plus every assertion — may
/// run. The clock starts when the evaluator signals the engine is built:
/// constructing a boa context is OUR cost, and a host slow enough that the
/// build eats the window must fail the host, not a correct solution (CI's
/// loaded runners proved the failure mode is real).
///
/// A wrong regex loop is an ordinary small-model failure and it has to cost
/// 250ms, not a wedged admin request. That is only true if the calls happen
/// INSIDE the timed region: the timeout below covers the evaluation of the
/// whole script and nothing else, so pulling the function out and calling it
/// afterwards puts an unbounded `while (true) {}` on the host's stack with no
/// timeout anywhere near it. Written down because the obvious shape of this
/// function is the broken one.
pub const CODE_TIMEOUT_MS: u64 = 250;

/// The loop and recursion ceilings for the evaluating thread. These are NOT the
/// timeout — they are what reaps a thread whose timeout already fired, so a run
/// cannot accumulate spinning threads. A correct solution runs its loops tens
/// of times; ten million is five orders of headroom.
const LOOP_ITERATION_LIMIT: u64 = 10_000_000;
const RECURSION_LIMIT: usize = 1_024;

/// What the evaluating thread sends back. The JsValue itself never crosses the
/// channel — boa objects are not `Send` — so the string-or-not conversion
/// happens on the thread that owns the context.
enum EvalOutcome {
    Reply(String),
    NotAString,
    Failed(String),
}

/// The script's own JSON coming back, validated. Defensive because the string
/// is produced by a program the model wrote: a candidate that shadows
/// `JSON.stringify` can return anything at all, and that has to fail the task,
/// never the probe.
#[derive(Deserialize)]
struct CodeReply {
    nofn: Option<bool>,
    out: Option<Vec<CodeOut>>,
}

#[derive(Deserialize)]
struct CodeOut {
    ok: bool,
    value: Option<String>,
    error: Option<String>,
}

fn read_code_result(reply: &str) -> Option<CodeReply> {
    serde_json::from_str::<CodeReply>(reply).ok()
}

/// The candidate's completion value, or the message for "the code did not run".
/// Everything boa-related happens on the spawned thread; the caller's only
/// concern is the clock. The clock starts at the worker's READY signal —
/// after context construction, before the first line of candidate code — so
/// the window times the candidate, never our engine setup.
fn eval_candidate(script: String) -> Result<EvalOutcome, String> {
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("code-probe".into())
        .spawn(move || {
            let mut context = build_boa_context();
            // The clock starts here. The receiver may already be gone
            // (timeout fired during evaluation); a send error is not one,
            // because nobody is waiting to hear it.
            let _ = ready_tx.send(());
            let outcome = run_in_boa(&mut context, &script);
            let _ = tx.send(outcome);
        })
        .expect("spawning the code-probe thread is not a resource decision we make");
    // Engine setup is untimed and bounded only by the worker's life: a slow
    // build must not eat the candidate's window (the flake this split fixes),
    // and a worker that dies mid-build surfaces as the same "did not come
    // back" sentence an eval-time panic gets.
    if ready_rx.recv_timeout(Duration::from_secs(30)).is_err() {
        let _ = worker.join();
        return Err("the evaluator did not come back".to_string());
    }
    match rx.recv_timeout(Duration::from_millis(CODE_TIMEOUT_MS)) {
        Ok(outcome) => Ok(outcome),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // node's sentence, so a timed-out candidate reads identically in
            // either language.
            Err(format!(
                "Script execution timed out after {CODE_TIMEOUT_MS}ms"
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // The evaluator itself died — a boa panic we did not foresee. The
            // candidate still fails, the probe still lives.
            let _ = worker.join();
            Err("the evaluator did not come back".to_string())
        }
    }
}

/// The engine the candidate runs in: a fresh context per probe (nothing a
/// candidate defines can outlive its own evaluation), the runtime limits
/// that reap a timed-out thread, and a stubbed console. Setup only — no
/// candidate code runs in here, which is exactly why it sits outside the
/// timed window.
fn build_boa_context() -> boa_engine::Context {
    use boa_engine::Context;
    let mut context = Context::default();
    let limits = context.runtime_limits_mut();
    limits.set_loop_iteration_limit(LOOP_ITERATION_LIMIT);
    limits.set_recursion_limit(RECURSION_LIMIT);
    context
}

fn run_in_boa(context: &mut boa_engine::Context, script: &str) -> EvalOutcome {
    use boa_engine::Source;
    // `console` is stubbed rather than omitted: a model that left a debug log
    // in an otherwise correct function solved the problem, and a ReferenceError
    // for it would score our context instead of the model. Everything else the
    // fresh context has is the ECMAScript standard library and nothing more.
    let prelude =
        "var console = { log: function () {}, warn: function () {}, error: function () {} };";
    let evaluated = context
        .eval(Source::from_bytes(prelude))
        .and_then(|_| context.eval(Source::from_bytes(script.as_bytes())));
    match evaluated {
        Ok(value) => match value.as_string() {
            Some(s) => EvalOutcome::Reply(s.to_std_string_escaped()),
            None => EvalOutcome::NotAString,
        },
        Err(err) => EvalOutcome::Failed(err.to_string()),
    }
}

/// Run one task's assertions against the model's source. Returns None when
/// every assertion passed, or the one line the admin reads.
pub fn run_code_task(task: &CodeTask, raw: &str) -> Option<String> {
    let src = extract_code(raw);
    if src.trim().is_empty() {
        return Some(format!("{}: the model returned no code", task.name));
    }
    // The arguments cross as JSON text, parsed INSIDE the sandbox one array per
    // case, so a solution that mutates its input cannot make a later case fail
    // for a reason that has nothing to do with the model.
    let arg_lists: Vec<&Vec<Value>> = task.cases.iter().map(|c| &c.args).collect();
    let args_text = serde_json::to_string(&arg_lists).expect("static fixtures serialize");
    let args_literal = serde_json::to_string(&args_text).expect("a JSON text string serializes");
    let script = format!(
        "{src}
;(function () {{
  if (typeof {fn_} !== 'function') return JSON.stringify({{ nofn: true }})
  var cases = JSON.parse({args_literal})
  var out = []
  for (var i = 0; i < cases.length; i++) {{
    try {{ out.push({{ ok: true, value: JSON.stringify({fn_}.apply(null, cases[i])) }}) }}
    catch (e) {{ out.push({{ ok: false, error: String((e && e.message) || e) }}) }}
  }}
  return JSON.stringify({{ out: out }})
}})()",
        fn_ = task.fn_,
    );
    let reply = match eval_candidate(script) {
        // `Failed` is the one branch whose sentence carries the thrown error;
        // every engine words its parse errors differently, so the shape is the
        // contract, not the message text.
        Ok(EvalOutcome::Reply(text)) => text,
        Ok(EvalOutcome::NotAString) => {
            return Some(format!("{}: the code did not produce a result", task.name));
        }
        Ok(EvalOutcome::Failed(msg)) => {
            return Some(format!("{}: the code did not run ({msg})", task.name));
        }
        Err(msg) => return Some(format!("{}: the code did not run ({msg})", task.name)),
    };
    let parsed = match read_code_result(&reply) {
        Some(parsed) => parsed,
        None => return Some(format!("{}: the code did not produce a result", task.name)),
    };
    if parsed.nofn == Some(true) {
        return Some(format!(
            "{}: no function named {} was defined",
            task.name, task.fn_
        ));
    }
    let out = parsed.out.unwrap_or_default();
    for (i, c) in task.cases.iter().enumerate() {
        let args_json = serde_json::to_string(&c.args).expect("static fixtures serialize");
        let inner = args_json
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(&args_json);
        let call = format!("{}({})", task.name, inner);
        let Some(got) = out.get(i) else {
            return Some(format!("{call} was never reached"));
        };
        if !got.ok {
            return Some(format!(
                "{} threw: {}",
                call,
                got.error.as_deref().unwrap_or("unknown error")
            ));
        }
        // An absent `value` means the function returned undefined (or something
        // JSON cannot carry), which is a real answer and almost never the right
        // one. JS prints that as the bare word `undefined`.
        let value: Option<Value> = got
            .value
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok());
        let printed = match &value {
            Some(v) => serde_json::to_string(v).expect("a parsed value serializes"),
            None => "undefined".to_string(),
        };
        let expect_text = serde_json::to_string(&c.expect).expect("static fixtures serialize");
        if !value.as_ref().is_some_and(|v| same_value(v, &c.expect)) {
            return Some(format!("{call} returned {printed}, expected {expect_text}"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD_SLUGIFY: &str = "function slugify(input) {
  return String(input).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}";

    const GOOD_MERGE: &str = "function mergeRanges(ranges) {
  const sorted = ranges.map((r) => [r[0], r[1]]).sort((a, b) => a[0] - b[0])
  const out = []
  for (const r of sorted) {
    const last = out[out.length - 1]
    if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1])
    else out.push([r[0], r[1]])
  }
  return out
}";

    fn slugify() -> &'static CodeTask {
        &CODE_TASKS[0]
    }

    fn merge() -> &'static CodeTask {
        &CODE_TASKS[1]
    }

    #[test]
    fn passes_a_correct_function_graded_by_running_the_assertions() {
        assert_eq!(run_code_task(slugify(), GOOD_SLUGIFY), None);
        assert_eq!(run_code_task(merge(), GOOD_MERGE), None);
    }

    #[test]
    fn accepts_the_two_wrappers_a_model_habitually_adds() {
        let wrapped = format!("```js\nexport {GOOD_SLUGIFY}\n```");
        assert_eq!(run_code_task(slugify(), &wrapped), None);
    }

    #[test]
    fn accepts_a_stray_debug_log_rather_than_failing_for_our_bare_context() {
        let chatty = GOOD_SLUGIFY.replacen("return", "console.log('slugifying');\n  return", 1);
        assert_eq!(run_code_task(slugify(), &chatty), None);
    }

    #[test]
    fn names_the_exact_failing_assertion_for_a_near_miss() {
        // The classic small-model version: no trimming of the leading/trailing dash.
        let nearly = "function slugify(input) { return String(input).toLowerCase().replace(/[^a-z0-9]+/g, '-') }";
        let problem = run_code_task(slugify(), nearly).expect("the near-miss fails");
        assert!(
            problem.contains("expected \"hello-world\""),
            "got: {problem}"
        );
    }

    #[test]
    fn fails_a_function_that_was_never_defined_and_prose_with_no_code() {
        let missing = run_code_task(slugify(), "function slug(x) { return x }")
            .expect("the wrong name fails");
        assert!(
            missing.contains("no function named slugify"),
            "got: {missing}"
        );
        let empty = run_code_task(slugify(), "   ").expect("empty fails");
        assert!(empty.contains("returned no code"), "got: {empty}");
    }

    #[test]
    fn fails_code_that_does_not_parse_rather_than_throwing_out_of_the_probe() {
        let broken = run_code_task(
            slugify(),
            "function slugify(input) { return input.toLowerCase(",
        )
        .expect("a parse error fails");
        assert!(broken.contains("did not run"), "got: {broken}");
    }

    #[test]
    fn survives_an_infinite_loop_at_the_cost_of_a_timeout_not_a_wedged_request() {
        let started = std::time::Instant::now();
        let spin = run_code_task(slugify(), "function slugify(input) { while (true) {} }")
            .expect("the spin fails");
        assert!(spin.contains("did not run"), "got: {spin}");
        assert!(
            started.elapsed() < Duration::from_millis(2_000),
            "a timeout costs its 250ms, not a hang"
        );
    }

    #[test]
    fn is_not_corrupted_by_a_function_that_mutates_its_arguments() {
        let mutating = "function mergeRanges(ranges) {
      ranges.sort((a, b) => a[0] - b[0])
      const out = []
      for (const r of ranges) {
        const last = out[out.length - 1]
        if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1])
        else out.push(r)
      }
      return out
    }";
        assert_eq!(run_code_task(merge(), mutating), None);
    }

    #[test]
    fn takes_the_fenced_block_when_there_is_one_and_leaves_bare_source_alone() {
        assert_eq!(
            extract_code("Here you go:\n```javascript\nconst a = 1\n```\nDone."),
            "const a = 1\n"
        );
        assert_eq!(extract_code("const a = 1"), "const a = 1");
    }

    #[test]
    fn strips_only_a_leading_export_not_the_word_export_inside_code() {
        assert_eq!(
            extract_code("export function f() { return \"export function\" }"),
            "function f() { return \"export function\" }"
        );
    }
}
