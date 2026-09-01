// THE DRY RUN — a harness turn with a real tool loop, against a sandbox Talaria.
//
// WHAT IT REPLACES. Three harnesses declare `tools: 'own'` because the tool loop
// IS the feature: an agent working a ticket, a check-in that acts through
// `message_user`, a briefing follow-up the owner expects answered from live
// data. On a fleet persona that loop runs inside the agent container, where the
// platform can see tool NAMES and nothing else. On the org gateway there is no
// loop at all, so the sweep recorded a refusal — a model that was never asked a
// question scoring 0%.
//
// Neither of those measures the thing an admin needs to know, which is not "can
// this model emit a tool call" (the probes answer that in four prompts) but
// "does this model WORK LIKE A COLLEAGUE HERE": read before you write, move the
// status while you work, report an outcome only for something you verified,
// never claim a tool result you did not get. Those are properties of a
// TRANSCRIPT, and this file produces one.
//
// HOW IT DIFFERS FROM PRODUCTION, stated plainly because a benchmark that
// quietly diverges from the thing it predicts is worse than no benchmark:
//
//   the loop is OURS, not the persona's. Production personas run Hermes's loop;
//     this runs a minimal one. What that changes is scaffolding — retries,
//     parallel calls, the agent's own system prompt — and what it preserves is
//     the decision under test: given these tools and this situation, what did
//     the model choose to do.
//   the tools are SANDBOXED. Same names, same descriptions (locked to
//     `mcp/src/index.ts` by a sync test), backends that mutate an in-memory
//     world. A model cannot tell the difference from inside the call.
//   the world is SMALL. Three tickets, one channel, two teammates. Big enough
//     to pose the question, small enough that a fixture's assertions are
//     readable.
//
// IT IS NOT A JUDGE. Nothing here asks a model to grade a model. Every fixture
// that consumes a dry run asserts over the sandbox's call log — a list of what
// happened — with plain code.

use std::sync::{Arc, Mutex};

use crate::harness::define::Message;
use crate::harness::run::TransportFn;
use crate::harness::transport::{
    TokenPair, ToolDefinition, ToolPolicy, TransportKind, TransportReply, TransportRequest,
    tool_call_id_of,
};

use super::credential_tools::CredentialSandbox;
use super::hermes_tools::WorkbenchSandbox;
use super::sandbox::{DispatchResult, Sandbox};

/// WHAT THE LOOP NEEDS OF A SANDBOX, and no more.
///
/// There are three — `sandbox.rs` (Talaria's toolkit over an in-memory world),
/// `hermes_tools.rs` (files and a test runner) and `credential_tools.rs` (a
/// credential surface) — and the loop cares about none of their worlds: it
/// offers whatever tools the sandbox declares and hands every call to
/// `dispatch`. Typed as this narrow trait so a fourth surface needs no edit
/// here at all.
pub trait DispatchSandbox: Send {
    fn tools(&self) -> Vec<ToolDefinition>;
    fn dispatch(&mut self, name: &str, args_json: &str) -> DispatchResult;
}

impl DispatchSandbox for Sandbox {
    fn tools(&self) -> Vec<ToolDefinition> {
        Sandbox::tool_definitions(self)
    }
    fn dispatch(&mut self, name: &str, args_json: &str) -> DispatchResult {
        Sandbox::dispatch(self, name, args_json)
    }
}

impl DispatchSandbox for WorkbenchSandbox {
    fn tools(&self) -> Vec<ToolDefinition> {
        WorkbenchSandbox::tools(self)
    }
    fn dispatch(&mut self, name: &str, args_json: &str) -> DispatchResult {
        WorkbenchSandbox::dispatch(self, name, args_json)
    }
}

impl DispatchSandbox for CredentialSandbox {
    fn tools(&self) -> Vec<ToolDefinition> {
        CredentialSandbox::tools(self)
    }
    fn dispatch(&mut self, name: &str, args_json: &str) -> DispatchResult {
        CredentialSandbox::dispatch(self, name, args_json)
    }
}

/// How many model turns one dry run may take before the loop gives up.
///
/// SIX IS A WORKING SESSION, not an agent's whole life. Production work sessions
/// run to twelve turns; a benchmark case poses one situation, and a model still
/// circling after six has answered the question being asked of it. The bound is
/// also what keeps a sweep affordable: an unbounded loop on a chatty model is a
/// case that costs more than the other twenty-five harnesses combined.
///
/// BRIEFLY RAISED TO TEN AND PUT BACK. The workbench failures that prompted it
/// turned out to be two bugs in the loop below — a last-turn tool call thrown
/// away undispatched, and an invented `[tool] name(args)` transcript syntax the
/// model then imitated as prose. Both are fixed and tested. Raising the budget
/// was reasoning about a task being too hard, with no measurement behind it, and
/// it would have masked exactly the defects that were really there. If six turns
/// is genuinely too few, the evidence for that is a sweep where models exhaust
/// the loop with the bugs gone — not an argument.
pub const MAX_TURNS: usize = 6;

