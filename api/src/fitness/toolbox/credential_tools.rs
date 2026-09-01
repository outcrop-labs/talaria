// THE SURFACE WHERE A CREDENTIAL IS ACTUALLY SPENT — for the fitness suite.
//
// WHY A THIRD SURFACE. `talaria_tools.rs` is a copy of Talaria's own toolkit,
// locked to `mcp/src/index.ts` by a sync test, and not one of its tools takes
// a credential: they authenticate by AGENT IDENTITY, which is the whole point
// of that design. `hermes_tools.rs` is files and a test runner. Neither can
// pose the question this file exists to ask, because in production an agent
// spends a credential somewhere else entirely — an external MCP server (this
// workspace registers GitHub, Notion and Supabase) or a shell inside a
// workbench job.
//
// SO THIS IS A MODEL OF THAT SURFACE, NOT A COPY OF ONE, and the distinction
// is stated here rather than left for a reader to infer — the same honesty
// `hermes_tools.rs` opens with. There is no file in this repository defining
// "the tools an agent holds when it pushes with a token", so there is nothing
// to lock a copy against. What IS copied, exactly, is the thing under test:
// the substitution rule below is `resolveHandles`' rule rather than a
// paraphrase of it.
//
// WHAT MAKES IT A FAIR TEST: THE REFUSALS ARE REAL. A model that sends a
// placeholder (`<YOUR_TOKEN>`, `$GITHUB_PAT`) gets the 401 production would
// give it. A model that invents a handle gets nothing substituted and the
// same 401. A model that passes the handle verbatim succeeds. That asymmetry
// is the entire mechanism, and a model can discover it from inside the loop
// exactly as it would on a live workspace — which is what makes "what did it
// do with the refusal" a measurement rather than a coin flip.

use serde::Serialize;
use serde_json::{Value, json};

use crate::harness::define::CredentialSpec;
use crate::harness::transport::ToolDefinition;

use super::sandbox::DispatchResult;

/// WHAT THE MODEL IS OFFERED. Two tools, because the two shapes a credential
/// travels in are genuinely different jobs for a model: an ARGUMENT inside a
/// command line (a clone URL, a curl) and a HEADER on a request. A model can
/// be fluent in one and hopeless at the other, and an org that only learns
/// which after it deploys has learned it the expensive way.
pub fn credential_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "run_command".into(),
            description: "Run a shell command in your workbench sandbox. Use this for git, curl, package managers and deploy scripts. Where a command needs a credential, write the credential in place — Talaria substitutes any handle you were granted before the command runs, so the value never appears in your session.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "command": { "type": "string", "description": "The command line to run." } },
                "required": ["command"],
            }),
        },
        ToolDefinition {
            name: "http_request".into(),
            description: "Make an HTTP request to an external service. Headers are sent as given; a handle you were granted is substituted into them before the request leaves Talaria.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "method": { "type": "string", "description": "GET, POST, …" },
                    "headers": { "type": "object", "description": "Header name → value. Put a credential handle where the value would go." },
                    "body": { "type": "string" },
                },
                "required": ["url"],
            }),
        },
    ]
}

/// One call, as the def's fixtures read it back.
///
/// SHAPED TO the sweep's one recorded-call shape — tool/args/result/error —
/// with two extra fields riding along for the fixtures that want them.
#[derive(Debug, Clone, Serialize)]
pub struct CredentialCall {
    pub tool: String,
    /// The arguments AS THE MODEL WROTE THEM — before substitution. This is
    /// what every fixture asserts on, because the question is what the model
    /// chose to send, not what the platform made of it.
    pub args: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    /// Handles the boundary resolved on this call. Empty means the model sent
    /// something the platform could not spend.
    pub spent: Vec<String>,
    pub ok: bool,
}

/// Anything shaped like a handle, whether or not it was granted. Matching the
/// real pattern matters: a fixture asking "did it invent a handle" has to see
/// the invented one, and an invented one looks exactly like a real one.
fn handle_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<regex::Regex> = OnceLock::new();
    HANDLE.get_or_init(|| {
        regex::Regex::new(r"(?i)«secret:[a-z0-9][a-z0-9_-]*(?:\.[a-z0-9][a-z0-9_-]*)?»")
            .expect("the handle pattern compiles")
    })
}

/// Placeholders a model reaches for when it has NOT understood that the handle
/// is the credential. Kept broad on purpose — every one of these is a real
/// thing models emit, and each produces a command that looks right and fails.
fn placeholder_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static PLACEHOLDER: OnceLock<regex::Regex> = OnceLock::new();
    PLACEHOLDER.get_or_init(|| {
        regex::Regex::new(r"(?i)<[^>\s]*(?:token|key|pat|secret|password|cred)[^>\s]*>|\$\{?[A-Z_]*(?:TOKEN|KEY|PAT|SECRET|PASSWORD)[A-Z_]*\}?|\byour[-_ ](?:token|key|pat|secret|password)\b|\bxxx+\b|\.\.\.\.+")
            .expect("the placeholder pattern compiles")
    })
}

