// THE BASE AGENT SURFACE — files and a test runner — for the fitness suite.
//
// WHY IT IS SEPARATE FROM `talaria_tools.rs`. That file is a copy of Talaria's
// own MCP toolkit, locked to `mcp/src/index.ts` by a sync test, and every tool
// on it is a workspace verb: tickets, channels, documents. This is the other
// half of what a coding agent holds — the CLI harness's own file tools, which
// belong to the harness (Claude Code, Codex, Aider …) rather than to Talaria,
// and which no file in this repository defines. There is nothing to lock a copy
// against, so this is a MODEL of that surface rather than a copy of one, and
// the distinction is stated here rather than left for a reader to infer.
//
// WHAT THAT MEANS FOR THE VERDICT IT PRODUCES. The three Workbench slots
// (`code-light`, `code-standard`, `code-heavy`) declare `requires: ['code',
// 'tools']`, and the role hint says why: "without tool calling the run does not
// degrade, it does nothing while reporting that it worked". That is the exact
// failure a prose eval cannot see and this can — so the question these tools
// exist to answer is narrow and worth answering:
//
//     given a repository, a failing test and file tools, does this model
//     LOCATE the defect, EDIT the right file, and CHECK its own work?
//
// It is NOT a claim that the model would drive Claude Code well end to end.
// Nobody can measure that from here, and the fixtures say what they measure.
//
// THE TEST RUNNER DOES NOT RUN CODE. Executing model-written code inside a
// benchmark is a sandbox-escape surface and a flake source, and Talaria has no
// business doing it in-process. `run_tests` instead applies each fixture's own
// `passes` predicate to the CURRENT file contents — a deterministic assertion
// the fixture author wrote, exactly like every other `EvalCase::check` in the
// tree. The model cannot tell the difference: it edits a file, runs the tests,
// and gets a pass or a named failure back.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::harness::define::{WorkspaceFile, WorkspaceSpec};
use crate::harness::transport::ToolDefinition;

use super::sandbox::{DispatchResult, SandboxCall, ToolRefusal};

/// The base tools, as a coding harness offers them. Deliberately five: the
/// fewest that make "find the defect, fix it, verify" expressible. A wider
/// surface would measure tolerance for options rather than the job.
pub fn hermes_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_files".into(),
            description: "List every file in the working tree, with its size in bytes. Start here — the tree is small and this is cheaper than guessing at paths.".into(),
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
        },
        ToolDefinition {
            name: "read_file".into(),
            description: "Read a file in full. Read before you edit: write_file replaces the whole file, so an edit written from memory loses whatever you did not remember.".into(),
            parameters: json!({ "type": "object",
                "properties": { "path": { "type": "string", "description": "Path from the working tree root" } },
                "required": ["path"] }),
        },
        ToolDefinition {
            name: "search".into(),
            description: "Search the working tree for a literal string and return every matching line with its path and line number. Use it to find where a symbol is defined or used.".into(),
            parameters: json!({ "type": "object",
                "properties": { "query": { "type": "string", "description": "Literal text to look for" } },
                "required": ["query"] }),
        },
        ToolDefinition {
            name: "write_file".into(),
            description: "Replace a file’s ENTIRE contents. There is no patch mode: send the whole file as it should end up. Creating a new file is a write to a path that does not exist yet.".into(),
            parameters: json!({ "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path from the working tree root" },
                    "content": { "type": "string", "description": "The complete new contents of the file" },
                },
                "required": ["path", "content"] }),
        },
        ToolDefinition {
            name: "run_tests".into(),
            description: "Run the project’s test suite against the files as they stand right now. Returns either that everything passed, or the first failure with the assertion that produced it.".into(),
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
        },
    ]
}

/// A fresh workspace. Nothing it touches outlives the returned value. The
/// oracle rides along from the def's `DryRunDecl::workspace` — the fixture's
/// own definition of "the tests are green", applied to the files as they
/// stand rather than to one exact diff, because a real fix can be made in
/// more than one place and a benchmark that demands one exact diff measures
/// obedience rather than capability.
pub struct WorkbenchSandbox {
    pub files: Vec<WorkspaceFile>,
    /// Same shape as the Talaria sandbox's log, so a fixture reads both the
    /// same way.
    pub calls: Vec<SandboxCall>,
    passes: Arc<dyn Fn(&[WorkspaceFile]) -> Option<String> + Send + Sync>,
}

