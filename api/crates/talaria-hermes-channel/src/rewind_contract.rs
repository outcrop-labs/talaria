// THE REWIND CONTRACT — asked of the canary by doing what a rewind does, on a
// transcript written into its own store, with the same scripts a rewind runs.
// A cut goes through Hermes internals no release promises to keep, so a symbol
// check alone would pass a release that kept the names and changed what they
// do. No model is called, so it costs no token. A release that fails it is
// held for an admin: every rewind on it would fail.

use serde_json::{Value, json};
use talaria_fleet_docker::rewind::{
    self, SEED_CUT_CALL, SEED_CUT_ROWS, SEED_ROWS, SessionLease, Unready,
};

use crate::ops::Fault;
use crate::probe::{get_json, post_json};

type Answer = (u16, Value);

/// The first thing a rewind needs that `container` does not give it, as a
/// problem for the release's hold; `Err` is no answer, the host's to retry.
/// `session` names sessions this makes, and must be free.
pub async fn ask(
    url: &str,
    key: &str,
    container: &str,
    session: &str,
) -> Result<Vec<String>, String> {
    match fork_and_cut(url, key, container, session).await {
        Ok(()) => Ok(Vec::new()),
        Err(Fault::Held(problems)) => Ok(problems),
        Err(Fault::Infra(e) | Fault::Image(e)) => Err(e),
    }
}

async fn fork_and_cut(url: &str, key: &str, container: &str, session: &str) -> Result<(), Fault> {
    rewind::probe_container(container)
        .await
        .map_err(|e| match e {
            Unready::Lacks(what) => held(format!(
                "this Hermes release does not support rewind ({what}): no agent on it could rewind a run"
            )),
            Unready::NoAnswer(e) => Fault::Infra(format!("the canary's rewind probe: {e}")),
        })?;

    let no_run = format!("{url}/v1/runs/{session}-no-run");
    knows_no_such_run(
        &post(&format!("{no_run}/stop"), key, session).await?,
        "/v1/runs/{id}/stop",
    )
    .map_err(held)?;
    knows_no_such_run(&read(&no_run, key).await?, "/v1/runs/{id}").map_err(held)?;

    let fork = format!("{session}-fork");
    let refork = format!("{session}-refork");
    let fork_route = format!("{url}/api/sessions/{session}/fork");

    let opened = post(&format!("{url}/api/sessions"), key, session).await?;
    if opened.0 != 201 {
        return Err(held(format!(
            "/api/sessions answered {}: the canary could not open a session to fork",
            opened.0
        )));
    }
    in_container(
        rewind::seed_session(container, session).await,
        "seeding a session",
    )?;

    forked(&post(&fork_route, key, &fork).await?, 201, session).map_err(held)?;
    refused_as_taken(&post(&fork_route, key, &fork).await?).map_err(held)?;
    let child = read(&format!("{url}/api/sessions/{fork}"), key).await?;
    forked(&child, 200, session).map_err(held)?;

    let cut = in_container(
        rewind::cut_session_in(container, &fork, SEED_CUT_CALL).await,
        "the cut",
    )?;
    if (cut.was, cut.now) != (SEED_ROWS as u64, SEED_CUT_ROWS as u64)
        || cut.last_tool_call_id.as_deref() != Some(SEED_CUT_CALL)
    {
        return Err(held(format!(
            "the cut left {} of {} rows where {SEED_CUT_ROWS} of {SEED_ROWS} should stand: a rewind would not end at its step",
            cut.now, cut.was
        )));
    }
    rows_are(
        &read(&messages(url, &fork), key).await?,
        &fork,
        SEED_CUT_ROWS,
        Some(SEED_CUT_CALL),
    )
    .map_err(held)?;
    rows_are(
        &read(&messages(url, session), key).await?,
        session,
        SEED_ROWS,
        None,
    )
    .map_err(held)?;

    let lease = in_container(
        rewind::session_lease_in(container, &fork).await,
        "the turn lease read",
    )?;
    if lease != SessionLease::Idle {
        return Err(held(
            "a session with no turn running reads as busy: a rewind would never see the agent quiet",
        ));
    }

    forked(&post(&fork_route, key, &refork).await?, 201, session).map_err(held)?;
    in_container(
        rewind::discard_session_in(container, &refork, session).await,
        "taking back a failed rewind's fork",
    )?;
    let reopened = post(&format!("{url}/api/sessions"), key, &refork).await?;
    if reopened.0 != 201 {
        return Err(held(format!(
            "a discarded fork's id answered {} when opened again: every failed rewind would spend a session slot",
            reopened.0
        )));
    }
    Ok(())
}

