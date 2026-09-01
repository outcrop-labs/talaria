// IMAGE UNDERSTANDING, SUPPLIED BY THE DEPLOYMENT. Port of
// ui/src/server/vision.ts.
//
// THE SHAPE OF THE PROBLEM. `vision` is a property of a MODEL, and most models
// do not have it — including several that are otherwise the right choice for an
// agent. That used to be the end of the conversation: a blind model handed a
// screenshot could do nothing with it, and every surface that might have wanted
// one had to assume it was talking to a model that could see.
//
// It does not have to be the end. An org that has assigned ANY model to the
// `vision` role owns a way to read an image; a model that cannot see can still
// call a tool. So this is the second half of the pattern `capability_reach`
// already established for search: the model does not do it, the DEPLOYMENT
// does, and the fitness matrix says `supplied` rather than `no`.
//
// WHY IT IS A HARNESS AND NOT A RAW CALL. The describing model is a model like
// any other: it needs routing, a capability floor, metering against the ledger,
// and a guard pass over what it returns — an image is untrusted input, and text
// extracted from one lands in a transcript. `run_harness` owns all of that, and
// a hand-written call would get some subset of it wrong in its own way.
//
// WHAT IT DELIBERATELY DOES NOT DO: pretend the calling model can see. The
// answer comes back as TEXT, attributed to the model that produced it, and the
// caller is told which one. A description is a second-hand account and every
// surface that shows it should be able to say so.

use std::sync::Arc;

use serde_json::Value;

use crate::harness::define::{
    GuardDecl, HarnessDefinition, Message, OnFailure, Output, RenderContext, RoleFloor,
    define_harness,
};
use crate::harness::run::{RunContext, real_deps, run_harness};
use crate::harness::transport::{
    TransportKind, TransportReply, TransportRequest, gateway_image_turn,
};
use crate::harness_model::ModelSpec;
use crate::state::AppState;

/// How long a description may take. Well under a harness turn budget: a caller
/// is usually inside its own turn when it asks, and a describe that outlives the
/// turn that wanted it is a timeout charged to the wrong model.
const TIMEOUT_MS: u64 = 60_000;

/// THE ROLE, AND ONLY THE ROLE — the chain as a crate constant so the def can
/// borrow it forever.
const VISION_CHAIN: [&str; 1] = ["role"];

/// THE DESCRIBING PROMPT, and the two rules that make its output usable.
///
/// DESCRIBE, DO NOT ADVISE. The caller is another model that will act on this;
/// a describer that editorialises puts its own judgement into a chain where
/// nobody can see it came from a second model.
///
/// SAY WHAT IS NOT THERE. "The error message is not visible in this crop" is
/// the single most valuable sentence a describer produces, because the
/// alternative is the calling model inventing one from a plausible-sounding
/// description.
const SYSTEM: &str = concat!(
    "You are reading an image on behalf of another agent that cannot see it. Answer its question from the image alone.\n",
    "Describe what is actually there — text verbatim where it is legible, layout and state where it matters, numbers exactly.\n",
    "If the image does not answer the question, say precisely what it does and does not show. Never guess at content that is cropped, blurred or absent.\n",
    "No advice, no next steps, no interpretation beyond what is visible. You are the eyes, not the decision.",
);

/// One describe ask. `image` is a data URI or https URL — the CALLER resolves
/// an upload id to one of these; this module does not know what an upload is.
/// `question` is what the asker actually needs to know: a description written
/// blind is a paragraph about a screenshot, one written against a question is
/// an answer.
pub struct DescribeInput<'a> {
    pub image: &'a str,
    pub question: &'a str,
}