impl WorkbenchSandbox {
    pub fn new(workspace: WorkspaceSpec) -> WorkbenchSandbox {
        WorkbenchSandbox {
            files: workspace.files,
            calls: vec![],
            passes: workspace.passes,
        }
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        hermes_tools()
    }

    /// Are the tests green as things stand? The fixture's own oracle, so an
    /// assertion can ask "did it actually fix it" rather than "did it claim to".
    pub fn green(&self) -> bool {
        (self.passes)(&self.files).is_none()
    }

    /// THE WORKSPACE AS THE MODEL LEFT IT, in the slot `CheckCtx::world`
    /// reads. A file surface has no Talaria world, so this is what stands in
    /// its place: the files, and the oracle's verdict on them. Computed on
    /// demand — a value captured at construction would report the state the
    /// model started from.
    pub fn world_as_value(&self) -> Value {
        json!({
            "files": self.files.iter().map(|f| json!({ "path": f.path, "content": f.content })).collect::<Vec<_>>(),
            "failure": (self.passes)(&self.files),
        })
    }

    pub fn calls_to<'a>(&'a self, tool: &'a str) -> impl Iterator<Item = &'a SandboxCall> + 'a {
        self.calls.iter().filter(move |c| c.tool == tool)
    }

    /// Did `a` happen before `b`, where "happened" means "was called at all"?
    pub fn called_before(&self, a: &str, b: &str) -> bool {
        let first = self.calls.iter().position(|c| c.tool == a);
        let second = self.calls.iter().position(|c| c.tool == b);
        match (first, second) {
            (Some(f), Some(s)) => f < s,
            _ => false,
        }
    }

    fn handle(&mut self, tool: &str, a: &Value) -> Result<Value, ToolRefusal> {
        match tool {
            "list_files" => Ok(json!({ "files": self.files.iter()
                .map(|f| json!({ "path": f.path, "bytes": f.content.len() }))
                .collect::<Vec<_>>() })),

            "read_file" => {
                let path = a["path"].as_str().unwrap_or("");
                let file = self.files.iter().find(|f| f.path == path).ok_or_else(|| {
                    // The tree is small, so naming what IS there turns a wrong
                    // guess into one recoverable turn instead of a dead end.
                    ToolRefusal(format!(
                        "no file at \"{path}\". The tree contains: {}",
                        self.files
                            .iter()
                            .map(|f| f.path.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?;
                Ok(json!({ "path": path, "content": file.content }))
            }

            "search" => {
                let query = a["query"].as_str().unwrap_or("");
                if query.is_empty() {
                    return Err(ToolRefusal("\"query\" is required".into()));
                }
                let mut hits = vec![];
                for f in &self.files {
                    for (i, text) in f.content.split('\n').enumerate() {
                        if text.contains(query) {
                            hits.push(
                                json!({ "path": f.path, "line": i + 1, "text": text.trim() }),
                            );
                        }
                    }
                }
                Ok(json!({ "hits": hits }))
            }

            "write_file" => {
                let path = a["path"].as_str().unwrap_or("");
                if path.is_empty() {
                    return Err(ToolRefusal("\"path\" is required".into()));
                }
                // A write with no content is the commonest malformed call here,
                // and silently creating an empty file would destroy the thing
                // being fixed.
                let content = a["content"].as_str().ok_or_else(|| {
                    ToolRefusal(
                        "\"content\" must be the complete new contents of the file, as a string"
                            .into(),
                    )
                })?;
                match self.files.iter_mut().find(|f| f.path == path) {
                    Some(file) => file.content = content.to_string(),
                    None => self.files.push(WorkspaceFile {
                        path: path.to_string(),
                        content: content.to_string(),
                    }),
                }
                Ok(json!({ "ok": true, "path": path, "bytes": content.len() }))
            }

            "run_tests" => {
                let failure = (self.passes)(&self.files);
                Ok(match failure {
                    None => json!({ "passed": true }),
                    Some(failure) => json!({ "passed": false, "failure": failure }),
                })
            }

            other => Err(ToolRefusal(format!("there is no tool called \"{other}\""))),
        }
    }

    /// One tool call against the workspace, recorded either way — same
    /// observation contract as the Talaria sandbox's dispatch.
    pub fn dispatch(&mut self, name: &str, args_json: &str) -> DispatchResult {
        let args: Map<String, Value> = match serde_json::from_str::<Value>(args_json) {
            Ok(Value::Object(map)) => map,
            Ok(_) => Map::new(),
            Err(_) => {
                let error = "the arguments were not valid JSON".to_string();
                self.calls.push(SandboxCall {
                    tool: name.to_string(),
                    args: Value::Object(Map::new()),
                    result: None,
                    error: Some(error.clone()),
                });
                return DispatchResult {
                    text: format!("Error: {error}"),
                    is_error: true,
                };
            }
        };

        match self.handle(name, &Value::Object(args.clone())) {
            Ok(result) => {
                self.calls.push(SandboxCall {
                    tool: name.to_string(),
                    args: Value::Object(args),
                    result: Some(result.clone()),
                    error: None,
                });
                DispatchResult {
                    text: serde_json::to_string(&result).expect("a workspace result serializes"),
                    is_error: false,
                }
            }
            // A refusal is the sandbox behaving like a real tool. There is no
            // other error kind: `handle` returns only refusals, so nothing here
            // can dress a bug up as the model's fault.
            Err(ToolRefusal(message)) => {
                self.calls.push(SandboxCall {
                    tool: name.to_string(),
                    args: Value::Object(args),
                    result: None,
                    error: Some(message.clone()),
                });
                DispatchResult {
                    text: format!("Error: {message}"),
                    is_error: true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> WorkbenchSandbox {
        WorkbenchSandbox::new(WorkspaceSpec {
            files: vec![
                WorkspaceFile {
                    path: "src/bug.ts".into(),
                    content: "const x = 1;\nconst y = x + one;\n".into(),
                },
                WorkspaceFile {
                    path: "src/ok.ts".into(),
                    content: "export const one = 1;\n".into(),
                },
            ],
            passes: Arc::new(|files: &[WorkspaceFile]| {
                let bug = files.iter().find(|f| f.path == "src/bug.ts")?;
                if bug.content.contains("x + one") {
                    Some("expected 2, got x + one".into())
                } else {
                    None
                }
            }),
        })
    }

    #[test]
    fn lists_reads_searches_and_then_edits_and_verifies() {
        let mut s = ws();
        let listed = s.dispatch("list_files", "{}");
        assert!(listed.text.contains("src/bug.ts"));
        let read = s.dispatch("read_file", r#"{"path":"src/bug.ts"}"#);
        assert!(read.text.contains("x + one"));
        let found = s.dispatch("search", r#"{"query":"one"}"#);
        assert!(found.text.contains("\"line\":2"));
        assert!(found.text.contains("src/ok.ts"));
        // The oracle starts red.
        assert!(!s.green());
        let wrote = s.dispatch(
            "write_file",
            r#"{"path":"src/bug.ts","content":"const x = 1;\nconst y = x + 1;\n"}"#,
        );
        assert!(wrote.text.contains("\"bytes\":30"));
        let ran = s.dispatch("run_tests", "{}");
        assert!(ran.text.contains("\"passed\":true"));
        assert!(s.green());
    }

    #[test]
    fn a_read_of_a_missing_file_names_what_is_there() {
        let mut s = ws();
        let r = s.dispatch("read_file", r#"{"path":"src/nope.ts"}"#);
        assert!(r.text.contains("no file at \"src/nope.ts\""));
        assert!(r.text.contains("src/ok.ts"));
    }

    #[test]
    fn a_write_without_content_refuses_rather_than_emptying_the_file() {
        let mut s = ws();
        let r = s.dispatch("write_file", r#"{"path":"src/bug.ts"}"#);
        assert!(r.is_error);
        assert!(r.text.contains("complete new contents"));
        assert!(
            s.files[0].content.contains("x + one"),
            "the file must survive the malformed write"
        );
    }

    #[test]
    fn the_world_is_read_after_the_loop_not_snapshotted_at_construction() {
        let mut s = ws();
        let before = s.world_as_value();
        assert!(before["failure"].is_string());
        s.dispatch("write_file", r#"{"path":"src/bug.ts","content":"fixed"}"#);
        let after = s.world_as_value();
        assert!(after["failure"].is_null());
        assert!(
            after["files"][0]["content"]
                .as_str()
                .unwrap()
                .contains("fixed")
        );
    }

    #[test]
    fn unknown_tools_and_bad_json_are_recorded_observations() {
        let mut s = ws();
        let unknown = s.dispatch("delete_file", r#"{"path":"src/bug.ts"}"#);
        assert_eq!(
            unknown.text,
            "Error: there is no tool called \"delete_file\""
        );
        let bad = s.dispatch("read_file", "{oops");
        assert_eq!(bad.text, "Error: the arguments were not valid JSON");
        assert_eq!(s.calls.len(), 2);
        assert!(s.calls.iter().all(|c| c.error.is_some()));
    }
}
