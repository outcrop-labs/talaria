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

/// The tests that read or set the org's model roles hold this: the roles are
/// one row for the whole database, and this module's tests run at once.
static ROLES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let _roles = ROLES.lock().await;
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

/// A ready record as provisioning writes it, for `model_arg`.
fn ready_for(model_arg: &str) -> Value {
    let mut r = Record::start(1);
    r.harness = Some(json!({
        "omp": format!("omp/{}", talaria_workbench_harnesses::OMP_VERSION),
        "model": model_arg.trim_start_matches("talaria/"),
        "modelArg": model_arg,
        "envPrefix": "",
    }));
    r.ready(2);
    r.to_json(2)
}

#[tokio::test]
#[ignore = "needs a live dev database (source ui/.env)"]
async fn the_lines_name_the_checked_model_and_a_moved_model_is_provisioned_again() {
    let _roles = ROLES.lock().await;
    let pg = pg().await;
    let slug = "wb-prov-model";
    reset(&pg, slug).await;
    let (agent_id, job) = fixture(&pg, slug).await;
    let saved = talaria_model_roles::get_model_roles(&pg).await;
    sqlx::query("delete from llm_endpoints where name = 'wbprov-prov-ep'")
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query(
        "insert into llm_endpoints (name, provider, base_url, class, models) \
         values ('wbprov-prov-ep', 'openai', 'http://127.0.0.1:9/v1', 'cloud', '[\"m-std\", \"m-light\"]'::jsonb)",
    )
    .execute(&pg)
    .await
    .unwrap();
    talaria_gateway::registry::invalidate_endpoints_cache();
    talaria_model_roles::set_model_role(&pg, "code-standard", Some("wbprov-prov-ep/m-std"))
        .await
        .unwrap();
    talaria_model_roles::set_model_role(&pg, "code-light", Some("wbprov-prov-ep/m-light"))
        .await
        .unwrap();

    // No coding account: the org's code-standard role is the job's model,
    // on the gateway, and the line's --model is it.
    let jm = provision::job_model(&pg, &agent_id, None).await.unwrap();
    assert!(jm.gateway);
    assert_eq!(jm.model.as_deref(), Some("wbprov-prov-ep/m-std"));
    assert_eq!(jm.arg.as_deref(), Some("talaria/wbprov-prov-ep/m-std"));
    assert_eq!(jm.env_prefix, "");

    // Ready for exactly that model: the lines name what the preflight checked.
    set_provision(&pg, &job, &ready_for("talaria/wbprov-prov-ep/m-std")).await;
    let j = job_status(&pg, slug, json!({ "jobId": job })).await;
    let line = j["harnesses"][0]["jsonRun"]
        .as_str()
        .unwrap_or_else(|| panic!("no lines: {j}"));
    assert!(
        line.contains("--model talaria/wbprov-prov-ep/m-std "),
        "{line}"
    );

    // The role moves after ready: the lines would name a model no preflight
    // checked, so job_status provisions the job again and hands out none.
    talaria_model_roles::set_model_role(&pg, "code-standard", Some("wbprov-prov-ep/m-light"))
        .await
        .unwrap();
    let j = job_status(&pg, slug, json!({})).await;
    assert!(j.get("harnesses").is_none(), "no stale lines: {j}");
    assert_ne!(j["provision"]["state"], json!("ready"), "{j}");

    // Restore what the database had.
    talaria_settings::set_setting(&pg, "model_roles", &saved)
        .await
        .unwrap();
    sqlx::query("delete from llm_endpoints where name = 'wbprov-prov-ep'")
        .execute(&pg)
        .await
        .unwrap();
    talaria_gateway::registry::invalidate_endpoints_cache();
    reset(&pg, slug).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (source ui/.env)"]
async fn a_resumed_round_is_provisioned_afresh_when_it_starts() {
    let pg = pg().await;
    let slug = "wb-prov-resume";
    reset(&pg, slug).await;
    let (agent_id, _) = fixture(&pg, slug).await;
    let pr = talaria_workbench_mcp::PullRequestRef {
        url: "https://example.invalid/pr/7".into(),
        number: 7,
    };
    for round in ["awaiting_approval", "queued"] {
        // The previous round: PR open, its provision ready (its lines still
        // in the record).
        let (job,): (String,) = sqlx::query_as(
            "insert into workbench_jobs \
               (agent_id, agent_model, task_id, host_id, repo, branch, effort, plan, status) \
             values ($1::uuid, $2, null, 'github', 'provision-live/repo', 'test/branch', 'heavy', '', 'pr_open') \
             returning id::text",
        )
        .bind(&agent_id)
        .bind(slug)
        .fetch_one(&pg)
        .await
        .unwrap();
        let old = ready_for("talaria/old-round-model");
        set_provision(&pg, &job, &old).await;
        let resumed = talaria_workbench_mcp::resume_job_row(
            &pg,
            &job,
            &talaria_workbench_mcp::JobResume {
                status: round,
                effort: "heavy",
                plan: "round two",
                queue_reason: (round == "queued").then_some("host has 1.0 GiB RAM free"),
                taking_over: None,
            },
            &pr,
        )
        .await;
        assert!(resumed.is_some(), "{round}: the round resumed");
        assert!(
            provision::read(&pg, &job).await.is_none(),
            "{round}: the old provision is gone"
        );
        // Approved, or promoted from the queue: started, and provisioned
        // afresh — never the last round's lines.
        sqlx::query("update workbench_jobs set status = 'started' where id = $1::uuid")
            .bind(&job)
            .execute(&pg)
            .await
            .unwrap();
        let j = job_status(&pg, slug, json!({ "jobId": job })).await;
        assert!(j.get("harnesses").is_none(), "{round}: no old lines: {j}");
        let now = provision::read(&pg, &job).await.expect("provisioned");
        assert_ne!(now["runId"], old["runId"], "{round}: a new run: {now}");
        sqlx::query("update workbench_jobs set status = 'abandoned' where id = $1::uuid")
            .bind(&job)
            .execute(&pg)
            .await
            .unwrap();
    }
    reset(&pg, slug).await;
}
