// The model-fitness code probe's child process, against the REAL binary.
//
// `talaria-api __code-probe` is the hidden argv mode main.rs dispatches
// before building a runtime; the probe re-executes the api under rlimits and
// grades what comes back. The unit tests in talaria-fitness-code-runner
// cover the protocol over in-memory streams and the grading in-process; this
// is the one place the whole chain — spawn, rlimits, stdin, stdout, kill —
// runs against the executable that ships. No database, no network.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use talaria_fitness_code_runner::{CHILD_PROBE_ARG, CODE_TASKS, run_code_task_within};

fn api_bin() -> &'static str {
    env!("CARGO_BIN_EXE_talaria-api")
}

/// Drive the hidden mode by hand: the ask on stdin, the reply on stdout.
fn probe(ask: &str) -> (i32, String) {
    let mut child = Command::new(api_bin())
        .arg(CHILD_PROBE_ARG)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the api binary spawns");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(ask.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("the child exits");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

#[test]
fn the_binary_answers_the_hidden_mode_without_booting_the_api() {
    // No config, no database, no listener: the mode must answer with an
    // empty environment, which is also how the probe spawns it.
    let (code, reply) = probe(r#"{"script":"JSON.stringify({ok:true})","budget_ms":10000}"#);
    assert_eq!(code, 0, "{reply}");
    assert_eq!(reply, r#"{"Outcome":{"Reply":"{\"ok\":true}"}}"#);

    let (code, reply) = probe(r#"{"script":"while (true) {}","budget_ms":100}"#);
    assert_eq!(code, 0, "{reply}");
    assert!(reply.contains("timed out after 100ms"), "{reply}");

    let (code, reply) = probe("not an ask");
    assert_eq!(code, 2, "{reply}");
    assert!(reply.contains("bad probe ask"), "{reply}");
}

#[test]
fn the_grading_runs_the_candidate_in_the_child_and_survives_a_hostile_one() {
    // Register the real binary as the child, the way main.rs does for the
    // serving process, so `run_code_task_within` takes the subprocess path.
    let _ = talaria_fitness_code_runner::CHILD_PROBE.set(api_bin().into());
    let slugify = &CODE_TASKS[0];

    // A correct function grades correct through the child.
    let good = "function slugify(input) {
  return String(input).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}";
    assert_eq!(run_code_task_within(slugify, good, 10_000), None);

    // The reply that used to allocate gigabytes inside the api: the child
    // hits its address-space ceiling and dies, the candidate fails, and the
    // parent is back promptly with the api untouched.
    let hog = "function slugify(input) { var s = 'x'; while (true) { s = s + s } return s }";
    let started = Instant::now();
    let problem = run_code_task_within(slugify, hog, 2_000).expect("a memory hog fails");
    assert!(problem.contains("did not run"), "{problem}");
    assert!(
        started.elapsed() < Duration::from_secs(40),
        "the parent's deadline, not the kernel's OOM killer, ends it: {:?}",
        started.elapsed()
    );

    // A spin is the budget's own sentence, through the child.
    let spin = "function slugify(input) { while (true) {} }";
    let problem = run_code_task_within(slugify, spin, 200).expect("a spin fails");
    assert!(problem.contains("timed out after 200ms"), "{problem}");
}
