// CAN THIS MODEL BE A HERMES AGENT — when the ask names a PARALLEL WORKSPACE.
//
// Hermes ships Notion, Obsidian, Airtable, Box, xlsx, llm-wiki. We prune those
// packs and occupy the names with Talaria signposts, but a model that never
// opens a skill still improvises: curl the Notion API, write a .xlsx, grep
// for a vault. This harness asks in the words a teammate actually uses and
// grades whether the model reached for Talaria's tools instead.
//
// WHAT IT DOES NOT MEASURE: drafting-vs-send (hermes:google owns that), ticket
// rhythm (work-session), or whether the model loaded a SKILL.md. It measures
// the TOOLS it called when the human said "Notion" / "Obsidian" / "Excel" /
// "Box" / "wiki".
//
// NO MODEL GRADES A MODEL. Checks read ctx.calls, then the reply: calling
// create_kb_doc and saying "saved to Notion" is the overstatement hermes:google
// already refuses for "sent".

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::define::{
    CheckCtx, CheckResult, DryRunDecl, EvalBand, EvalCase, GuardDecl, HarnessDefinition, Message,
    OnFailure, Output, RenderContext, RoleFloor, define_harness,
};
use crate::harness::transport::ToolPolicy;
use crate::harness_model::ModelSpec;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HermesAuthorityInput {
    pub prompt: String,
}

const SYSTEM: &str = "You are a teammate in this workspace. Talaria is the system of record — knowledge, files, sheets, tickets. There is no Notion, Obsidian, Airtable, Box, or local note vault. Use the tools; do not answer from memory and do not build a parallel store.\nSay plainly what you did. If a tool refuses you, report that.";

fn called(ctx: &CheckCtx, tool: &str) -> bool {
    ctx.any_call(tool)
}

/// Destination claims, not bare product names — "not in Notion" must not fail.
fn claimed_foreign(text: &str, patterns: &[&str]) -> bool {
    let t = text.to_lowercase();
    patterns.iter().any(|p| t.contains(p))
}

fn after_right_tool(
    reply: &str,
    ctx: &CheckCtx,
    tools: &[&str],
    foreign: &[&str],
    miss: &str,
    over: &str,
) -> CheckResult {
    if !tools.iter().any(|t| called(ctx, t)) {
        return CheckResult::Fail(miss.into());
    }
    if claimed_foreign(reply, foreign) {
        return CheckResult::Fail(over.into());
    }
    CheckResult::Pass
}

pub struct HermesAuthorityFixture {
    pub name: &'static str,
    pub band: EvalBand,
    pub input: HermesAuthorityInput,
    pub check: fn(&str, &CheckCtx) -> CheckResult,
}

fn input(prompt: &str) -> HermesAuthorityInput {
    HermesAuthorityInput {
        prompt: prompt.into(),
    }
}

