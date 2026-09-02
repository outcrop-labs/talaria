// THE RECORDED-TRANSCRIPT HARNESS: run any harness against written-down model
// replies, with no gateway, no fleet, no database and no clock.
//
// ONE FAKE WORLD, CONSTRUCTED ONCE. Every def's tests and the fitness sweep
// drive their runs through this module; a second copy of a fake is worse
// than a second copy of real code, because the assertions it supports
// quietly become assertions about the fake. run.rs's tests construct it too
// — nothing else ever grows another.
//
// WHAT IS FAKE AND WHAT IS REAL, which is the whole design:
//
//   FAKE   the transport (scripted replies), model resolution, routing, the
//          capability record, the clock, and the two recorders. Every one of
//          them is an edge that would otherwise need a service.
//
//   REAL   the parser (the runner runs it, through the harness's own schema),
//          the guard rules (`guard::gate_safe` runs the actual RULES
//          registry), and the persona resolver (`persona_keys_from` over
//          recorded agent-config rows). Faking any of those would turn an
//          assertion into a restatement of the fake: a stubbed `guard_text`
//          that returns [] makes "the runner refuses to repair a reply
//          carrying a credential" pass on a runner that does no such thing.
//
// THE CAPABILITY DEFAULT IS UNKNOWN, deliberately. An absent fact is the state
// a fresh self-host is in, and `missing_capabilities` treats unknown as
// present (capability.rs's cardinal rule: only a fact that positively says
// "no" counts as missing). A helper that defaulted to "everything works"
// would hide every floor refusal, which is the behavior most worth testing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Map;

use super::run::{
    DeltaFn, HarnessDeps, HarnessResult, HarnessRunRow, RunContext, StreamFn, StreamOptions,
};
use super::transport::{TransportKind, TransportReply, TransportRequest};
use crate::capability::{CapabilityFact, capability_key};
use crate::capability_reach::{Reach, ReachVia, Supplier};
use crate::gateway::guard::{self, Finding, GuardConfig, GuardMode};
use crate::harness_model::ModelChainStep;
use crate::persona::{PersonaRow, persona_index};

/// What the transport hands back, in order. THE LAST ONE REPEATS, and the
/// repeat is load-bearing rather than convenient: a JSON harness that repairs
/// sends a second call, and a case that supplied one bad reply means "this
/// model always answers like this", not "this model answers badly once and
/// then a fixture runs out". A `Full` reply instead of a `Text` one is how a
/// case says something about the CALL — tool names from a persona, a usage
/// record, or `contract_dropped` for the silent-strip case.
#[derive(Clone)]
pub enum RecordedReply {
    Text(String),
    Full(TransportReply),
}

pub fn replies(v: &[&str]) -> Vec<RecordedReply> {
    v.iter()
        .map(|s| RecordedReply::Text((*s).to_string()))
        .collect()
}

/// What the model chain resolves to. `NoModel` means NOTHING routes, which is the
/// state of an install that has never named an endpoint and is a real branch
/// in the runner; `Default` (the field left unset) means a pinned-looking
/// `pl-main`.
pub enum RecordedModel {
    Default,
    NoModel,
    Resolved(String, ModelChainStep),
}

/// A capability fact as a case writes it: a deliberate measurement plus its
/// source, because `learned` (what the gateway writes off a 400) behaves
/// differently at the floor than `probe` does.
#[derive(Clone, Copy)]
pub struct RecordedFact {
    pub value: bool,
    pub source: &'static str,
}

/// The source most cases mean: Talaria measured the model itself.
pub fn probe(value: bool) -> RecordedFact {
    RecordedFact {
        value,
        source: "probe",
    }
}

pub fn sourced(value: bool, source: &'static str) -> RecordedFact {
    RecordedFact { value, source }
}

/// Capability facts keyed by full 'endpoint:model' key or bare endpoint name
/// (the shorthand most cases want). Absent = UNKNOWN — see the header.
pub fn facts(
    specs: &[(&str, &str, RecordedFact)],
) -> HashMap<String, HashMap<String, RecordedFact>> {
    let mut out: HashMap<String, HashMap<String, RecordedFact>> = HashMap::new();
    for (key, cap, spec) in specs {
        out.entry((*key).to_string())
            .or_default()
            .insert((*cap).to_string(), *spec);
    }
    out
}