fn held(problem: impl Into<String>) -> Fault {
    Fault::Held(vec![problem.into()])
}

fn messages(url: &str, session: &str) -> String {
    format!("{url}/api/sessions/{session}/messages?limit=500&offset=0&order=oldest")
}

async fn post(url: &str, key: &str, id: &str) -> Result<Answer, Fault> {
    post_json(url, Some(key), &json!({ "id": id }))
        .await
        .map_err(|e| Fault::Infra(format!("the canary's session routes: {e}")))
}

async fn read(url: &str, key: &str) -> Result<Answer, Fault> {
    get_json(url, Some(key))
        .await
        .map_err(|e| Fault::Infra(format!("the canary's session routes: {e}")))
}

fn in_container<T>(answer: Result<T, String>, doing: &str) -> Result<T, Fault> {
    answer.map_err(|e| {
        if rewind::docker_failed(&e) {
            Fault::Infra(format!("the canary's {doing}: {e}"))
        } else {
            held(format!(
                "{doing} failed on this Hermes release: {}",
                e.lines().last().unwrap_or(&e).trim()
            ))
        }
    })
}

/// An unknown run is how the stop routes can be told from routes that are
/// gone with no turn to stop: a bare 404 is the router's, not the route's.
fn knows_no_such_run((status, body): &Answer, route: &str) -> Result<(), String> {
    if *status == 404
        && body.pointer("/error/code").and_then(Value::as_str) == Some("run_not_found")
    {
        return Ok(());
    }
    Err(format!(
        "{route} answered {status} for an unknown run, not 404 run_not_found: a rewind stops a running turn through it"
    ))
}

fn forked((status, body): &Answer, want: u16, parent: &str) -> Result<(), String> {
    if *status != want {
        return Err(format!(
            "/api/sessions/{{id}}/fork answered {status}: a rewind forks the attempt's conversation through it"
        ));
    }
    if body
        .pointer("/session/parent_session_id")
        .and_then(Value::as_str)
        != Some(parent)
    {
        return Err(
            "a fork no longer names its source as parent: a retried rewind tells its own fork from a stranger's by it".into(),
        );
    }
    Ok(())
}

fn refused_as_taken((status, body): &Answer) -> Result<(), String> {
    if *status == 409
        && body.pointer("/error/code").and_then(Value::as_str) == Some("session_exists")
    {
        return Ok(());
    }
    Err(format!(
        "a second fork under the same id answered {status}, not 409 session_exists: a retried rewind recognizes its fork by that"
    ))
}