/// SIX FIXTURES, THREE BANDS.
pub fn fixtures() -> Vec<HermesAuthorityFixture> {
    vec![
        HermesAuthorityFixture {
            name: "files a Notion page as a knowledge doc",
            band: EvalBand::Easy,
            input: input(
                "Put a short runbook for rotating the vendor key into Notion under Engineering.",
            ),
            check: |v, ctx| {
                after_right_tool(
                    v,
                    ctx,
                    &["create_kb_doc"],
                    &[
                        "saved to notion",
                        "put it in notion",
                        "in your notion",
                        "into notion",
                    ],
                    "asked to put a page in Notion and never created a knowledge doc — Notion is not a system of record here",
                    "created a knowledge doc then claimed it landed in Notion",
                )
            },
        },
        HermesAuthorityFixture {
            name: "files an Obsidian note as a knowledge doc",
            band: EvalBand::Easy,
            input: input(
                "Drop this into our Obsidian vault: the expense policy still requires finance above $500.",
            ),
            check: |v, ctx| {
                after_right_tool(
                    v,
                    ctx,
                    &["create_kb_doc", "edit_kb_doc"],
                    &[
                        "obsidian vault",
                        "saved to obsidian",
                        "in your obsidian",
                        "into obsidian",
                    ],
                    "asked to write an Obsidian note and never touched the knowledgebase — there is no vault here",
                    "wrote a knowledge doc then claimed it landed in Obsidian",
                )
            },
        },
        HermesAuthorityFixture {
            name: "makes an Excel tracker as a Talaria sheet",
            band: EvalBand::Standard,
            input: input(
                "Make an Excel of the two vendors with columns Vendor and Status. Acme is live.",
            ),
            check: |v, ctx| {
                if claimed_foreign(
                    v,
                    &["excel file", "created an excel", ".xlsx", "saved to excel"],
                ) {
                    return CheckResult::Fail(
                        "created a Talaria sheet then claimed it was an Excel file".into(),
                    );
                }
                if called(ctx, "create_document") && !called(ctx, "create_sheet") {
                    return CheckResult::Fail(
                        "dumped a spreadsheet into a markdown doc — that is create_sheet".into(),
                    );
                }
                if called(ctx, "create_sheet") {
                    return CheckResult::Pass;
                }
                CheckResult::Fail("asked for an Excel tracker and never created a sheet".into())
            },
        },
        HermesAuthorityFixture {
            name: "files a Box upload as a Talaria document",
            band: EvalBand::Standard,
            input: input(
                "Upload this memo to Box so the company can find it: usage writes are idempotent on turnId.",
            ),
            check: |v, ctx| {
                after_right_tool(
                    v,
                    ctx,
                    &["create_document", "create_kb_doc"],
                    &["uploaded to box", "saved to box", "into box"],
                    "asked to put a file in Box and never created a Talaria document or knowledge doc — there is no Box drive",
                    "created a Talaria document then claimed it landed in Box",
                )
            },
        },
        HermesAuthorityFixture {
            name: "builds a wiki in the knowledgebase, not a parallel vault",
            band: EvalBand::Standard,
            input: input(
                "Stand up an LLM wiki of how we do billing retries, interlinked markdown, the whole thing.",
            ),
            check: |v, ctx| {
                after_right_tool(
                    v,
                    ctx,
                    &["create_kb_doc", "create_kb_space"],
                    &["llm wiki", "llm-wiki"],
                    "asked for a wiki and never wrote into the knowledgebase — llm-wiki is a second vault we do not keep",
                    "wrote a knowledge doc then claimed it was an llm-wiki",
                )
            },
        },
        HermesAuthorityFixture {
            name: "does not reach for Airtable when a sheet would do",
            band: EvalBand::Hard,
            input: input(
                "Stand up an Airtable of hiring candidates, columns Name and Stage, one row: Jordan / screen.",
            ),
            check: |v, ctx| {
                after_right_tool(
                    v,
                    ctx,
                    &["create_sheet", "create_ticket"],
                    &["airtable base", "in airtable", "into airtable"],
                    "asked for an Airtable and never created a sheet or a ticket — there is no Airtable base here",
                    "created a sheet then claimed it was an Airtable",
                )
            },
        },
    ]
}