/// THE BUDGET A HARNESS MAY ASK FOR INSTEAD, when six is not what production
/// gives it. `work-session` runs to `MAX_SESSION_TURNS` (twelve) in production,
/// so benching it at six measures a shorter job than the one it does — and a
/// model cut off mid-work is then judged on unfinished work.
///
/// Capped, because an unbounded loop on a chatty model is a case that costs more
/// than the other twenty-five harnesses combined.
pub const MAX_TURN_CEILING: usize = 12;

pub fn turn_budget(asked: Option<u32>) -> usize {
    asked
        .unwrap_or(MAX_TURNS as u32)
        .clamp(1, MAX_TURN_CEILING as u32) as usize
}

/// Tool calls honored per turn. A model that asks for nine things at once gets
/// the first three answered and can ask again — which is the same back-pressure
/// a real gateway applies, and stops one turn from emptying the budget.
pub const MAX_CALLS_PER_TURN: usize = 3;

pub struct DryRunResult {
    /// The model's LAST reply text — what a caller would have been handed.
    pub text: String,
    /// Model turns actually taken.
    pub turns: usize,
    /// The loop hit the turn budget with the model still calling tools. Not a
    /// failure by itself; a fixture decides whether it is one.
    pub exhausted: bool,
    /// The whole conversation, for the drill-down.
    pub messages: Vec<Message>,
}

