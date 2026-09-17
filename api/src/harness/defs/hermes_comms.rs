// CAN THIS MODEL BE A HERMES AGENT — over CHANNELS, specifically.
//
// The sixth of the Hermes family (see `hermes_knowledge.rs` for why the family
// exists). Knowledge, documents, governance, google and research each asked a
// model to drive one toolkit surface. Comms was the hole: four tools a teammate
// uses every hour (`list_channels`, `read_channel`, `post_to_channel`,
// `message_user`) plus `react_to_message`, and until this harness none of them
// had ever been put in front of a model by the sweep. The work-session and
// outreach harnesses touch a channel only as colour around a ticket or a
// check-in; they do not ask "did it read before it posted".
//
// WHAT IT MEASURES. Failures an org actually pays for when an agent is in
// #platform:
//
//   READS BEFORE POSTING     answering "what's the latest" from memory is how
//                            a confident wrong status lands in the feed.
//   REACT, DON'T CHATTER     a ✅ on the message is the Slack-shaped twist the
//                            product already ships (`react_to_message`); a new
//                            "lgtm" post is noise the room has to scroll past.
//   REPLIES STAY IN THREAD   `post_to_channel` takes `threadId`. A top-level
//                            reply to a threaded question forks the
//                            conversation and nobody sees it next to the ask.
//   THE ROOM, NOT A DM       "tell the team" is `post_to_channel`.
//                            `message_user` is rate-limited and wakes a person.
//   IDS COME FROM LISTINGS   production 404s on a channel name. A sandbox that
//                            accepted `#platform` would teach a call that 400s.
//
// NO MODEL GRADES A MODEL. Every fixture reads `ctx.calls` or the world the
// dry run left behind.
//
// THE SANDBOX HALF IS DECLARED, NOT EMBEDDED. `dry_run` names the five tools;
// dispatch lives in `fitness/toolbox/sandbox.rs`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fitness::toolbox::world::SandboxWorld;
use crate::harness::define::{
    CheckCtx, CheckResult, DryRunDecl, EvalBand, EvalCase, GuardDecl, HarnessDefinition, Message,
    OnFailure, Output, RenderContext, RoleFloor, define_harness,
};
use crate::harness::transport::ToolPolicy;
use crate::harness_model::ModelSpec;

// ── The shapes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HermesCommsInput {
    pub prompt: String,
}

const SYSTEM: &str = "You are a teammate in this workspace. Talk in the rooms you belong to using the tools, not from memory.\nRead a channel before you post into it. A reaction acknowledges; a new message is for something the room did not already have. Reply in a thread when the question lives in one.\nTell the room in the room. A DM is for one person who needs you now, not for a status the channel should see.\nSay plainly what you did. If a tool refuses you, report that — do not work around it silently.";

fn called(ctx: &CheckCtx, tool: &str) -> bool {
    ctx.any_call(tool)
}

fn world(ctx: &CheckCtx) -> Option<SandboxWorld> {
    ctx.world.as_ref().and_then(SandboxWorld::from_value)
}

const NO_WORLD: &str = "this fixture grades the workspace the agent left behind, and this run produced no observable world — the candidate ran its own tool loop, so the sweep saw tool names and not their effects. Bench this harness on a model the platform can drive, or rewrite the fixture to assert on the transcript alone.";

fn no_world() -> CheckResult {
    CheckResult::Gap(NO_WORLD.into())
}

fn arg_str<'a>(args: &'a Value, field: &str) -> Option<&'a str> {
    args.get(field).and_then(Value::as_str)
}

fn posted_top_level(ctx: &CheckCtx) -> bool {
    ctx.calls_of("post_to_channel")
        .iter()
        .any(|c| !c.errored && arg_str(&c.args, "threadId").is_none())
}

fn posted_in_thread(ctx: &CheckCtx, thread: &str) -> bool {
    ctx.calls_of("post_to_channel")
        .iter()
        .any(|c| !c.errored && arg_str(&c.args, "threadId") == Some(thread))
}

fn reacted_to(ctx: &CheckCtx, message_id: &str) -> bool {
    ctx.calls_of("react_to_message")
        .iter()
        .any(|c| !c.errored && arg_str(&c.args, "messageId") == Some(message_id))
}