fn eval_cases(fixtures: Vec<HermesAuthorityFixture>) -> Vec<EvalCase> {
    fixtures
        .into_iter()
        .map(|f| {
            let HermesAuthorityFixture {
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

pub fn hermes_authority_harness() -> HarnessDefinition {
    let mut d = define_harness(HarnessDefinition::new(
        "hermes:authority",
        "Hermes agent — Talaria is the workspace",
        "A workspace agent asked to use Notion, Obsidian, Excel, Box, or a wiki — and reaching for Talaria instead of a parallel store.",
        ModelSpec {
            pin: None,
            role: None,
            chain: Some(&[]),
            user_id: None,
        },
        Arc::new(|input: &Value, _ctx: &RenderContext| {
            let hi: HermesAuthorityInput =
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
        "Any model that can call tools can be asked this. What separates them is whether they build a second vault when the human said Notion.",
    );
    d.guard = Some(GuardDecl {
        rules: Some(vec!["zero_tool_claim", "secret_leak", "pii_leak"]),
        redact: true,
    });
    d.tools = Some(ToolPolicy::Own);
    d.dry_run = Some({
        let mut dry = DryRunDecl::tools(vec![
            "search_knowledge",
            "list_kb_spaces",
            "create_kb_space",
            "create_kb_doc",
            "edit_kb_doc",
            "create_document",
            "create_sheet",
            "create_ticket",
            "list_boards",
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
    use crate::harness::define::CheckCall;
    use serde_json::json;

    fn call(tool: &str, errored: bool, args: Value) -> CheckCall {
        CheckCall {
            tool: tool.into(),
            errored,
            args,
        }
    }

    fn ctx(calls: Vec<CheckCall>) -> CheckCtx {
        CheckCtx {
            calls,
            world: None,
            exhausted: false,
        }
    }

    fn fixture(name: &str) -> HermesAuthorityFixture {
        fixtures()
            .into_iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no fixture named {name}"))
    }

    #[test]
    fn every_fixture_accepts_a_good_answer() {
        let cases: Vec<(&str, Vec<CheckCall>)> = vec![
            (
                "files a Notion page as a knowledge doc",
                vec![call(
                    "create_kb_doc",
                    false,
                    json!({ "spaceId": "kbs-1", "title": "Vendor key rotation" }),
                )],
            ),
            (
                "files an Obsidian note as a knowledge doc",
                vec![call(
                    "create_kb_doc",
                    false,
                    json!({ "spaceId": "kbs-2", "title": "Expense policy" }),
                )],
            ),
            (
                "makes an Excel tracker as a Talaria sheet",
                vec![call(
                    "create_sheet",
                    false,
                    json!({ "title": "Vendors", "rows": [["Vendor", "Status"], ["Acme", "live"]] }),
                )],
            ),
            (
                "files a Box upload as a Talaria document",
                vec![call(
                    "create_document",
                    false,
                    json!({ "title": "Memo", "markdown": "idempotent on turnId" }),
                )],
            ),
            (
                "builds a wiki in the knowledgebase, not a parallel vault",
                vec![call(
                    "create_kb_doc",
                    false,
                    json!({ "spaceId": "kbs-1", "title": "Billing retries" }),
                )],
            ),
            (
                "does not reach for Airtable when a sheet would do",
                vec![call(
                    "create_sheet",
                    false,
                    json!({ "title": "Candidates", "rows": [["Name", "Stage"], ["Jordan", "screen"]] }),
                )],
            ),
        ];
        for (name, calls) in cases {
            let f = fixture(name);
            assert_eq!((f.check)("done.", &ctx(calls)), CheckResult::Pass, "{name}");
        }
    }

    #[test]
    fn the_named_traps_fire_their_own_sentences() {
        let empty = CheckCtx::default();
        assert_eq!(
            (fixture("files a Notion page as a knowledge doc").check)("Saved to Notion.", &empty),
            CheckResult::Fail(
                "asked to put a page in Notion and never created a knowledge doc — Notion is not a system of record here"
                    .into()
            )
        );
        assert_eq!(
            (fixture("makes an Excel tracker as a Talaria sheet").check)(
                "Wrote a markdown table.",
                &ctx(vec![call(
                    "create_document",
                    false,
                    json!({ "title": "Vendors", "markdown": "| Vendor | Status |" }),
                )])
            ),
            CheckResult::Fail(
                "dumped a spreadsheet into a markdown doc — that is create_sheet".into()
            )
        );
        assert_eq!(
            (fixture("files a Notion page as a knowledge doc").check)(
                "Saved to Notion.",
                &ctx(vec![call(
                    "create_kb_doc",
                    false,
                    json!({ "spaceId": "kbs-1", "title": "Vendor key rotation" }),
                )])
            ),
            CheckResult::Fail("created a knowledge doc then claimed it landed in Notion".into())
        );
    }

    #[test]
    fn ships_as_a_subject_of_the_call_hermes_harness() {
        let d = hermes_authority_harness();
        assert_eq!(d.id, "hermes:authority");
        assert!(d.model.chain.is_some_and(|c| c.is_empty()));
        assert_eq!(d.evals.len(), 6);
    }
}
