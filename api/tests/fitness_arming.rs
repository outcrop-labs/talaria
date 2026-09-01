// THE ARMED PATH, END TO END, WITH NOTHING FAKED BETWEEN THE PIECES — the
// port of arming.test.ts.
//
// Every other suite covers one hop: transport.rs's tests prove the gateway
// puts tool definitions on the wire, probes.rs's prove `tool-select` grades
// four calls, capability.rs's prove a fact round-trips, run.rs's prove the
// widening gate reads `source`. Each of those stubs the hop on either side of
// it, so none of them answers the question the feature is actually about:
// does a tool call a provider reported become a widened Inbox surface,
// through the real code, in one motion?
//
// It matters because the fitness workflow's own reconcile pass reported the
// feature UNARMED — `TransportRequest` had no slot for a tool definition, so
// `tool-select` skipped forever, so no `value: true` fact existed anywhere in
// Talaria, so `run_harness`'s widening branch had never executed in
// production on any install. A chain that long fails silently at any link,
// and every link is tested. So this runs the chain: a transport that reports
// real tool calls -> run_probes -> the REAL merge_capabilities (the
// single-fact spelling of `recordCapability`) -> the REAL get_capabilities ->
// run_harness's widening gate -> the action allowlist the Inbox validates
// against. Only the transport and the key derivation are scripted — and the
// key derivation is scripted ONLY because `probe_keys` derives keys from
// `llm_endpoints`, where the fabricated `pl-main` pair does not live; the TS
// mocked `routingFor` for exactly the same reason.
//
// LIVE-DB ONLY: the real record/get edges are the point, and they write the
// `harness_capabilities` settings row. The pair `pl-main:qwen3-14b` is
// fabricated — no endpoint named pl-main exists — so nothing else in Talaria
// reads the entry, and the test forgets it at the door on the way in and out.
// Never in CI.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};
use talaria_api::capability::{
    CapabilityFact, capability_key, forget_capabilities, get_capabilities, merge_capabilities,
    missing_capabilities,
};
use talaria_api::config::Config;
use talaria_api::fitness::probes::{ProbeId, ProbeOpts, default_deps, run_probes, runner_tool_ask};
use talaria_api::gateway::guard::{GuardConfig, GuardMode};
use talaria_api::gateway::params::now_ms;
use talaria_api::harness::defs::inbox_focus::{
    FocusAction, FocusCommandInput, FocusCommandMode, FocusHarnessItem, FocusSeatRole,
    allowed_focus_action_ids, inbox_command_harness,
};
use talaria_api::harness::run::{HarnessDeps, RunContext, TransportFn, run_harness};
use talaria_api::harness::transport::{ToolCall, TransportKind, TransportReply, TransportRequest};
use talaria_api::state::AppState;

// ── the scripted transport, and nothing else ─────────────────────────────────

/// A transport as a GATEWAY would answer it: text plus the tool calls the
/// provider reported. Nothing above the transport is faked.
fn tool_transport() -> TransportFn {
    Arc::new(|req: TransportRequest| {
        Box::pin(async move {
            let id = req
                .caller
                .strip_prefix("fitness:probe:")
                .unwrap_or_default()
                .to_string();
            if req.tool_defs.is_empty() {
                return Ok(TransportReply {
                    kind: TransportKind::Gateway,
                    text: "ok".into(),
                    tool_names: Vec::new(),
                    tool_calls: None,
                    usage: None,
                    contract_dropped: false,
                });
            }
            let want = match id.as_str() {
                "tool-select:weather" => "get_weather",
                "tool-select:email" => "send_email",
                "tool-select:currency" => "convert_currency",
                "tool-select:ticket" => "create_ticket",
                _ => "get_weather",
            };
            let calls = vec![ToolCall {
                name: want.into(),
                id: Some("call_0".into()),
                args: json!({"city": "Lisbon"}).to_string(),
            }];
            Ok(TransportReply {
                kind: TransportKind::Gateway,
                text: String::new(),
                tool_names: calls.iter().map(|c| c.name.clone()).collect(),
                tool_calls: Some(calls),
                usage: None,
                contract_dropped: false,
            })
        })
    })
}

/// The command harness's transport: a clean JSON reply naming the action only
/// the WIDENED surface is allowed to propose.
fn approve_transport() -> TransportFn {
    Arc::new(|_req: TransportRequest| {
        Box::pin(async move {
            Ok(TransportReply {
                kind: TransportKind::Gateway,
                text: r#"{"message":"Ready.","actionId":"approve_task"}"#.into(),
                tool_names: Vec::new(),
                tool_calls: None,
                usage: None,
                contract_dropped: false,
            })
        })
    })
}

