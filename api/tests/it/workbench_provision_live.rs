use crate::support::pg;
// Live-DB proof of workbench provisioning's record (cargo test -- --ignored):
// the claim that lets exactly one run own a job's provision, the harness
// lines handed out only once the provision is ready, and job_status
// provisioning a failed job again when — and only when — the agent asks.
// These are SQL and dispatch paths, so only a real database vouches for them.
//
//   source ui/.env && cargo test --test it -- --ignored workbench_provision_live::

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_agent_auth::AgentSubject;
use talaria_workbench_mcp::provision::{self, Record, STALE_AFTER_MS};
use talaria_workbench_mcp::{WorkbenchDeps, dispatch_workbench_mcp};

const DEPT: &str = "wb-provision-test";

/// One test's own rows (its agent; the FK cascade takes its job). Per slug,
/// never the whole department: the module's tests run concurrently, and a
/// department-wide delete at one test's end removed another's job mid-test.
async fn reset(pg: &PgPool, slug: &str) {
    sqlx::query("delete from agent_defs where department = $1 and slug = $2")
        .bind(DEPT)
        .bind(slug)
        .execute(pg)
        .await
        .unwrap();
}

/// A Developer Agent of this suite's own, and one started job of its.
async fn fixture(pg: &PgPool, slug: &str) -> (String, String) {
    let (agent_id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name, developer) \
         values ($1, $2, $1, 'WB Provision Test', true) returning id::text",
    )
    .bind(slug)
    .bind(DEPT)
    .fetch_one(pg)
    .await
    .unwrap();
    let (job_id,): (String,) = sqlx::query_as(
        "insert into workbench_jobs \
           (agent_id, agent_model, task_id, host_id, repo, branch, effort, plan, status) \
         values ($1::uuid, $2, null, 'github', 'provision-live/repo', 'test/branch', 'light', '', 'started') \
         returning id::text",
    )
    .bind(&agent_id)
    .bind(slug)
    .fetch_one(pg)
    .await
    .unwrap();
    (agent_id, job_id)
}

async fn set_provision(pg: &PgPool, job_id: &str, v: &Value) {
    sqlx::query("update workbench_jobs set provision = $2::jsonb where id = $1::uuid")
        .bind(job_id)
        .bind(v)
        .execute(pg)
        .await
        .unwrap();
}

/// job_status as the gateway hands it to the workbench server; the job as
/// the answer lists it.
async fn job_status(pg: &PgPool, slug: &str, args: Value) -> Value {
    let deps = WorkbenchDeps {
        pg: pg.clone(),
        sb: talaria_api::secretbox::SecretBox::default(),
        redis: None,
    };
    let rpc = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "job_status", "arguments": args },
    });
    let (_, out) =
        dispatch_workbench_mcp(&deps, &rpc, &AgentSubject::Model(slug.to_string()), None).await;
    let out = out.expect("a JSON-RPC answer");
    let text = out["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text in {out}"));
    let parsed: Value = serde_json::from_str(text).unwrap_or_else(|_| panic!("not JSON: {text}"));
    parsed["jobs"][0].clone()
}