pub struct HermesCommsFixture {
    pub name: &'static str,
    pub band: EvalBand,
    pub input: HermesCommsInput,
    pub check: fn(&str, &CheckCtx) -> CheckResult,
}

fn input(prompt: &str) -> HermesCommsInput {
    HermesCommsInput {
        prompt: prompt.into(),
    }
}

/// SIX FIXTURES, THREE BANDS.
pub fn fixtures() -> Vec<HermesCommsFixture> {
    vec![
        HermesCommsFixture {
            name: "reads the channel instead of answering from memory",
            band: EvalBand::Easy,
            input: input("What's the latest in #platform about the ledger migration?"),
            check: |_v, ctx| {
                if !called(ctx, "read_channel") && !called(ctx, "list_channels") {
                    return CheckResult::Fail(
                        "answered a question about a channel without listing or reading any".into(),
                    );
                }
                CheckResult::Pass
            },
        },
        HermesCommsFixture {
            name: "reacts instead of posting chatter",
            band: EvalBand::Easy,
            input: input(
                "Ack Priya's ledger-blocker message in #platform — just let her know you saw it.",
            ),
            check: |_v, ctx| {
                if posted_top_level(ctx) {
                    return CheckResult::Fail(
                        "posted a new message to acknowledge — a reaction is the tool for that"
                            .into(),
                    );
                }
                if !reacted_to(ctx, "msg-1") {
                    return CheckResult::Fail(
                        "never reacted to the message it was asked to acknowledge".into(),
                    );
                }
                CheckResult::Pass
            },
        },
        HermesCommsFixture {
            name: "replies stay in the thread",
            band: EvalBand::Standard,
            input: input(
                "Priya's ledger-blocker message in #platform has a thread. Reply IN THAT THREAD that we can ship without the migration.",
            ),
            check: |_v, ctx| {
                if posted_top_level(ctx) {
                    return CheckResult::Fail(
                        "posted a top-level message instead of replying in the thread — the ask lives under Priya's message"
                            .into(),
                    );
                }
                if !posted_in_thread(ctx, "msg-1") {
                    return CheckResult::Fail(
                        "never posted a reply with threadId set to the root it was asked to answer"
                            .into(),
                    );
                }
                CheckResult::Pass
            },
        },
        HermesCommsFixture {
            name: "tells the room in the room, not in a DM",
            band: EvalBand::Standard,
            input: input(
                "Tell the platform team the ledger migration is unblocked. They should all see it.",
            ),
            check: |_v, ctx| {
                if called(ctx, "message_user") {
                    return CheckResult::Fail(
                        "DMed a person about something the whole room should see — that is post_to_channel"
                            .into(),
                    );
                }
                if !called(ctx, "post_to_channel") {
                    return CheckResult::Fail(
                        "never posted to the channel it was asked to tell".into(),
                    );
                }
                CheckResult::Pass
            },
        },
        HermesCommsFixture {
            name: "uses a channel id from the listing, not the name",
            band: EvalBand::Standard,
            input: input("Read #platform and tell me what Priya said about the ledger."),
            check: |_v, ctx| {
                let reads = ctx.calls_of("read_channel");
                if reads.is_empty() {
                    return CheckResult::Fail("never read the channel it was asked about".into());
                }
                // A name is not an id — production 404s, the sandbox refuses
                // the same way, and a model that recovers lists then retries.
                let used_id = reads
                    .iter()
                    .any(|c| !c.errored && arg_str(&c.args, "channelId") == Some("ch-platform"));
                if !used_id {
                    return CheckResult::Fail(
                        "never called read_channel with the id list_channels returns (ch-platform) — a channel name 404s"
                            .into(),
                    );
                }
                CheckResult::Pass
            },
        },
        HermesCommsFixture {
            name: "does not spam DMs when asked to nudge everyone",
            band: EvalBand::Hard,
            input: input("Nudge everyone about standup — make sure Priya and Dana both know."),
            check: |_v, ctx| {
                let Some(w) = world(ctx) else {
                    return no_world();
                };
                if w.dms_sent.len() >= 2 {
                    return CheckResult::Fail(
                        "DMed every teammate about a room-wide ping — that burns the rate limit; post_to_channel is the room"
                            .into(),
                    );
                }
                if called(ctx, "post_to_channel") || w.dms_sent.len() <= 1 {
                    CheckResult::Pass
                } else {
                    CheckResult::Fail(
                        "neither posted to the room nor messaged one person — it did nothing with the ask"
                            .into(),
                    )
                }
            },
        },
    ]
}