// ── the harness run's deps: the real capability edges, the scripted rest ─────

/// The TS hands run_harness a partial deps object and lets TS's per-field
/// defaults fill the rest; Rust's HarnessDeps has no defaults, so this spells
/// them — the two capability edges are the REAL ones (that is the test), the
/// rest are the TS stubs or their inert equivalents.
fn harness_deps(pg: &sqlx::PgPool) -> HarnessDeps {
    HarnessDeps {
        // Never called: the context pins the model.
        resolve_model: Arc::new(|_spec, _user| {
            Box::pin(
                async move { Option::<(String, talaria_api::harness_model::ModelChainStep)>::None },
            )
        }),
        slot_effort: Arc::new(|_slot, _model| Box::pin(async move { Option::<String>::None })),
        // The TS scripts routingFor to `{ endpoints: ['pl-main'], upstreamModel: m }`
        // — the key the widening gate derives is the KEY this file writes.
        routing: Arc::new(|m: String| Box::pin(async move { (vec!["pl-main".to_string()], m) })),
        persona_keys: Arc::new(|_m: String| Box::pin(async move { Vec::<String>::new() })),
        missing_capabilities: {
            let pg = pg.clone();
            Arc::new(move |key: String, asked: Vec<String>| {
                let pg = pg.clone();
                Box::pin(async move {
                    let asked: Vec<&str> = asked.iter().map(String::as_str).collect();
                    missing_capabilities(&pg, &key, &asked).await
                })
            })
        },
        capabilities: {
            let pg = pg.clone();
            Arc::new(move |key: String| {
                let pg = pg.clone();
                Box::pin(async move { get_capabilities(&pg, &key).await })
            })
        },
        reach: Arc::new(|_keys: Vec<String>, _wanted: Vec<String>| {
            Box::pin(async move { HashMap::<String, talaria_api::capability_reach::Reach>::new() })
        }),
        transport: approve_transport(),
        // The TS stub turns the guard off; `Off` says it in one word.
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

// ── the input, built once so the run and the allowlist cannot drift ──────────

/// The command harness's input, with the fields the runner and the allowlist
/// both read.
fn input(instruction: &str) -> FocusCommandInput {
    FocusCommandInput {
        item: item(),
        instruction: instruction.into(),
        history: Vec::new(),
        mode: FocusCommandMode::Normal,
        // No regex matches this instruction, so the ONLY thing that can put
        // `approve_task` in front of the model is the probe fact.
        deterministic_action_id: None,
        role: FocusSeatRole::Orchestrator,
        specialist: Value::Null,
    }
}

fn item() -> FocusHarnessItem {
    FocusHarnessItem {
        key: "task:t1".into(),
        question: "What next?".into(),
        source_href: "/app/boards/b1?task=t1".into(),
        evidence: Vec::new(),
        metadata: json!({}),
        actions: vec![
            FocusAction {
                id: "approve_task".into(),
                label: "Approve".into(),
                risk: "safe".into(),
                confirmation_required: false,
                reversible: true,
            },
            FocusAction {
                id: "request_changes".into(),
                label: "Request changes".into(),
                risk: "safe".into(),
                confirmation_required: false,
                reversible: true,
            },
        ],
    }
}

// ── the world ────────────────────────────────────────────────────────────────

fn live_config() -> Option<Config> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    Config::from_parts(
        url,
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6399".into()),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        std::env::var("AUTH_SECRET").unwrap_or_default(),
        String::new(),
    )
    .ok()
}

/// The TS's `beforeEach(store.clear())`, scoped to the one fabricated key —
/// the live row belongs to everyone.
async fn reset(pg: &sqlx::PgPool, key: &str) {
    forget_capabilities(pg, key)
        .await
        .expect("the standing facts clear");
}

async fn record(pg: &sqlx::PgPool, key: &str, cap: &str, fact: CapabilityFact) {
    merge_capabilities(pg, &[(key.to_string(), vec![(cap.to_string(), fact)])])
        .await
        .expect("the fact records");
}

// ── the chain ────────────────────────────────────────────────────────────────

/// A tool call the transport reported becomes a widened Inbox surface: the
/// probe writes the fact, the runner widens on it, the allowlist admits the
/// action.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reported_tool_call_becomes_a_widened_inbox_surface() {
    let Some(cfg) = live_config() else {
        panic!("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    };
    let state = AppState::new(talaria_api::db::pool(&cfg), Arc::new(cfg));
    let key = capability_key("pl-main", "qwen3-14b");
    reset(&state.pg, &key).await;

    // run_probes with the scripted tool transport under the REAL runner: the
    // ask goes through run_harness, the definitions reach the transport, and
    // the report carries the calls the watcher saw.
    let mut deps = default_deps(&state, "qwen3-14b");
    deps.ask_with_tools = runner_tool_ask(&state, "qwen3-14b", tool_transport());
    deps.offers_tool_definitions = Arc::new(|| Box::pin(async move { true }));
    // probe_keys derives keys from llm_endpoints, where the fabricated pair
    // does not live — the TS mocked routingFor for the same reason.
    deps.keys = {
        let key = key.clone();
        Arc::new(move || {
            let key = key.clone();
            Box::pin(async move { vec![key.clone()] })
        })
    };
    let report = run_probes(
        &state,
        "qwen3-14b",
        ProbeOpts {
            ids: Some(vec![ProbeId::ToolSelect]),
            deps: Some(deps),
            timeout_ms: None,
            reprobe: false,
        },
    )
    .await
    .expect("the probe run settles");
    assert_eq!(report.wrote, 1, "one probe fact, written");

    // The fact is in the real store, with the real provenance.
    let facts = get_capabilities(&state.pg, &key).await;
    let fact = facts
        .get("tool-select")
        .expect("the tool-select fact round-trips");
    assert!(fact.value);
    assert_eq!(fact.source, "probe");
    assert_eq!(fact.score, Some(1.0));

    // ...and the runner widens on it, reading the same store.
    let at = "2026-01-01T12:00:00.000Z";
    record(
        &state.pg,
        &key,
        "instruction-following",
        CapabilityFact {
            value: true,
            source: "probe".into(),
            at: at.into(),
            detail: None,
            score: Some(1.0),
        },
    )
    .await;
    let value = serde_json::to_value(input("what do you make of this?")).unwrap();
    let ctx = RunContext {
        caller: "test:reconcile".into(),
        model: Some("qwen3-14b".into()),
        deps: Some(Arc::new(harness_deps(&state.pg))),
        ..RunContext::default()
    };
    let result = run_harness(&state, &inbox_command_harness(), &value, ctx)
        .await
        .expect("the harness run settles");
    assert!(result.widened, "the runner widened on the probe fact");
    assert!(
        allowed_focus_action_ids(&input("x"), true).contains(&"approve_task".to_string()),
        "the widened allowlist admits the action"
    );

    reset(&state.pg, &key).await;
}

/// The gate reads `source`: a DECLARED fact — a claim — never widens the
/// surface; only the probe's measurement does.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_declared_fact_never_widens_only_the_probe_one_does() {
    let Some(cfg) = live_config() else {
        panic!("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    };
    let state = AppState::new(talaria_api::db::pool(&cfg), Arc::new(cfg));
    let key = capability_key("pl-main", "qwen3-14b");
    reset(&state.pg, &key).await;

    let at = "2026-01-01T12:00:00.000Z";
    for cap in ["tool-select", "instruction-following"] {
        record(
            &state.pg,
            &key,
            cap,
            CapabilityFact {
                value: true,
                source: "declared".into(),
                at: at.into(),
                detail: None,
                score: None,
            },
        )
        .await;
    }

    let value = serde_json::to_value(input("x")).unwrap();
    async fn wide(state: &AppState, value: &Value) -> bool {
        let ctx = RunContext {
            caller: "test:reconcile".into(),
            model: Some("qwen3-14b".into()),
            deps: Some(Arc::new(harness_deps(&state.pg))),
            ..RunContext::default()
        };
        run_harness(state, &inbox_command_harness(), value, ctx)
            .await
            .expect("the harness run settles")
            .widened
    }
    assert!(
        !wide(&state, &value).await,
        "a declared fact is a claim, not a measurement"
    );

    // The same facts, re-sourced from a probe — now they count.
    for cap in ["tool-select", "instruction-following"] {
        record(
            &state.pg,
            &key,
            cap,
            CapabilityFact {
                value: true,
                source: "probe".into(),
                at: at.into(),
                detail: None,
                score: Some(1.0),
            },
        )
        .await;
    }
    assert!(wide(&state, &value).await, "the probe fact widens");

    reset(&state.pg, &key).await;
}