#[tokio::test]
#[ignore = "needs a live dev database (source ui/.env)"]
async fn one_run_owns_a_provision_and_a_dead_one_is_taken_over() {
    let pg = pg().await;
    reset(&pg, "wb-prov-claim").await;
    let (_, job) = fixture(&pg, "wb-prov-claim").await;

    // Never provisioned: the first claim takes it.
    let first = Record::start(talaria_agent_auth::now_ms());
    assert!(provision::claim(&pg, &job, &first).await.unwrap());
    // Alive: a second run is refused, so a provision is never doubled.
    let second = Record::start(talaria_agent_auth::now_ms());
    assert!(!provision::claim(&pg, &job, &second).await.unwrap());
    let stored = provision::read(&pg, &job).await.unwrap();
    assert_eq!(
        stored["runId"],
        json!(first.run_id),
        "the first run still owns it"
    );

    // Dead (no heartbeat for longer than STALE_AFTER_MS): taken over.
    let mut dead = stored.clone();
    dead["heartbeatMs"] = json!(talaria_agent_auth::now_ms() - STALE_AFTER_MS - 1_000);
    set_provision(&pg, &job, &dead).await;
    assert!(provision::claim(&pg, &job, &second).await.unwrap());
    assert_eq!(
        provision::read(&pg, &job).await.unwrap()["runId"],
        json!(second.run_id)
    );

    // Ready or failed: a fresh provision may start (prepare_env, a retry).
    let mut ready = Record::start(1);
    ready.ready(2);
    set_provision(&pg, &job, &ready.to_json(2)).await;
    let third = Record::start(talaria_agent_auth::now_ms());
    assert!(provision::claim(&pg, &job, &third).await.unwrap());

    // A job that ended, or whose workspace was cleared, is never claimed.
    let mut done = Record::start(1);
    done.ready(2);
    set_provision(&pg, &job, &done.to_json(2)).await;
    sqlx::query("update workbench_jobs set status = 'abandoned' where id = $1::uuid")
        .bind(&job)
        .execute(&pg)
        .await
        .unwrap();
    assert!(
        !provision::claim(&pg, &job, &Record::start(talaria_agent_auth::now_ms()))
            .await
            .unwrap()
    );
    sqlx::query(
        "update workbench_jobs set status = 'started', workspace_cleared_at = now() where id = $1::uuid",
    )
    .bind(&job)
    .execute(&pg)
    .await
    .unwrap();
    assert!(
        !provision::claim(&pg, &job, &Record::start(talaria_agent_auth::now_ms()))
            .await
            .unwrap()
    );
    reset(&pg, "wb-prov-claim").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (source ui/.env)"]
async fn the_harness_lines_are_withheld_until_the_provision_is_ready() {
    let pg = pg().await;
    reset(&pg, "wb-prov-lines").await;
    let (_, job) = fixture(&pg, "wb-prov-lines").await;

    // Running, alive: provision on the wire, no lines. (Listed, not asked
    // about by id, so the answer does not wait for it.)
    let mut running = Record::start(1).to_json(talaria_agent_auth::now_ms());
    running["step"] = json!("toolchain");
    set_provision(&pg, &job, &running).await;
    let j = job_status(&pg, "wb-prov-lines", json!({})).await;
    assert_eq!(j["provision"]["state"], json!("running"), "{j}");
    assert_eq!(j["provision"]["step"], json!("toolchain"));
    assert!(
        j["provision"]["next"]
            .as_str()
            .unwrap()
            .contains("job_status")
    );
    assert!(j.get("harnesses").is_none(), "no lines before ready: {j}");
    assert!(j["provision"].get("runId").is_none() && j["provision"].get("heartbeatMs").is_none());

    // Ready, for the pinned omp: the lines, starting with this job's launcher.
    let mut ready = Record::start(1);
    ready.harness =
        Some(json!({ "omp": format!("omp/{}", talaria_workbench_harnesses::OMP_VERSION) }));
    ready.ready(2);
    set_provision(&pg, &job, &ready.to_json(2)).await;
    let j = job_status(&pg, "wb-prov-lines", json!({ "jobId": job })).await;
    assert_eq!(j["provision"]["state"], json!("ready"), "{j}");
    let line = j["harnesses"][0]["jsonRun"]
        .as_str()
        .unwrap_or_else(|| panic!("no lines: {j}"));
    assert!(
        line.starts_with(&format!("/opt/data/workbench/harness/jobs/{job}/omp ")),
        "{line}"
    );
    assert!(!line.contains("npx") && !line.contains("@latest"));
    reset(&pg, "wb-prov-lines").await;
}

#[tokio::test]
#[ignore = "needs a live dev database (source ui/.env)"]
async fn job_status_provisions_a_failed_job_again_only_when_asked() {
    let pg = pg().await;
    reset(&pg, "wb-prov-retry").await;
    let (_, job) = fixture(&pg, "wb-prov-retry").await;
    let mut failed = Record::start(1);
    failed.begin("clone");
    failed.fail(
        &provision::StepFailure {
            step: "clone".into(),
            ms: 5,
            attempts: 2,
            error: "could not clone: fatal: unable to access the host".into(),
            output: String::new(),
            ended: false,
        },
        2,
    );
    set_provision(&pg, &job, &failed.to_json(2)).await;

    // Not asked: the failure stands, untouched.
    let j = job_status(&pg, "wb-prov-retry", json!({})).await;
    assert_eq!(j["provision"]["state"], json!("failed"));
    assert_eq!(j["provision"]["step"], json!("clone"));
    assert!(j.get("harnesses").is_none());
    let next = j["provision"]["next"].as_str().unwrap();
    assert!(
        next.contains("retry: true") && next.contains("report_outcome"),
        "{next}"
    );
    assert!(!next.to_lowercase().contains("blocked"), "{next}");

    // Asked: a new run starts. This database has no sandbox to provision in,
    // so the new run fails at its first step — which is the proof that it
    // ran: a different run, and a different failing step.
    let j = job_status(&pg, "wb-prov-retry", json!({ "jobId": job, "retry": true })).await;
    let stored = provision::read(&pg, &job).await.unwrap();
    assert_ne!(
        stored["runId"],
        json!(failed.run_id),
        "a new run owns the record: {stored}"
    );
    assert_ne!(j["provision"]["step"], json!("clone"), "{j}");
    assert!(
        j.get("harnesses").is_none(),
        "still no lines without a ready harness"
    );
    reset(&pg, "wb-prov-retry").await;
}