/// The world a recorded run runs against.
pub struct RecordedWorld {
    pub replies: Vec<RecordedReply>,
    pub model: RecordedModel,
    /// Endpoints the model can land on — the capability keys are built from
    /// these. `None` means one, or none when the case supplies personas (a
    /// persona is not in the gateway catalog, so routing finds no endpoints
    /// for it, and that is precisely the condition capability keys used to be
    /// empty in).
    pub endpoints: Option<Vec<String>>,
    pub facts: HashMap<String, HashMap<String, RecordedFact>>,
    pub guard_mode: GuardMode,
    pub policed_hosts: Vec<String>,
    /// Agent-version rows the REAL persona resolver reads. Supplying them is
    /// how a case tests tier resolution, which is the only question there.
    pub personas: Vec<PersonaRow>,
    /// The config lookup dies (the database is down mid-run). The
    /// `persona_keys` edge has no error channel — by design, because the REAL
    /// edge degrades to no keys rather than failing the run — so the throw is
    /// simulated as its landing state.
    pub personas_throw: bool,
    /// The transport refuses rather than answers — the outage posture.
    pub transport_error: Option<String>,
    /// The one caller-side routing override: no endpoints, no upstream.
    pub empty_routing: bool,
    /// Set to record every model id the capability index was asked about —
    /// how a case asserts WHICH model the runner consulted, which no other
    /// edge can say.
    pub persona_asks: Option<Arc<Mutex<Vec<String>>>>,
    /// Registered tools standing in for a capability, by capability name —
    /// the `reach` edge's answer for a floor's `suppliable` question. Empty
    /// means nothing in the install reaches anything, which is the posture a
    /// refusal test wants. The supplier names are opaque to the runner.
    pub reach: HashMap<String, Supplier>,
    /// How much the fake clock advances per reading, in ms. Every latency a
    /// recorded run reports is a multiple of this, so a test asserts on a
    /// number it chose rather than on how fast the machine happened to be.
    pub tick: i64,
}

impl Default for RecordedWorld {
    fn default() -> Self {
        RecordedWorld {
            // The default: a reply the judge-shaped test defs accept, so
            // `RecordedWorld::default()` is a WORKING run and a case opts
            // into failure by naming replies — cases here always run real
            // defs, so the working-run default is the useful one.
            replies: replies(&["{\"verdict\":\"pass\",\"summary\":\"looks right\"}"]),
            model: RecordedModel::Default,
            endpoints: None,
            facts: HashMap::new(),
            guard_mode: GuardMode::Observe,
            policed_hosts: Vec::new(),
            personas: Vec::new(),
            personas_throw: false,
            transport_error: None,
            empty_routing: false,
            persona_asks: None,
            reach: HashMap::new(),
            tick: 7,
        }
    }
}

/// Everything a recorded run lets a caller inspect afterwards.
pub struct RecordedRun {
    /// Every request the runner made, in order — the prompt it rendered, the
    /// temperature it asked for, whether it asked for JSON at the protocol
    /// level.
    pub requests: Arc<Mutex<Vec<TransportRequest>>>,
    /// The `harness_runs` rows the runner tried to write. This is the
    /// production ground truth the fitness page reads, so a test that asserts
    /// on it is asserting about the column an admin will actually see.
    pub runs: Arc<Mutex<Vec<HarnessRunRow>>>,
    /// The findings the runner tried to file. Grounded ones are already
    /// excluded by the runner, exactly as `record_findings` excludes them.
    pub findings: Arc<Mutex<Vec<Finding>>>,
    world: Arc<RecordedWorld>,
    clock: Arc<Mutex<i64>>,
}

