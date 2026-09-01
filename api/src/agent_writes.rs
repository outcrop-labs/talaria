// THE ONE DOOR for agent-authored text on its way to a human — port of
// ui/src/server/agent-writes.ts.
//
// `guard_completion` guards model OUTPUT that arrived as a completion. An
// agent also writes to humans through its MCP TOOLS, and a tool ARGUMENT is
// model output that never touched a harness: a ticket comment, a channel
// post, a DM, a ticket's title/description/outcome. Before this door existed
// those paths were unguarded — an agent echoing its own `tak_…` credential
// into a teammate's inbox was not flagged, and the stored copy kept the
// credential forever.
//
// ONE DOOR, NOT SEVERAL: one question written once, each write path calling
// it, because N checks at N sites is N chances to write the check differently
// and an N+1th site next quarter with none.
//
// WHO COUNTS AS AN AGENT: a caller that KNOWS it is writing as an agent says
// so (`WriteAuthor::Agent` — channels has an author_type column, so it
// knows); a bare author string is VERIFIED against agent_defs, so the human
// door cannot launder an agent write back in. The lookup deliberately does
// not filter on `enabled` — a disabled agent's text must not slip the guard
// by being read as a person's.
//
// A SECRET REDACTS AND RECORDS. IT NEVER BLOCKS. These tools are how an agent
// does its job; a blocked comment is not a safe outcome, the detectors are
// regex families whose precision is high but not perfect, and redaction is
// lossless in the way that matters — the sentence survives, the credential
// does not. The org already has the graduated control: observe records,
// strict scrubs.
//
// WHAT NEVER GOES BACK TO THE AGENT: guardrails' cardinal invariant — flagged
// CONTENT never re-enters a model's context. This door returns text and
// nothing else to the caller's caller; the findings it returns are for the
// CALLER's own bookkeeping (pinning a caveat onto a row the UI renders), and
// the `snippet` inside them is a verbatim excerpt that must never be handed
// to the agent. The agent sees its own sentence with the secret gone, which
// is a redaction and not a quotation.

use crate::gateway::guard::{
    Finding, Grounding, GuardMode, guard_config, guard_text, needs_redaction, record_findings,
    redact_secrets,
};
use sqlx::PgPool;

/// The write path a finding came from (TS AgentWriteSurface: 'ticket-comment'
/// | 'channel-post' | 'direct-message' | 'ticket-write' | 'capability-gap').
/// It becomes the `caller` on the guard_findings row, so `model` keeps meaning
/// "this model's confabulation rate" while `caller` says which door the text
/// came through.
pub type AgentWriteSurface = &'static str;