fn eval_cases(fixtures: Vec<HermesCommsFixture>) -> Vec<EvalCase> {
    fixtures
        .into_iter()
        .map(|f| {
            let HermesCommsFixture {
                name,
                band,
                input,
                check,
            } = f;
            let input = serde_json::to_value(&input).expect("a fixture input serializes");
            EvalCase::new(
                name,
                input,
                Arc::new(move |v: &Value, ctx: &CheckCtx| {
                    match serde_json::from_value::<String>(v.clone()) {
                        Ok(s) => check(&s, ctx),
                        Err(e) => {
                            CheckResult::Fail(format!("the fixture check threw on the value: {e}"))
                        }
                    }
                }),
            )
            .band(band)
        })
        .collect()
}

pub fn hermes_comms_harness() -> HarnessDefinition {
    let mut d = define_harness(HarnessDefinition::new(
        "hermes:comms",
        "Hermes agent — channels",
        "A workspace agent talking in the rooms it belongs to, using the fleet toolkit.",
        ModelSpec {
            pin: None,
            role: None,
            chain: Some(&[]),
            user_id: None,
        },
        Arc::new(|input: &Value, _ctx: &RenderContext| {
            let hi: HermesCommsInput =
                serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
            Ok(vec![Message::system(SYSTEM), Message::user(hi.prompt)])
        }),
        Output::Text {
            clean: Some(Arc::new(|raw: &str| {
                Ok(Some(Value::String(raw.trim().to_string())))
            })),
            verify: None,
        },
        OnFailure::Null,
    ));
    d.requires = vec!["tools", "tool-select"];
    d.floor = RoleFloor::runs_anyway(
        "Any model that can call tools can be asked this; a weaker one posts chatter and DMs the room. A model that cannot call tools at all is not a candidate for any Hermes agent.",
    );
    d.guard = Some(GuardDecl {
        rules: Some(vec!["zero_tool_claim", "secret_leak", "pii_leak"]),
        redact: true,
    });
    d.tools = Some(ToolPolicy::Own);
    d.dry_run = Some({
        let mut dry = DryRunDecl::tools(vec![
            "list_channels",
            "read_channel",
            "post_to_channel",
            "react_to_message",
            "message_user",
            "list_teammates",
        ]);
        dry.max_turns = Some(8);
        dry
    });
    d.evals = eval_cases(fixtures());
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fitness::toolbox::world::base_world;
    use crate::harness::define::CheckCall;
    use serde_json::json;

    fn call(tool: &str, errored: bool, args: Value) -> CheckCall {
        CheckCall {
            tool: tool.into(),
            errored,
            args,
        }
    }

    fn dry(calls: Vec<CheckCall>, world: SandboxWorld) -> CheckCtx {
        CheckCtx {
            calls,
            world: Some(world.to_value()),
            exhausted: false,
        }
    }

    fn fixture(name: &str) -> HermesCommsFixture {
        fixtures()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no fixture named {name}"))
    }

    #[test]
    fn every_fixture_accepts_a_good_answer() {
        let w = base_world();
        let cases: Vec<(&str, &str, Vec<CheckCall>)> = vec![
            (
                "reads the channel instead of answering from memory",
                "Priya said the ledger migration is the blocker this month.",
                vec![call(
                    "read_channel",
                    false,
                    json!({ "channelId": "ch-platform" }),
                )],
            ),
            (
                "reacts instead of posting chatter",
                "Reacted ✅.",
                vec![call(
                    "react_to_message",
                    false,
                    json!({ "channelId": "ch-platform", "messageId": "msg-1", "emoji": "✅" }),
                )],
            ),
            (
                "replies stay in the thread",
                "Replied in the thread.",
                vec![call(
                    "post_to_channel",
                    false,
                    json!({ "channelId": "ch-platform", "content": "we can ship without it", "threadId": "msg-1" }),
                )],
            ),
            (
                "tells the room in the room, not in a DM",
                "Posted in #platform.",
                vec![call(
                    "post_to_channel",
                    false,
                    json!({ "channelId": "ch-platform", "content": "ledger migration is unblocked" }),
                )],
            ),
            (
                "uses a channel id from the listing, not the name",
                "Priya said the ledger migration is the blocker.",
                vec![
                    call("list_channels", false, json!({})),
                    call("read_channel", false, json!({ "channelId": "ch-platform" })),
                ],
            ),
            (
                "does not spam DMs when asked to nudge everyone",
                "Posted in #platform so both of them see it.",
                vec![call(
                    "post_to_channel",
                    false,
                    json!({ "channelId": "ch-platform", "content": "standup in 5" }),
                )],
            ),
        ];
        for (name, reply, calls) in cases {
            let f = fixture(name);
            assert_eq!(
                (f.check)(reply, &dry(calls, w.clone())),
                CheckResult::Pass,
                "{}",
                f.name
            );
        }
    }

    #[test]
    fn the_named_traps_fire_their_own_sentences() {
        let empty = CheckCtx::default();
        assert_eq!(
            (fixture("reads the channel instead of answering from memory").check)(
                "Nothing new.",
                &empty
            ),
            CheckResult::Fail(
                "answered a question about a channel without listing or reading any".into()
            )
        );
        assert_eq!(
            (fixture("reacts instead of posting chatter").check)(
                "lgtm",
                &dry(
                    vec![call(
                        "post_to_channel",
                        false,
                        json!({ "channelId": "ch-platform", "content": "lgtm" }),
                    )],
                    base_world()
                )
            ),
            CheckResult::Fail(
                "posted a new message to acknowledge — a reaction is the tool for that".into()
            )
        );
        assert_eq!(
            (fixture("replies stay in the thread").check)(
                "Replied.",
                &dry(
                    vec![call(
                        "post_to_channel",
                        false,
                        json!({ "channelId": "ch-platform", "content": "we can ship" }),
                    )],
                    base_world()
                )
            ),
            CheckResult::Fail(
                "posted a top-level message instead of replying in the thread — the ask lives under Priya's message"
                    .into()
            )
        );
        assert_eq!(
            (fixture("tells the room in the room, not in a DM").check)(
                "Told Priya.",
                &dry(
                    vec![call(
                        "message_user",
                        false,
                        json!({ "to": "Priya", "message": "unblocked" }),
                    )],
                    base_world()
                )
            ),
            CheckResult::Fail(
                "DMed a person about something the whole room should see — that is post_to_channel"
                    .into()
            )
        );
        assert_eq!(
            (fixture("uses a channel id from the listing, not the name").check)(
                "Priya said so.",
                &dry(
                    vec![call(
                        "read_channel",
                        true,
                        json!({ "channelId": "platform" }),
                    )],
                    base_world()
                )
            ),
            CheckResult::Fail(
                "never called read_channel with the id list_channels returns (ch-platform) — a channel name 404s"
                    .into()
            )
        );
        let mut spammed = base_world();
        spammed.dms_sent = vec![
            crate::fitness::toolbox::world::SandboxDm {
                user: "Priya".into(),
                body: "standup".into(),
            },
            crate::fitness::toolbox::world::SandboxDm {
                user: "Dana".into(),
                body: "standup".into(),
            },
        ];
        assert_eq!(
            (fixture("does not spam DMs when asked to nudge everyone").check)(
                "Nudged them both.",
                &dry(
                    vec![
                        call(
                            "message_user",
                            false,
                            json!({ "to": "Priya", "message": "standup" }),
                        ),
                        call(
                            "message_user",
                            false,
                            json!({ "to": "Dana", "message": "standup" }),
                        ),
                    ],
                    spammed
                )
            ),
            CheckResult::Fail(
                "DMed every teammate about a room-wide ping — that burns the rate limit; post_to_channel is the room"
                    .into()
            )
        );
    }

    #[test]
    fn ships_as_a_subject_of_the_call_hermes_harness() {
        let d = hermes_comms_harness();
        assert_eq!(d.id, "hermes:comms");
        assert!(d.model.chain.is_some_and(|c| c.is_empty()));
        assert_eq!(d.evals.len(), 6);
    }
}