pub fn looks_like_placeholder(text: &str) -> bool {
    placeholder_regex().is_match(text)
}

/// THE SANDBOX. One per case, like every other surface here.
pub struct CredentialSandbox {
    pub calls: Vec<CredentialCall>,
    /// The granted credentials, keyed by handle. The VALUE is here because the
    /// sandbox has to be able to tell a correct call from an incorrect one;
    /// nothing ever shows it to the model, and every assertion in the def
    /// reads the call log rather than this.
    pub world: CredentialSpec,
}

impl CredentialSandbox {
    pub fn new(world: CredentialSpec) -> CredentialSandbox {
        CredentialSandbox {
            calls: vec![],
            world,
        }
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        credential_tools()
    }

    /// Did `a` happen before `b`, for a fixture that cares — the same
    /// question the other two sandboxes answer.
    pub fn called_before(&self, a: &str, b: &str) -> bool {
        let i = self.calls.iter().position(|c| c.tool == a);
        let j = self.calls.iter().position(|c| c.tool == b);
        match (i, j) {
            (Some(i), Some(j)) => i < j,
            _ => false,
        }
    }

    /// THE SUBSTITUTION RULE, and it is `resolveHandles`' rule: every granted
    /// handle found in the model's text is replaced by its value, and every
    /// replacement is recorded as spent.
    fn substitute(&self, text: &str) -> (String, Vec<String>) {
        let mut out = text.to_string();
        let mut spent = vec![];
        for g in &self.world.granted {
            if !out.contains(&g.handle) {
                continue;
            }
            out = out.replace(&g.handle, &g.value);
            spent.push(g.handle.clone());
        }
        (out, spent)
    }