/// The real resolver over recorded agent-version rows — the same fold the
/// `persona_keys` edge's `persona_capability_keys` runs, but over rows a
/// test owns.
pub fn persona_keys_from(model: &str, rows: &[PersonaRow]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for t in persona_index(rows).get(model).into_iter().flatten() {
        let key = capability_key(&t.endpoint, &t.model);
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    seen
}

/// Keys are 'endpoint:model'. A full key in `world.facts` wins; otherwise the
/// bare endpoint name applies to every model it serves.
fn facts_for(world: &RecordedWorld, key: &str) -> HashMap<String, CapabilityFact> {
    let endpoint = key.split(':').next().unwrap_or(key);
    let empty = HashMap::new();
    let known = world
        .facts
        .get(key)
        .or_else(|| world.facts.get(endpoint))
        .unwrap_or(&empty);
    known
        .iter()
        .map(|(cap, spec)| {
            (
                cap.clone(),
                CapabilityFact {
                    value: spec.value,
                    source: spec.source.to_string(),
                    // The stub IS the expiry policy (the real edge owns it),
                    // so `at` is never consulted here.
                    at: String::new(),
                    detail: None,
                    score: None,
                },
            )
        })
        .collect()
}

fn guard_config_of(world: &RecordedWorld) -> GuardConfig {
    GuardConfig {
        mode: world.guard_mode,
        checks: Map::new(),
        min_confidence: 0.5,
        policed_hosts: world.policed_hosts.clone(),
    }
}

/// Build the injected edges for one recorded run.
pub fn recorded_run(w: RecordedWorld) -> RecordedRun {
    let world = Arc::new(w);
    let requests: Arc<Mutex<Vec<TransportRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let runs: Arc<Mutex<Vec<HarnessRunRow>>> = Arc::new(Mutex::new(Vec::new()));
    let findings: Arc<Mutex<Vec<Finding>>> = Arc::new(Mutex::new(Vec::new()));
    let clock: Arc<Mutex<i64>> = Arc::new(Mutex::new(0));
    RecordedRun {
        requests,
        runs,
        findings,
        world: world.clone(),
        clock,
    }
}

impl RecordedRun {
    /// The dep set, rebuilt per call: every edge is a closure over the same
    /// immutable world, so a run and a later re-run see one recording state.
    pub fn deps(&self) -> Arc<HarnessDeps> {
        let world = self.world.clone();
        let requests = self.requests.clone();
        let runs = self.runs.clone();
        let findings = self.findings.clone();
        let clock = self.clock.clone();
        // A persona is not on the gateway catalog, so `routing` finds no
        // endpoints for it — which is precisely the condition that used to
        // leave `keys` empty.
        let endpoints = world.endpoints.clone().unwrap_or_else(|| {
            if !world.personas.is_empty() || world.personas_throw {
                Vec::new()
            } else {
                vec!["spark".to_string()]
            }
        });
        let tick = world.tick;
        Arc::new(HarnessDeps {
            resolve_model: {
                let world = world.clone();
                Arc::new(move |_spec, _user| {
                    let world = world.clone();
                    Box::pin(async move {
                        match &world.model {
                            RecordedModel::Default => Some(("pl-main".to_string(), "pin")),
                            RecordedModel::NoModel => None,
                            RecordedModel::Resolved(m, s) => Some((m.clone(), *s)),
                        }
                    })
                })
            },
            slot_effort: Arc::new(|_slot, _model| Box::pin(async { None })),
            routing: {
                let endpoints = endpoints.clone();
                let empty = world.empty_routing;
                Arc::new(move |model| {
                    let endpoints = endpoints.clone();
                    Box::pin(async move {
                        if empty {
                            (Vec::new(), String::new())
                        } else {
                            (endpoints.clone(), model)
                        }
                    })
                })
            },
            persona_keys: {
                let world = world.clone();
                Arc::new(move |model| {
                    let world = world.clone();
                    Box::pin(async move {
                        if let Some(asks) = &world.persona_asks {
                            asks.lock().expect("persona asks").push(model.clone());
                            return Vec::new();
                        }
                        if world.personas_throw {
                            return Vec::new();
                        }
                        persona_keys_from(&model, &world.personas)
                    })
                })
            },
            // capability.rs's cardinal rule, exactly: only a fact that
            // positively says "no" counts as missing. Unknown is not
            // missing.
            missing_capabilities: {
                let world = world.clone();
                Arc::new(move |key, asked| {
                    let world = world.clone();
                    Box::pin(async move {
                        let known = facts_for(&world, &key);
                        asked
                            .iter()
                            .filter(|cap| {
                                known.get(cap.as_str()).map(|f| !f.value).unwrap_or(false)
                            })
                            .cloned()
                            .collect()
                    })
                })
            },
            capabilities: {
                let world = world.clone();
                Arc::new(move |key| {
                    let world = world.clone();
                    Box::pin(async move { facts_for(&world, &key) })
                })
            },
            reach: {
                let world = world.clone();
                Arc::new(move |_keys, wanted| {
                    let world = world.clone();
                    Box::pin(async move {
                        wanted
                            .iter()
                            .filter_map(|cap| {
                                world.reach.get(cap).map(|s| {
                                    (
                                        cap.clone(),
                                        Reach {
                                            capability: cap.clone(),
                                            reached: true,
                                            via: Some(ReachVia::Tool),
                                            supplier: Some(s.clone()),
                                            detail: String::new(),
                                        },
                                    )
                                })
                            })
                            .collect()
                    })
                })
            },
            transport: {
                let world = world.clone();
                Arc::new(move |req| {
                    let world = world.clone();
                    let requests = requests.clone();
                    Box::pin(async move {
                        if let Some(err) = &world.transport_error {
                            return Err(err.clone());
                        }
                        let n = {
                            let mut r = requests.lock().expect("requests");
                            r.push(req);
                            r.len()
                        };
                        let reply = world
                            .replies
                            .get(n.saturating_sub(1))
                            .or_else(|| world.replies.last());
                        let text_of = |t: &str| TransportReply {
                            kind: TransportKind::Gateway,
                            text: t.to_string(),
                            tool_names: Vec::new(),
                            tool_calls: None,
                            usage: None,
                            contract_dropped: false,
                        };
                        match reply {
                            Some(RecordedReply::Full(r)) => Ok(r.clone()),
                            Some(RecordedReply::Text(t)) => Ok(text_of(t)),
                            None => Ok(text_of("")),
                        }
                    })
                })
            },
            guard_config: {
                let world = world.clone();
                Arc::new(move || {
                    let world = world.clone();
                    Box::pin(async move { Some(guard_config_of(&world)) })
                })
            },
            // The REAL rules registry — `guard::gate_safe`, not a stub.
            guard_text: {
                let world = world.clone();
                Arc::new(move |text, input| {
                    let world = world.clone();
                    Box::pin(async move {
                        let config = guard_config_of(&world);
                        guard::gate_safe(&config, &text, input.as_deref())
                    })
                })
            },
            record_findings: {
                let findings = findings.clone();
                Arc::new(move |hits, _meta| {
                    let findings = findings.clone();
                    Box::pin(async move {
                        findings.lock().expect("findings").extend(hits);
                    })
                })
            },
            record_run: {
                let runs = runs.clone();
                Arc::new(move |row| {
                    let runs = runs.clone();
                    Box::pin(async move {
                        runs.lock().expect("runs").push(row);
                    })
                })
            },
            now: {
                let clock = clock.clone();
                Arc::new(move || {
                    let mut c = clock.lock().expect("clock");
                    *c += tick;
                    *c
                })
            },
        })
    }

    /// A `RunContext` wired to this world's deps. The caller names itself.
    pub fn ctx(&self, caller: &str) -> RunContext {
        RunContext {
            caller: caller.into(),
            deps: Some(self.deps()),
            ..Default::default()
        }
    }

    pub fn n_requests(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }

    pub fn req_at(&self, i: usize) -> TransportRequest {
        self.requests.lock().expect("requests")[i].clone()
    }

    pub fn n_runs(&self) -> usize {
        self.runs.lock().expect("runs").len()
    }

    pub fn run_at(&self, i: usize) -> HarnessRunRow {
        self.runs.lock().expect("runs")[i].clone()
    }

    pub fn findings(&self) -> Vec<Finding> {
        self.findings.lock().expect("findings").clone()
    }
}

/// The check ids a result's findings carry — the vocabulary nearly every
/// guard assertion in the def corpus spells.
pub fn checks(res: &HarnessResult) -> Vec<String> {
    res.findings.iter().map(|f| f.check.clone()).collect()
}

/// A transport that pumps deltas and never assembles the text itself — the
/// honest shape of a route that pipes chunks straight into a Response.
pub fn pump(deltas: Vec<&'static str>) -> StreamFn {
    Arc::new(move |_req, emit| {
        let deltas = deltas.clone();
        Box::pin(async move {
            for d in &deltas {
                emit(d);
            }
            Ok(TransportReply {
                kind: TransportKind::Gateway,
                text: String::new(),
                tool_names: Vec::new(),
                tool_calls: None,
                usage: None,
                contract_dropped: false,
            })
        })
    })
}

/// A `StreamOptions` over `pump`, with a sink for the deltas.
pub fn streamed_of(deltas: Vec<&'static str>, on_delta: Option<DeltaFn>) -> StreamOptions {
    StreamOptions {
        stream: pump(deltas),
        on_delta,
    }
}