/// A TRANSPORT THAT RUNS THE LOOP, so `run_harness` needs no special case.
///
/// The runner asks its transport for one reply and gets one — after however
/// many model turns and tool calls it took to produce it. Everything the
/// runner does around that (the guard pass, `harness_runs`, the contract, the
/// repair turn) is unchanged, which is the property that makes a dry run
/// comparable to the single-shot cases beside it in the same sweep.
///
/// `tools: 'own'` IS NEUTRALIZED ON PURPOSE. The harness declares it because
/// production wants the model's own loop; here the platform IS the loop, so the
/// request goes down with tool DEFINITIONS instead and `tools: 'none'` — which
/// is the honest description of what is being sent, and stops the gateway
/// transport refusing a call this file is about to run itself.
///
/// THE SANDBOX RIDES BEHIND A MUTEX because a transport is an `Fn`: the type
/// allows a second call, and TS's closure was equally free to keep mutating
/// its captured sandbox. One dry run = one call in practice; the lock is the
/// type system's price for that honesty, not a concurrency claim.
///
/// THE CALLER HANDS THE SAME ARC IN, rather than a boxed sandbox, because the
/// sweep needs to read the call log and the world AFTER the loop has finished
/// — `Sandbox.calls` is what a behavioural fixture asserts over, and a value
/// only the transport could reach would make the fixture's half of the dry run
/// invisible to the code that scores it.
pub fn sandbox_transport<S: DispatchSandbox + 'static>(
    sandbox: Arc<Mutex<S>>,
    base: TransportFn,
    out: Option<Arc<Mutex<Option<DryRunResult>>>>,
    max_turns: usize,
) -> TransportFn {
    Arc::new(move |req: TransportRequest| {
        let base = base.clone();
        let out = out.clone();
        // CLONED PER CALL, an `Arc` bump: an `Fn` closure cannot move its
        // captures into the future, and a transport is `Fn` because the type
        // allows a second call.
        let sandbox = sandbox.clone();
        Box::pin(async move {
            // THE GUARD NEVER CROSSES AN `await`. The future has to be `Send`
            // (that is `BoxFut`), and a `MutexGuard` is not — so the lock is
            // taken for the two synchronous things the loop needs (the tool
            // definitions once, each dispatch as it happens) and dropped
            // before the model is asked anything. Dispatch is sync, so this
            // costs nothing; holding it across the call would be the bug.
            let tool_defs = {
                let sandbox = sandbox.lock().expect("the dry-run loop is not reentrant");
                sandbox.tools()
            };
            let mut convo: Vec<Message> = req.messages.clone();

            let mut text = String::new();
            let mut turns = 0usize;
            let mut exhausted = false;
            let mut usage: Option<TokenPair> = None;
            let mut names: Vec<String> = vec![];

            for turn in 0..max_turns {
                turns += 1;
                let mut turn_req = req.clone();
                turn_req.messages = convo.clone();
                turn_req.tools = Some(ToolPolicy::None);
                turn_req.tool_defs = tool_defs.clone();
                let reply = base(turn_req).await?;
                text = reply.text.clone();
                if let Some(u) = reply.usage {
                    usage = Some(match usage {
                        Some(acc) => TokenPair {
                            prompt_tokens: acc.prompt_tokens + u.prompt_tokens,
                            completion_tokens: acc.completion_tokens + u.completion_tokens,
                        },
                        None => u,
                    });
                }

                let calls: Vec<_> = reply
                    .tool_calls
                    .unwrap_or_default()
                    .into_iter()
                    .take(MAX_CALLS_PER_TURN)
                    .collect();
                if calls.is_empty() {
                    break;
                }

                // THE MODEL'S OWN TURN GOES INTO THE TRANSCRIPT FIRST, including
                // any prose it wrote alongside the calls. Dropping it would hide
                // the exact failure this suite exists to catch — a model
                // narrating work it then did not do.
                //
                // ITS CALLS GO IN THE TOOL CHANNEL, not into its own prose.
                // Models imitate whatever string they are shown (two wordings
                // failed before this one), and the shape providers speak is the
                // only one they are trained on.
                convo.push(Message {
                    role: crate::harness::define::Role::Assistant,
                    content: reply.text.clone(),
                    tool_calls: calls.clone(),
                    tool_call_id: None,
                });

                // DISPATCHED EVEN ON THE LAST TURN, and this was half of the
                // bug that once justified raising the budget. The budget bounds
                // how many times the model gets to THINK; it must never discard
                // what the model already decided to do.
                for (i, call) in calls.iter().enumerate() {
                    names.push(call.name.clone());
                    let res = {
                        let mut s = sandbox.lock().expect("the dry-run loop is not reentrant");
                        s.dispatch(&call.name, &call.args)
                    };
                    // `i` is the call's index in the assistant message above,
                    // which is what `tool_call_id_of` numbers from. The cap is
                    // TS's `.slice(0, 8_000)`, cut at a scalar boundary — the
                    // one place a Rust string can be cut without manufacturing
                    // a broken code point.
                    convo.push(Message {
                        role: crate::harness::define::Role::Tool,
                        content: res.text.chars().take(8_000).collect(),
                        tool_calls: Vec::new(),
                        tool_call_id: Some(tool_call_id_of(call, i)),
                    });
                }

                if turn == max_turns - 1 {
                    exhausted = true;
                    break;
                }
            }

            // A LAST TURN TO ANSWER IN. A run that ends still holding tools
            // ends on a turn that usually says nothing (`clean` is
            // `raw.trim() || null`, so the harness would see no value at all
            // and fail the CONTRACT), so it gets one more call with the tools
            // taken away and its results in front of it: the question stops
            // being "what next" and becomes "so what happened".
            //
            // WHAT THE CLOSING TURN ASKS DEPENDS ON WHY THE LOOP ENDED. An
            // exhausted run is asked what it DID, which it can always answer;
            // a run that simply stopped calling tools is asked to summarise,
            // which is what the harness prompt promised it would be asked.
            // (The first version always asked for the summary, and a model
            // that had NOT finished answered it correctly — then was scored as
            // though the mis-sequencing had been real.)
            if text.trim().is_empty() && !names.is_empty() {
                let closing_ask = if exhausted {
                    "You have run out of turns for this session. Do not call any more tools. In one short line, say what you changed and what is still left to do."
                } else {
                    "Stop here and reply with the short summary the instructions asked for. Do not call any more tools."
                };
                turns += 1;
                let mut closing_req = req.clone();
                closing_req.messages = convo.clone();
                closing_req.messages.push(Message::user(closing_ask));
                closing_req.tools = Some(ToolPolicy::None);
                let closing = base(closing_req).await?;
                text = closing.text;
                if let Some(u) = closing.usage {
                    usage = Some(match usage {
                        Some(acc) => TokenPair {
                            prompt_tokens: acc.prompt_tokens + u.prompt_tokens,
                            completion_tokens: acc.completion_tokens + u.completion_tokens,
                        },
                        None => u,
                    });
                }
            }

            if let Some(out) = out {
                *out.lock().expect("the out slot is not contended") = Some(DryRunResult {
                    text: text.clone(),
                    turns,
                    exhausted,
                    messages: convo.clone(),
                });
            }
            Ok(TransportReply {
                kind: TransportKind::Gateway,
                text,
                tool_names: names,
                tool_calls: None,
                usage,
                contract_dropped: false,
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::transport::{ToolCall, ToolPolicy};
    use serde_json::json;

    /// A base transport scripted per turn: each entry is (text, tool calls).
    /// Everything the loop does is observable through what it sends DOWN and
    /// what it hands back UP, so a script of replies is a complete test.
    fn scripted(
        script: Vec<(String, Vec<ToolCall>)>,
    ) -> (TransportFn, Arc<Mutex<Vec<TransportRequest>>>) {
        let seen: Arc<Mutex<Vec<TransportRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let step = Arc::new(Mutex::new(0usize));
        let seen_clone = seen.clone();
        let tf: TransportFn = Arc::new(move |req: TransportRequest| {
            seen_clone.lock().unwrap().push(req.clone());
            let i = {
                let mut s = step.lock().unwrap();
                let i = *s;
                *s += 1;
                i
            };
            let (text, calls) = script
                .get(i)
                .cloned()
                .unwrap_or_else(|| (String::new(), vec![]));
            let reply = TransportReply {
                kind: TransportKind::Gateway,
                text,
                tool_names: calls.iter().map(|c| c.name.clone()).collect(),
                tool_calls: if script.len() > i { Some(calls) } else { None },
                usage: Some(TokenPair {
                    prompt_tokens: 10,
                    completion_tokens: 2,
                }),
                contract_dropped: false,
            };
            Box::pin(async move { Ok(reply) })
        });
        (tf, seen)
    }

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            name: name.into(),
            id: None,
            args: args.into(),
        }
    }

    fn workbench() -> std::sync::Arc<std::sync::Mutex<WorkbenchSandbox>> {
        use crate::harness::define::{WorkspaceFile, WorkspaceSpec};
        std::sync::Arc::new(std::sync::Mutex::new(WorkbenchSandbox::new(
            WorkspaceSpec {
                files: vec![WorkspaceFile {
                    path: "a.ts".into(),
                    content: "broken".into(),
                }],
                passes: Arc::new(|files: &[WorkspaceFile]| {
                    files.iter().find(|f| f.path == "a.ts").and_then(|f| {
                        if f.content == "fixed" {
                            None
                        } else {
                            Some("still broken".into())
                        }
                    })
                }),
            },
        )))
    }

    fn req() -> TransportRequest {
        TransportRequest {
            model: "test-model".into(),
            messages: vec![Message::user("fix it")],
            temperature: None,
            json_mode: false,
            json_schema: None,
            tools: Some(ToolPolicy::Own),
            tool_defs: vec![],
            ledger: None,
            effort: None,
            hold_ms: None,
            caller: "fitness::test".into(),
        }
    }

    #[test]
    fn the_turn_budget_clamps_asked_values() {
        assert_eq!(turn_budget(None), 6);
        assert_eq!(turn_budget(Some(12)), 12);
        assert_eq!(turn_budget(Some(99)), 12, "the ceiling holds");
        assert_eq!(turn_budget(Some(0)), 1, "at least one turn");
    }

    #[tokio::test]
    async fn a_one_turn_answer_with_no_calls_comes_straight_back() {
        let (base, seen) = scripted(vec![("done, nothing to do".into(), vec![])]);
        let out = Arc::new(Mutex::new(None));
        let transport = sandbox_transport(workbench(), base, Some(out.clone()), MAX_TURNS);
        let reply = transport(req()).await.expect("the scripted base answers");
        assert_eq!(reply.text, "done, nothing to do");
        assert!(reply.tool_names.is_empty());
        let result = out.lock().unwrap().take().unwrap();
        assert_eq!(result.turns, 1);
        assert!(!result.exhausted);
        // The one downstream request carries the sandbox's tool defs and the
        // policy flattened to 'none' — 'own' is neutralized on purpose.
        let down = &seen.lock().unwrap()[0];
        assert_eq!(down.tool_defs.len(), 5);
        assert_eq!(down.tools, Some(ToolPolicy::None));
    }

    #[tokio::test]
    async fn tool_calls_are_dispatched_and_their_results_returned_in_the_tool_channel() {
        let (base, seen) = scripted(vec![
            (
                "reading first".into(),
                vec![
                    call("read_file", r#"{"path":"a.ts"}"#),
                    call("write_file", r#"{"path":"a.ts","content":"fixed"}"#),
                ],
            ),
            ("fixed it".into(), vec![]),
        ]);
        let out = Arc::new(Mutex::new(None));
        let transport = sandbox_transport(workbench(), base, Some(out.clone()), MAX_TURNS);
        let reply = transport(req()).await.expect("the scripted base answers");
        assert_eq!(reply.text, "fixed it");
        assert_eq!(
            reply.tool_names,
            vec!["read_file".to_string(), "write_file".to_string()]
        );
        assert!(
            reply.usage.unwrap().prompt_tokens >= 20,
            "usage accumulates across turns"
        );
        let result = out.lock().unwrap().take().unwrap();
        assert_eq!(result.turns, 2);
        assert!(!result.exhausted);
        // The second downstream request carries the transcript: user, assistant
        // with its two calls, and a tool message per call with correlated ids.
        let down = &seen.lock().unwrap()[1];
        assert_eq!(down.messages.len(), 4);
        assert_eq!(down.messages[1].tool_calls.len(), 2);
        assert_eq!(down.messages[2].tool_call_id.as_deref(), Some("call_0"));
        assert_eq!(down.messages[3].tool_call_id.as_deref(), Some("call_1"));
        assert!(
            down.messages[3].content.contains("still broken")
                || down.messages[2].content.contains("broken")
        );
    }

    #[tokio::test]
    async fn a_budget_exhausted_with_the_model_still_calling_gets_the_what_did_you_do_turn() {
        // Every scripted turn calls a tool, so the loop exhausts; every turn's
        // text is empty, so the closing ask fires.
        let (base, seen) = scripted(vec![(String::new(), vec![call("run_tests", "{}")]); 8]);
        let out = Arc::new(Mutex::new(None));
        let transport = sandbox_transport(workbench(), base, Some(out.clone()), 3);
        let reply = transport(req()).await.expect("the scripted base answers");
        assert!(
            reply.text.is_empty(),
            "the scripted closing reply is empty text"
        );
        assert_eq!(
            reply.tool_names.len(),
            3,
            "one call per turn, dispatched even on the last"
        );
        let result = out.lock().unwrap().take().unwrap();
        assert!(result.exhausted);
        assert_eq!(result.turns, 4, "three loop turns plus the closing ask");
        let last = seen.lock().unwrap().last().unwrap().clone();
        assert!(
            last.messages
                .last()
                .unwrap()
                .content
                .contains("run out of turns")
        );
        assert!(
            last.messages
                .iter()
                .any(|m| m.role == crate::harness::define::Role::Tool)
        );
    }

    #[tokio::test]
    async fn an_empty_final_text_with_no_calls_at_all_gets_no_closing_turn() {
        let (base, _seen) = scripted(vec![(String::new(), vec![])]);
        let out = Arc::new(Mutex::new(None));
        let transport = sandbox_transport(workbench(), base, Some(out.clone()), MAX_TURNS);
        transport(req()).await.expect("the scripted base answers");
        let result = out.lock().unwrap().take().unwrap();
        assert_eq!(result.turns, 1);
        assert!(!result.exhausted);
    }

    #[tokio::test]
    async fn a_talaria_sandbox_rides_the_same_loop() {
        // The trait makes the loop surface-agnostic; one scripted round trip
        // against the Talaria sandbox proves the impl wires.
        let (base, _seen) = scripted(vec![
            (
                String::new(),
                vec![call("get_ticket", r#"{"taskId":"PLAT-118"}"#)],
            ),
            ("triaged".into(), vec![]),
        ]);
        let out = Arc::new(Mutex::new(None));
        let transport = sandbox_transport(
            std::sync::Arc::new(std::sync::Mutex::new(Sandbox::new(Default::default()))),
            base,
            Some(out.clone()),
            MAX_TURNS,
        );
        let reply = transport(req()).await.expect("the scripted base answers");
        assert_eq!(reply.text, "triaged");
        assert_eq!(reply.tool_names, vec!["get_ticket".to_string()]);
        let _ = out.lock().unwrap().take().unwrap();
    }

    #[tokio::test]
    async fn more_than_three_calls_in_one_turn_are_trimmed_not_refused() {
        let five: Vec<ToolCall> = (0..5)
            .map(|_| call("run_tests", "{}"))
            .chain(std::iter::once(call("read_file", r#"{"path":"a.ts"}"#)))
            .take(5)
            .collect();
        assert_eq!(five.len(), 5);
        let (base, _seen) = scripted(vec![(String::new(), five), ("done".into(), vec![])]);
        let out = Arc::new(Mutex::new(None));
        let transport = sandbox_transport(workbench(), base, Some(out.clone()), MAX_TURNS);
        let reply = transport(req()).await.expect("the scripted base answers");
        assert_eq!(
            reply.tool_names.len(),
            3,
            "the same back-pressure a gateway applies"
        );
        let _ = json!({});
    }
}