/// `WorkbenchActor`'s shape (TS WriteAuthor): the Agent form is a caller that
/// knows it is writing as an agent; Name is an author field that might be
/// either and is VERIFIED rather than believed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WriteAuthor<'a> {
    Name(&'a str),
    Agent(&'a str),
}

#[derive(Debug, Clone)]
pub struct GuardedAgentWrite {
    /// What to persist, deliver and notify with. Identical to the input
    /// unless strict mode redacted a credential out of it.
    pub text: String,
    /// For the caller's own bookkeeping — never returned to the agent.
    pub findings: Vec<Finding>,
    /// The agent this text was attributed to, or None when the author is a
    /// human (a person's typing is not model output and is not guarded here).
    pub agent: Option<String>,
    pub mode: GuardMode,
    pub redacted: bool,
}

/// Does this author string name a fleet agent? One indexed lookup on a small
/// table, no `enabled` filter (see the header). A query error reads as "not
/// an agent" — TS's `.catch(() => false)` — so a database hiccup fails open
/// at the human door rather than throwing out of a comment.
async fn is_agent(pg: &PgPool, name: &str) -> bool {
    sqlx::query_as::<_, (i32,)>("select 1 from agent_defs where model = $1 limit 1")
        .bind(name)
        .fetch_optional(pg)
        .await
        .is_ok_and(|r| r.is_some())
}

/// TS's `clean()`: nothing to do, and — quirks and all — the reported mode is
/// 'off', because a clean result never asked the config anything.
fn clean(text: &str, agent: Option<String>) -> GuardedAgentWrite {
    GuardedAgentWrite {
        text: text.to_string(),
        findings: Vec::new(),
        agent,
        mode: GuardMode::Off,
        redacted: false,
    }
}

/// The decision after the DB reads, split out so the contract is testable:
/// record against the agent, then redact only under strict + needsRedaction.
/// A grounded finding still redacts credentials (secret_leak's split); PII a
/// caller grounded in its own input survives verbatim.
fn finish(
    text: &str,
    agent: String,
    findings: Vec<Finding>,
    mode: GuardMode,
    input: Option<&str>,
) -> GuardedAgentWrite {
    if mode != GuardMode::Strict || !needs_redaction(&findings) {
        return GuardedAgentWrite {
            text: text.to_string(),
            findings,
            agent: Some(agent),
            mode,
            redacted: false,
        };
    }
    // `input` again, because the two halves have to agree: a span the guard
    // declined to file a finding about is a span the redactor must decline to
    // rewrite (PII), and a credential it declined to blame the agent for is
    // still one it removes from the stored copy (secrets).
    let grounding = input.map(Grounding::new);
    let (safe, redacted) = redact_secrets(text, grounding.as_ref());
    GuardedAgentWrite {
        text: safe,
        findings,
        agent: Some(agent),
        mode,
        redacted,
    }
}

/// Run the gate-safe guard rules over text an agent wrote, record what they
/// find against that agent, and hand back what is safe to store.
///
/// `guard_text` is reused rather than rewired: it is already the "gate-safe
/// rules over plain text" entry point, it reads the org's mode itself, and it
/// returns [] when the guard is off. A second wiring of the same rules is how
/// two callers end up disagreeing about which rules "the guard" means.
///
/// Never throws: a guard that failed closed on a database hiccup would take
/// down commenting, posting and DMs — a failure mode worse than the leak it
/// prevents, and indistinguishable from Talaria being broken.
pub async fn guard_agent_write(
    pg: &PgPool,
    surface: AgentWriteSurface,
    by: WriteAuthor<'_>,
    text: &str,
    input: Option<&str>,
) -> GuardedAgentWrite {
    if text.trim().is_empty() {
        return clean(text, None);
    }
    let agent = match by {
        WriteAuthor::Agent(a) => Some(a.to_string()),
        // The human door: verified, not believed — an author string that
        // names no fleet agent is a person, and a person's typing is not
        // model output.
        WriteAuthor::Name(n) => {
            if is_agent(pg, n).await {
                Some(n.to_string())
            } else {
                None
            }
        }
    };
    let Some(agent) = agent else {
        return clean(text, None);
    };
    let findings = guard_text(pg, text, input).await;
    if findings.is_empty() {
        return clean(text, Some(agent));
    }
    // A non-empty result proves the guard is not off, so this read is only
    // ever asking WHICH on-mode. If it lands on a garbage value the default
    // posture is observe: record the finding, change nothing. Guessing strict
    // would silently rewrite a teammate's message under a policy nobody set.
    let mode = guard_config(pg).await.mode;
    // `model` is the AGENT, so the fitness page's per-model confabulation
    // rate counts what an agent leaked through its tools alongside what it
    // leaked through a harness reply. `endpoint: 'fleet'` is the same fact
    // guardChatReply's spelling carries: the text was produced inside the
    // agent, not by a gateway completion we placed.
    record_findings(
        pg,
        &findings,
        &format!("{surface}:{agent}"),
        &agent,
        Some("fleet"),
        mode,
    )
    .await;
    finish(text, agent, findings, mode, input)
}

/// The same door for a write made of SEVERAL agent-authored fields at once —
/// a ticket is the case: title, description, outcome, resolution, every one
/// a tool argument. ONE PASS, NOT ONE PER FIELD: the fields are scanned
/// together so a ticket produces one finding row rather than four, and then
/// redacted individually so each column gets its own clean value. Absent
/// fields stay absent — a None means "don't touch this column" and must
/// survive. Mutates in place.
pub async fn guard_agent_fields(
    pg: &PgPool,
    surface: AgentWriteSurface,
    by: WriteAuthor<'_>,
    fields: &mut [Option<String>],
    input: Option<&str>,
) {
    // TS's join: string values with non-whitespace content, "\n\n" between.
    let joined = fields
        .iter()
        .filter_map(|f| f.as_ref())
        .filter(|v| !v.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    if joined.is_empty() {
        return;
    }
    let guarded = guard_agent_write(pg, surface, by, &joined, input).await;
    if !guarded.redacted {
        return;
    }
    // The SAME grounding material per field as the joined pass used, or the
    // two passes disagree: the join decided whether anything is redacted at
    // all, and these calls decide what each column ends up holding.
    let grounding = input.map(Grounding::new);
    for f in fields.iter_mut() {
        // TS's truthiness: any non-empty string gets the redactor (a
        // whitespace-only value is a no-op rewrite either way).
        if let Some(v) = f
            && !v.is_empty()
        {
            *v = redact_secrets(v, grounding.as_ref()).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_finding() -> Finding {
        Finding {
            check: "secret_leak".into(),
            severity: "high".into(),
            confidence: 0.95,
            message: "Output appears to contain a live credential (Anthropic key).".into(),
            snippet: "Anthropic key: sk-ant-a…".into(),
            grounded: false,
        }
    }

    const KEY: &str = "sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn strict_redacts_a_leaked_credential_and_records_the_rest() {
        let text = format!("here is my key {KEY} for later");
        let out = finish(
            &text,
            "helper".into(),
            vec![secret_finding()],
            GuardMode::Strict,
            None,
        );
        assert!(out.redacted);
        assert_eq!(
            out.text,
            "here is my key [redacted Anthropic key] for later"
        );
        assert_eq!(out.agent.as_deref(), Some("helper"));
        // The finding survives for the caller's bookkeeping — it just never
        // travels back to the agent.
        assert_eq!(out.findings.len(), 1);
    }

    #[test]
    fn observe_records_but_changes_nothing() {
        let text = format!("key {KEY}");
        let out = finish(
            &text,
            "helper".into(),
            vec![secret_finding()],
            GuardMode::Observe,
            None,
        );
        assert!(!out.redacted);
        assert_eq!(out.text, text);
    }

    #[test]
    fn strict_without_a_redaction_worthy_finding_is_a_no_op() {
        // needs_redaction is false: nothing to scrub, whatever the mode.
        let finding = Finding {
            check: "zero_tool_claim".into(),
            grounded: false,
            ..secret_finding()
        };
        let out = finish(
            "plain text",
            "helper".into(),
            vec![finding],
            GuardMode::Strict,
            None,
        );
        assert!(!out.redacted);
        assert_eq!(out.text, "plain text");
    }

    #[test]
    fn a_grounded_credential_still_scrubs_from_the_stored_copy() {
        // secret_leak's split: the FINDING was grounded away in guard_text,
        // but what lands here is strict + a redaction-worthy finding — and
        // the caller grounded the same material, so the PII half keeps it
        // while the credential half goes.
        let finding = Finding {
            grounded: true,
            ..secret_finding()
        };
        let input = format!("you pasted {KEY} earlier");
        let text = format!("as you said, the key is {KEY}");
        let out = finish(
            &text,
            "helper".into(),
            vec![finding],
            GuardMode::Strict,
            Some(&input),
        );
        assert!(out.redacted);
        assert!(out.text.contains("[redacted Anthropic key]"));
        // The same span with NO grounding material is scrubbed identically —
        // the grounding only ever saves PII, never a credential.
        let out = finish(
            &text,
            "helper".into(),
            vec![secret_finding()],
            GuardMode::Strict,
            None,
        );
        assert!(out.text.contains("[redacted Anthropic key]"));
    }

    #[test]
    fn a_grounded_card_survives_while_a_credential_does_not() {
        let input = "order 4111 1111 1111 1111 shipped";
        let text = format!("card 4111-1111-1111-1111 and key {KEY}");
        let pii = Finding {
            check: "pii_leak".into(),
            grounded: true, // guard_text dropped this finding, in reality
            ..secret_finding()
        };
        // Mix: a grounded pii finding (kept for its redaction vote) plus the
        // ungrounded secret — the card is the caller's own data and stays,
        // the invented key goes.
        let out = finish(
            &text,
            "helper".into(),
            vec![pii, secret_finding()],
            GuardMode::Strict,
            Some(input),
        );
        assert!(out.redacted);
        assert!(out.text.contains("4111-1111-1111-1111"));
        assert!(out.text.contains("[redacted Anthropic key]"));
    }
}