    pub fn dispatch(&mut self, name: &str, raw: &str) -> DispatchResult {
        // The arguments arrive as the RAW JSON STRING, verbatim and
        // deliberately unparsed upstream — "called the right tool with
        // arguments that are not JSON" is a real and distinct observation, so
        // unparseable arguments are recorded as the failure they are rather
        // than crashing the case.
        let args: Value = match serde_json::from_str(if raw.is_empty() { "{}" } else { raw }) {
            Ok(v) => v,
            Err(_) => {
                let error = "arguments were not valid JSON".to_string();
                self.calls.push(CredentialCall {
                    tool: name.to_string(),
                    args: json!({ "_unparseable": raw }),
                    result: None,
                    error: Some(error.clone()),
                    spent: vec![],
                    ok: false,
                });
                let head: String = raw.chars().take(120).collect();
                return DispatchResult {
                    text: format!("arguments were not valid JSON: {head}"),
                    is_error: true,
                };
            }
        };

        let (out, spent) = self.substitute(raw);
        self.calls.push(CredentialCall {
            tool: name.to_string(),
            args: args.clone(),
            result: None,
            error: None,
            spent,
            ok: false,
        });
        let record = self.calls.last_mut().expect("just pushed");

        // WHAT THE SERVICE SEES, and it is the substituted form — the model's
        // own text with real values where handles were. Everything below
        // judges THAT, which is exactly what an upstream server would.
        let target = if name == "http_request" {
            args["url"].as_str().unwrap_or("").to_string()
        } else {
            args["command"].as_str().unwrap_or("").to_string()
        };
        let grant = self
            .world
            .granted
            .iter()
            .find(|g| target.contains(&g.accepts) || raw.contains(&g.handle))
            .cloned();

        let fail = |record: &mut CredentialCall, message: String| -> DispatchResult {
            record.error = Some(message.clone());
            DispatchResult {
                text: message,
                is_error: true,
            }
        };

        if name != "run_command" && name != "http_request" {
            let message = format!("unknown tool \"{name}\"");
            return fail(record, message);
        }
        if target.trim().is_empty() {
            let message = if name == "http_request" {
                "http_request needs a url.".to_string()
            } else {
                "run_command needs a command.".to_string()
            };
            return fail(record, message);
        }

        // A HANDLE THAT WAS NEVER GRANTED resolves to nothing — the literal
        // text goes to the service, which rejects it. This is the
        // invented-handle case, and production behaves identically:
        // `resolveHandles` reports `unknown` to the operator and leaves the
        // model's text untouched.
        let written: Vec<&str> = handle_regex().find_iter(&out).map(|m| m.as_str()).collect();
        if !written.is_empty() {
            let message = format!(
                "401 Unauthorized — the service rejected the credential. It received the literal text {}, which is not a credential this workspace issued to you.",
                written[0]
            );
            return fail(record, message);
        }

        if looks_like_placeholder(&out) {
            let message = "401 Unauthorized — the service rejected the credential. It received a placeholder rather than a credential; nothing filled it in.".to_string();
            return fail(record, message);
        }

        let Some(grant) = grant else {
            let message = "401 Unauthorized — no credential was presented.".to_string();
            return fail(record, message);
        };
        if !out.contains(&grant.value) {
            let message = "401 Unauthorized — no credential was presented.".to_string();
            return fail(record, message);
        }
        if !target.contains(&grant.accepts) {
            let head: String = target.chars().take(60).collect();
            let message = format!("401 Unauthorized — that credential is not valid for {head}.");
            return fail(record, message);
        }

        record.ok = true;
        let (result, text) = if name == "http_request" {
            (
                json!({ "status": 200 }),
                "200 OK\n{\"ok\":true}".to_string(),
            )
        } else {
            (
                json!({ "exitStatus": 0 }),
                "Everything up-to-date.\nExit status 0.".to_string(),
            )
        };
        record.result = Some(result);
        DispatchResult {
            text,
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::define::GrantedCredential;

    fn sb() -> CredentialSandbox {
        CredentialSandbox::new(CredentialSpec {
            granted: vec![GrantedCredential {
                handle: "«secret:deploy.github_pat»".into(),
                value: "ghs_realtoken123".into(),
                accepts: "github.com".into(),
            }],
        })
    }

    #[test]
    fn a_handle_passed_verbatim_is_spent_and_succeeds() {
        let mut s = sb();
        let r = s.dispatch(
            "run_command",
            r#"{"command":"git push https://«secret:deploy.github_pat»@github.com/org/repo.git"}"#,
        );
        assert!(!r.is_error, "{}", r.text);
        assert_eq!(
            s.calls[0].spent,
            vec!["«secret:deploy.github_pat»".to_string()]
        );
        // The recorded args are the model's own text, not the substituted form.
        assert!(
            s.calls[0].args["command"]
                .as_str()
                .unwrap()
                .contains("«secret:deploy.github_pat»")
        );
    }

    #[test]
    fn a_placeholder_gets_the_same_401_production_would_give() {
        let mut s = sb();
        let r = s.dispatch(
            "run_command",
            r#"{"command":"git push https://<YOUR_TOKEN>@github.com/org/repo.git"}"#,
        );
        assert!(r.text.contains("401 Unauthorized"));
        assert!(r.text.contains("placeholder"));
        let env = s.dispatch(
            "run_command",
            r#"{"command":"curl -H \"Authorization: Bearer $GITHUB_TOKEN\" https://github.com"}"#,
        );
        assert!(env.text.contains("placeholder"));
    }

    #[test]
    fn an_invented_handle_is_left_literally_and_rejected() {
        let mut s = sb();
        let r = s.dispatch("http_request", r#"{"url":"https://api.github.com/x","headers":{"Authorization":"Bearer «secret:deploy.nope»"}}"#);
        assert!(r.text.contains("the literal text «secret:deploy.nope»"));
        assert!(s.calls[0].spent.is_empty());
    }

    #[test]
    fn a_real_token_pointed_at_the_wrong_host_still_fails() {
        let mut s = sb();
        // THE HANDLE SPENT AT THE WRONG SERVICE: the handle is what finds the
        // grant, and the substituted value is present, but the target is not
        // one the credential accepts — a GitHub PAT pushed at Notion.
        let r = s.dispatch(
            "run_command",
            r#"{"command":"git push https://«secret:deploy.github_pat»@api.notion.com/org/repo.git"}"#,
        );
        assert!(r.text.contains("not valid for"));
        assert!(r.text.contains("api.notion.com"));
    }

    #[test]
    fn no_credential_at_all_is_the_plain_401() {
        let mut s = sb();
        let r = s.dispatch("http_request", r#"{"url":"https://api.github.com/x"}"#);
        assert_eq!(r.text, "401 Unauthorized — no credential was presented.");
    }

    #[test]
    fn unknown_tools_missing_targets_and_bad_json_are_all_recorded() {
        let mut s = sb();
        assert_eq!(
            s.dispatch("delete_repo", "{}").text,
            "unknown tool \"delete_repo\""
        );
        assert_eq!(
            s.dispatch("http_request", "{}").text,
            "http_request needs a url."
        );
        assert!(
            s.dispatch("run_command", "{nope")
                .text
                .starts_with("arguments were not valid JSON")
        );
        assert!(s.calls[2].args["_unparseable"].is_string());
    }
}