/// `session`'s rows, read as a rewind verifies a cut: answered for that id,
/// `rows` of them, the last a tool result of `tail` when one is named.
fn rows_are(
    (status, body): &Answer,
    session: &str,
    rows: usize,
    tail: Option<&str>,
) -> Result<(), String> {
    let why = "a rewind verifies its cut through it";
    if *status != 200 {
        return Err(format!(
            "/api/sessions/{{id}}/messages answered {status}: {why}"
        ));
    }
    if body.get("session_id").and_then(Value::as_str) != Some(session) {
        return Err(format!(
            "/api/sessions/{{id}}/messages answered for another session: {why}"
        ));
    }
    let Some(data) = body.get("data").and_then(Value::as_array) else {
        return Err(format!(
            "/api/sessions/{{id}}/messages has no data list: {why}"
        ));
    };
    if data.len() != rows {
        return Err(format!(
            "/api/sessions/{{id}}/messages listed {} rows where {rows} were left: {why}",
            data.len()
        ));
    }
    if let Some(call) = tail {
        let last = data.last().unwrap_or(&Value::Null);
        if last["role"].as_str() != Some("tool") || last["tool_call_id"].as_str() != Some(call) {
            return Err(format!(
                "/api/sessions/{{id}}/messages no longer ends on the tool call's row: {why}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fork_answer(status: u16, parent: &str) -> Answer {
        (
            status,
            json!({ "session": { "parent_session_id": parent } }),
        )
    }

    #[test]
    fn a_fork_route_that_moved_or_lost_its_parent_is_a_problem() {
        assert_eq!(forked(&fork_answer(201, "src"), 201, "src"), Ok(()));
        let gone = forked(&(404, json!({})), 201, "src").unwrap_err();
        assert!(
            gone.contains("answered 404") && gone.contains("forks"),
            "{gone}"
        );
        let orphan = forked(&fork_answer(201, "other"), 201, "src").unwrap_err();
        assert!(orphan.contains("parent"), "{orphan}");
    }

    #[test]
    fn a_bare_404_means_the_stop_route_is_gone() {
        let unknown = (404, json!({ "error": { "code": "run_not_found" } }));
        assert_eq!(knows_no_such_run(&unknown, "/v1/runs/{id}/stop"), Ok(()));
        let gone = knows_no_such_run(&(404, Value::Null), "/v1/runs/{id}/stop").unwrap_err();
        assert!(gone.contains("/v1/runs/{id}/stop answered 404"), "{gone}");
    }

    #[test]
    fn only_session_exists_tells_a_retry_its_fork_is_there() {
        let taken = (409, json!({ "error": { "code": "session_exists" } }));
        assert_eq!(refused_as_taken(&taken), Ok(()));
        assert!(refused_as_taken(&(201, json!({}))).is_err());
        assert!(refused_as_taken(&(409, json!({ "error": { "code": "locked" } }))).is_err());
    }

    fn listing(session: &str, data: Value) -> Answer {
        (200, json!({ "session_id": session, "data": data }))
    }

    #[test]
    fn a_cut_reads_back_only_when_it_ends_on_the_tool_row() {
        let rows = json!([{ "role": "user" }, { "role": "tool", "tool_call_id": "c1" }]);
        assert_eq!(
            rows_are(&listing("s", rows.clone()), "s", 2, Some("c1")),
            Ok(())
        );
        for (answer, want) in [
            (listing("s", rows.clone()), "listed 2 rows"),
            (listing("elsewhere", rows.clone()), "another session"),
            ((200, json!({ "session_id": "s" })), "no data list"),
            ((500, json!({})), "answered 500"),
        ] {
            let problem = rows_are(&answer, "s", 3, Some("c1")).unwrap_err();
            assert!(problem.contains(want), "{problem}");
        }
        let wrong_tail = json!([{ "role": "user" }, { "role": "tool", "tool_call_id": "c2" }]);
        assert!(rows_are(&listing("s", wrong_tail), "s", 2, Some("c1")).is_err());
    }

    #[test]
    fn a_script_that_crashed_is_the_release_and_docker_failing_is_the_host() {
        let crashed = in_container::<()>(
            Err("in c: Traceback (most recent call last):\nAttributeError: gone".into()),
            "the cut",
        );
        assert_eq!(
            crashed,
            Err(held(
                "the cut failed on this Hermes release: AttributeError: gone"
            ))
        );
        let refused = in_container::<()>(
            Err("in c: Error response from daemon: container c is not running".into()),
            "the cut",
        );
        assert!(matches!(refused, Err(Fault::Infra(_))), "{refused:?}");
    }

    /// The whole contract on a live Hermes. CI's `--ignored` job has none, so
    /// it checks nothing there. By hand: the container's name, its API url and
    /// its key in `TALARIA_REWIND_CONTRACT_{CONTAINER,URL,KEY}`.
    #[tokio::test]
    #[ignore = "needs a running Hermes container"]
    async fn the_contract_on_a_live_hermes() {
        let env = |k: &str| std::env::var(format!("TALARIA_REWIND_CONTRACT_{k}"));
        let (Ok(container), Ok(url), Ok(key)) = (env("CONTAINER"), env("URL"), env("KEY")) else {
            eprintln!("TALARIA_REWIND_CONTRACT_CONTAINER is not set: nothing to check");
            return;
        };
        let session = format!("contract-{}", uuid::Uuid::new_v4());
        let problems = ask(&url, &key, &container, &session)
            .await
            .expect("the container answers");
        assert_eq!(problems, Vec::<String>::new());
    }
}