pub fn vision_describe_harness() -> HarnessDefinition {
    let mut d = define_harness(HarnessDefinition::new(
        "vision:describe",
        "Describe an image",
        "Reads an image on behalf of a model that cannot see, and answers one question about it.",
        // THE ROLE, AND ONLY THE ROLE. An org assigns `vision` on the Models
        // page and that is the model that reads images. A one-step chain
        // rather than a fallback: describing an image with whatever happened
        // to route is how a caller ends up trusting a description nobody
        // chose the model for, and a model that cannot see would be refused
        // by the floor anyway — loudly, which is the outcome an operator can
        // act on.
        ModelSpec {
            pin: None,
            role: Some("vision"),
            chain: Some(&VISION_CHAIN),
            user_id: None,
        },
        Arc::new(|input: &Value, _ctx: &RenderContext| {
            let question = input
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(vec![Message::system(SYSTEM), Message::user(question)])
        }),
        Output::Text {
            clean: Some(Arc::new(|raw: &str| {
                let trimmed = raw.trim();
                Ok((!trimmed.is_empty()).then(|| Value::String(trimmed.to_string())))
            })),
            verify: None,
        },
        // Null on failure, and the two callers agree: the route maps a null
        // value onto the no-model-assigned sentence below, and the tool
        // handler hands the same sentence to the asking model as its answer.
        OnFailure::Null,
    ));
    // The one capability that is not optional here. `refuse_below` is TRUE —
    // unlike most harnesses, degrading is not an option: a model that cannot
    // see does not produce a worse description, it produces a confident
    // invention.
    d.requires = vec!["vision"];
    d.floor = RoleFloor::refuses(
        vec!["vision"],
        "Describing an image needs a model that can see one. There is no degraded version of this: a blind model returns a plausible description of an image it never read.",
    );
    // The description is untrusted text derived from an untrusted image, and
    // it is about to be handed to another model as fact. `ungrounded_ref` is
    // the rule that matters: a URL or an id "read off" an image nobody else
    // can check is exactly the shape of an injected instruction.
    d.guard = Some(GuardDecl {
        rules: Some(vec!["ungrounded_ref", "secret_leak", "pii_leak"]),
        redact: true,
    });
    d.hold_ms = Some(TIMEOUT_MS);
    d.temperature = Some(0.2);
    d
}

/// WHAT CAME BACK — `model` reported so a caller can attribute it. A
/// description is a second-hand account and a surface that presents it as the
/// calling model's own observation is lying by omission.
pub struct Description {
    pub text: String,
    pub model: Option<String>,
    pub error: Option<String>,
}

/// Read an image on behalf of a model that cannot. Never fails: a caller is a
/// tool handler, and its error text goes into an agent's transcript — so a
/// failure comes back as a sentence the calling model can act on.
pub async fn describe_image(state: &AppState, image: &str, question: &str) -> Description {
    if image.is_empty() {
        return Description {
            text: String::new(),
            model: None,
            error: Some("no image was given".to_string()),
        };
    }
    if question.trim().is_empty() {
        return Description {
            text: String::new(),
            model: None,
            error: Some(
                "a description needs a question — say what you need to know from the image"
                    .to_string(),
            ),
        };
    }

    // `gateway_image_turn` is the seam that carries IMAGES, which the runner's
    // ordinary transport cannot: `Message.content` is a string by construction
    // (see define.rs). So the runner resolves the model, applies the floor and
    // meters the call, and the image rides the one transport built for it.
    // The dep set is taken WHOLESALE from `real_deps` with the transport leg
    // swapped — the Rust runner has no partial-override spelling, and this is
    // the only edge that differs.
    let mut deps = real_deps(state);
    deps.transport = {
        let st = state.clone();
        let image = image.to_string();
        Arc::new(move |req: TransportRequest| {
            let st = st.clone();
            let image = image.clone();
            Box::pin(async move {
                let text = gateway_image_turn(
                    &st,
                    &req.model,
                    &req.messages,
                    &[image],
                    "vision:describe",
                    Some(TIMEOUT_MS),
                )
                .await?;
                Ok(TransportReply {
                    kind: TransportKind::Gateway,
                    text,
                    tool_names: Vec::new(),
                    tool_calls: None,
                    usage: None,
                    contract_dropped: false,
                })
            })
        })
    };

    let def = vision_describe_harness();
    let input = serde_json::json!({ "image": image, "question": question });
    let ctx = RunContext {
        caller: "vision:describe".into(),
        deps: Some(Arc::new(deps)),
        ..Default::default()
    };
    let out = match run_harness(state, &def, &input, ctx).await {
        Ok(result) => Description {
            text: result
                .value
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            model: result.model,
            error: result.error,
        },
        // TS's `describeImage` catch: a runner-level failure is a sentence for
        // the transcript, not an exception for the caller to handle.
        Err(err) => Description {
            text: String::new(),
            model: None,
            error: Some(err.to_string()),
        },
    };
    if !out.text.is_empty() {
        return out;
    }
    Description {
        text: String::new(),
        model: out.model,
        error: Some(out.error.unwrap_or_else(|| "no model is assigned to the vision role in this workspace, so images cannot be read here. Tell whoever asked that you cannot see the image rather than describing it.".to_string())),
    }
}
