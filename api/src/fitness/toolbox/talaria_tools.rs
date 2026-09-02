// THE TALARIA TOOLKIT, AS A MODEL SEES IT — for the fitness suite to hand over.
//
// WHOSE TOOLKIT THIS IS, said first because the answer is not "the platform's".
// Every tool in this file is registered by `mcp/src/index.ts` and reached OVER
// MCP from inside a Hermes agent container. A fleet agent holds them; a native
// platform harness — the Muse, the librarian, the briefer's structured turns —
// does not, and cannot, because nothing in the UI process speaks MCP to itself.
// The platform's own tool surface is a different, much smaller thing — one
// tool, `web_search`, held out by the research harness (`harness/defs/research.rs`
// owns its schema). Conflating the two is the mistake this header exists to
// prevent: a fitness report that says "this model can use Talaria's tools" has
// to mean one of those two surfaces, and they have different sizes, different
// callers and different failure modes.
//
//     hermes      44 tools, over MCP, inside the agent container   THIS FILE
//     platform     handed to a model by a harness via `toolDefs`   research.rs (search)
//
// WHY A COPY AND NOT AN IMPORT. The real registrations live in `mcp/src/index.ts`,
// a separate package whose module body STARTS AN MCP SERVER and whose every
// handler calls Talaria's HTTP API. Importing it from the fitness suite would
// boot a server and reach for a network on `import`. So the definitions are
// copied here — and the sync test at the foot of this file reads
// `mcp/src/index.ts` and fails if a name, a
// DESCRIPTION or a PARAMETER NAME has drifted, which is what keeps "a copy of
// our base tools" true six months from now rather than merely true today.
//
// PARAMETER NAMES ARE PART OF THE COPY, and they did not used to be. Eight of
// the sixteen tools this file originally carried had invented argument names —
// `comment(body)` for the real `comment(content)`, `add_time(minutes)` for
// `add_time(seconds)`, `message_user(user, body)` for `message_user(to,
// message)`, `read_channel(channel)` for `read_channel(channelId)`. A model
// graded against those learned a call it would get a 400 on in production, and
// the benchmark scored it as competent. The sync test now compares the argument
// keys of every `inputSchema`, so that class of flattery cannot come back.
//
// WHY THE DESCRIPTIONS MATTER MORE THAN THE SCHEMAS. This is a probe of whether
// a model can WORK here, and the toolkit's description text is most of the
// instruction it gets: "you cannot move it back", "never twice for the same kind
// of work", "not for status updates". A sandbox with tidied-up descriptions
// would measure a model against a toolkit that does not exist and flatter every
// candidate — the descriptions are the hard part, so they are the part that is
// copied exactly.
//
// THE WHOLE TOOLKIT IS HERE, and that is a change too. It used to carry sixteen
// tools "because the harnesses under test never touch the rest", which quietly
// meant twenty-eight tools an org relies on had no simulated backend, no
// fixture, and no way for anyone to discover that. Coverage is now total and
// enforced by `scripts/check-invariants.mjs`; NARROWING is done per harness with
// `dryRun.tools`, which is where it belongs — a tool surface is a prompt, and
// handing a briefing chat the board-governance tools measures its tolerance for
// noise rather than its ability to answer a question.

use serde_json::{Value, json};
use std::sync::LazyLock;

// ── The axes a report reads along ────────────────────────────────────────────

/// WHERE A TOOL IS CALLED FROM. One value today and that is the finding, not an
/// oversight: every tool below is MCP, and Talaria's own harnesses hold almost
/// nothing. The platform's own native surface is the research harness's single
/// `web_search` tool (`harness/defs/research.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCaller {
    Hermes,
}

/// What the tool is FOR — the axis an admin reads a fitness report along, and
/// the tab grouping on the model-fitness page. Kept here rather than in the UI
/// so one tool cannot be filed under two headings in two places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGroup {
    Boards,
    Tickets,
    Ledger,
    Knowledge,
    Documents,
    Comms,
    People,
    Google,
    Research,
    Governance,
}

impl ToolGroup {
    /// The kebab-case group name — the admin-facing axis the UI tabs on and the
    /// spelling a report or fixture names a group by.
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolGroup::Boards => "boards",
            ToolGroup::Tickets => "tickets",
            ToolGroup::Ledger => "ledger",
            ToolGroup::Knowledge => "knowledge",
            ToolGroup::Documents => "documents",
            ToolGroup::Comms => "comms",
            ToolGroup::People => "people",
            ToolGroup::Google => "google",
            ToolGroup::Research => "research",
            ToolGroup::Governance => "governance",
        }
    }
}

// ── The record ───────────────────────────────────────────────────────────────

/// `${...}` in a real description interpolates a shared clause (LIVE_ONLY,
/// COMMENT_EXEMPTION). The sandbox spells those out; the sync test therefore
/// compares the STATIC PREFIX — everything up to the first interpolation — which
/// is the part a copy can be held to exactly.
#[derive(Debug, Clone)]
pub struct SandboxTool {
    /// The tool this stands in for, as registered in `mcp/src/index.ts`.
    pub name: &'static str,
    pub caller: ToolCaller,
    pub group: ToolGroup,
    pub description: &'static str,
    pub parameters: Value,
    /// Talaria refuses this for an agent that is not somebody's PERSONAL
    /// ASSISTANT (401/403, server-side). The sandbox refuses it the same way, so a
    /// fixture can measure whether a model respects "personal assistants only"
    /// rather than discovering it in production.
    pub assistant_only: bool,
    /// Needs a connected Google account. Staged as absent in some fixtures — a
    /// model that invents calendar entries when the integration is off is the
    /// failure worth catching.
    pub needs_google: bool,
}

// ── The schema helpers (str/num/bool/strs/oneOf, one line each) ──────────────

fn str_schema(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn num_schema(description: &str) -> Value {
    json!({ "type": "number", "description": description })
}

fn bool_schema(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

fn strs_schema(description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": description })
}

fn one_of(values: &[&str], description: &str) -> Value {
    json!({ "type": "string", "enum": values, "description": description })
}

const PRIORITIES: &[&str] = &["low", "medium", "high", "urgent"];
const EFFORTS: &[&str] = &["xs", "s", "m", "l", "xl"];
const COLORS: &[&str] = &[
    "slate", "bronze", "green", "amber", "red", "blue", "purple", "teal", "pink", "orange", "lime",
    "cyan", "indigo", "magenta", "olive", "brown",
];

// ── The catalog ──────────────────────────────────────────────────────────────

/// WHY A `LazyLock` AND NOT A `const`: the schemas are `serde_json` `Value`s
/// built by `json!`, and a Rust `const` cannot hold one (`json!` expands to
/// runtime inserts, not a const constructor). `LazyLock` builds the catalog
/// once on first use and hands out `&'static` entries, so `tools_named`
/// borrows the one table for the life of the process.
pub static TALARIA_TOOLS: LazyLock<Vec<SandboxTool>> = LazyLock::new(|| {
    vec![
        // ── Boards & tickets ───────────────────────────────────────────────────
        SandboxTool {
            name: "list_boards",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Boards,
            description: "List the boards this agent is allowed to work on. Boards a person archived are out of service and are not listed — every tool refuses them, so a board id you got from somewhere else and cannot see here will 403.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "list_tickets",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "List a board's tickets. Each carries status, priority, assignees (agent ids and humans as user:<id>), labels, due date, human estimate (estimatedHours), sub-task parent (parentId), and comment count. Archived tickets are not listed — a person took them off the table and they refuse every agent write. Optional filters narrow the list.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "boardId": str_schema("Board id (from list_boards)"),
                    "status": str_schema("Only this status (inbox/assigned/in_progress/blocked/quality_review/done)"),
                    "assignee": str_schema("Only tickets assigned to this agent id or user:<id>"),
                    "label": str_schema("Only tickets carrying this label"),
                    "overdue": bool_schema("Only tickets past their due date and not done"),
                    "parentId": str_schema("Only sub-tasks of this ticket"),
                },
                "required": ["boardId"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "get_ticket",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "Get a ticket in full: fields (incl. human estimate, sub-task parentId, mixed assignees — agents by id, humans as user:<id>), comments, activity, watchers, reviews, dependencies. May include `workflows` — how this kind of work is done here; follow their instructions. Tickets may carry an `attachments` array (files + knowledge/artifact refs) — read a file with fetch_attachment.",
            parameters: json!({ "type": "object", "properties": { "taskId": str_schema("Ticket id") }, "required": ["taskId"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "fetch_attachment",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "Read an attached file by upload id (from a ticket or chat attachments array; ref-type entries are knowledge docs/artifacts — use read_kb_doc/get_document for those). Text files come back as text, images as an image you can see; other binary formats report metadata only.",
            parameters: json!({ "type": "object", "properties": { "uploadId": str_schema("Attachment upload id") }, "required": ["uploadId"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "create_ticket",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "Create a ticket on a board. It lands in the inbox for a human to assign — agents cannot assign work, and hour estimates stay human too (use effort for your own sizing). Pass parentId to create it as a SUB-TASK of an existing ticket (work breakdown; one level deep).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "boardId": str_schema("Board id (from list_boards)"),
                    "title": str_schema("Ticket title"),
                    "description": str_schema("Markdown description"),
                    "priority": one_of(PRIORITIES, "Priority"),
                    "effort": one_of(EFFORTS, "T-shirt size estimate"),
                    "tags": strs_schema("Labels"),
                    "dueDate": str_schema("Due date, RFC3339"),
                    "startDate": str_schema("When work should begin (Gantt bars run start → due)"),
                    "color": one_of(COLORS, "Color-code the ticket (shows on cards + gantt)"),
                    "parentId": str_schema("Create as a sub-task of this ticket (same board, one level deep)"),
                },
                "required": ["boardId", "title"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "triage_ticket",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "Triage a ticket: adjust priority, effort, labels, description, due date — and move its status FORWARD. Forward means: start work you were already assigned (→ in_progress), park it (→ blocked), or hand it to review (→ quality_review). You cannot move it back. Restarting your own blocked ticket, taking anything out of quality_review, and any write at all to a ticket a person has taken off the table — all refuse with 403, and assigning or marking done are human-only.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "taskId": str_schema("Ticket id"),
                    "title": str_schema("New title"),
                    "description": str_schema("New markdown description"),
                    "priority": one_of(PRIORITIES, "New priority"),
                    "effort": one_of(EFFORTS, "T-shirt size estimate"),
                    "tags": strs_schema("Labels (replaces the set)"),
                    "dueDate": str_schema("Due date, RFC3339"),
                    "startDate": str_schema("When work begins (Gantt bars run start → due)"),
                    "color": one_of(COLORS, "Color-code the ticket (shows on cards + gantt)"),
                    "status": one_of(
                        &["in_progress", "blocked", "quality_review"],
                        "Forward only. in_progress is legal from a ticket already assigned to you or already in progress — never from blocked. blocked and quality_review are one-way: a person moves it after that.",
                    ),
                },
                "required": ["taskId"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "comment",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "Comment on a ticket. Comments stay allowed on a ticket in review, which is where anything further goes once you have reported an outcome.",
            parameters: json!({ "type": "object", "properties": { "taskId": str_schema("Ticket id"), "content": str_schema("Markdown comment body") }, "required": ["taskId", "content"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "report_outcome",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "Report the outcome of work on a ticket and hand it to the board's review column (\"Quality review\" unless your board renamed it). A human signs off on done. This is your LAST status move on that ticket: once it is in review, triage_ticket cannot take it back out, so add anything further as a comment. Only ever on a live ticket.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "taskId": str_schema("Ticket id"),
                    "outcome": str_schema("What was accomplished"),
                    "resolution": str_schema("How it was resolved"),
                    "errorMessage": str_schema("If the work failed, what went wrong"),
                },
                "required": ["taskId", "outcome"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "report_gap",
            caller: ToolCaller::Hermes,
            group: ToolGroup::People,
            description: "Report a capability gap — use ONLY when you genuinely cannot do assigned work properly: a tool or access you're missing, or an org-specific process you'd be guessing at. NOT for small tasks you can simply do with your own judgment, and never twice for the same kind of work (repeats are deduplicated; the team is already aware). The team sees gaps ranked by recurrence and can build a workflow/skill to cover them. taskId is optional and gated: it writes a line onto that ticket, so a board you are not allowed on — or a ticket that is off the table — refuses it.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "kind": str_schema("Short slug naming the kind of work, e.g. \"invoice-reconciliation\""),
                    "missing": str_schema("One line: what you can't do properly and why"),
                    "needs": str_schema("What a flow would need to cover: steps, tools, access, decisions"),
                    "taskId": str_schema("The ticket that surfaced the gap — keep it, so the report reaches the people who can see that work"),
                },
                "required": ["kind", "missing"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "add_time",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Ledger,
            description: "Add time spent to a ticket's auto-accumulated total. Log it as you work. Only ever on a live ticket, and time you never logged before sign-off cannot be added afterwards.",
            parameters: json!({ "type": "object", "properties": { "taskId": str_schema("Ticket id"), "seconds": num_schema("Seconds of work to add") }, "required": ["taskId", "seconds"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "add_dependency",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Tickets,
            description: "Mark a ticket as blocked by another ticket on the same board. The edge lands on BOTH tickets, so BOTH must be live: one that is off the table refuses with 403 on either side of the edge, and the error names which. Removing an edge is a human call.",
            parameters: json!({ "type": "object", "properties": { "taskId": str_schema("The blocked ticket"), "dependsOnId": str_schema("The ticket it depends on") }, "required": ["taskId", "dependsOnId"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "log_usage",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Ledger,
            description: "Report the LLM tokens you burned working a ticket. Feeds the ticket's cost rollup and the fleet ledger. Log it as you go: only ever on a live ticket.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "taskId": str_schema("Ticket id"),
                    "promptTokens": num_schema("Prompt/input tokens used"),
                    "completionTokens": num_schema("Completion/output tokens used"),
                    "tier": str_schema("Model tier the work ran on (alias name), if not your main model"),
                    "estimated": bool_schema("True when the counts are estimates"),
                },
                "required": ["taskId", "promptTokens", "completionTokens"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        // ── Knowledge ──────────────────────────────────────────────────────────
        SandboxTool {
            name: "web_search",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "Search the live web and get back ranked results with their URLs. Use it for anything that happened or changed recently, anything you would otherwise be guessing at, and any claim a reader would expect a source for — then cite the URLs you used. This searches the PUBLIC web: for this organization's own documents and past conversations use search_knowledge instead.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": str_schema("What to look up, as you would type it into a search engine"),
                    "limit": num_schema("How many results to return (default 10)"),
                },
                "required": ["query"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "describe_image",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "Read an image you cannot see: attaches it to this workspace's vision model and returns a description in text. Use it for a screenshot, a photo, a chart or a scanned document — ask a specific question rather than \"what is this\", because the answer is written against your question. The reply is another model's reading of the image, not yours: quote it as a description, and if it says something is not visible, do not fill the gap yourself.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "uploadId": str_schema("Attachment upload id (from a ticket or chat attachments array)"),
                    "question": str_schema("What you need to know from the image"),
                },
                "required": ["uploadId", "question"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "search_knowledge",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "Search the workspace and knowledge you have access to (org knowledgebase, departmental collections, and past chats/channels/plans/research). Use it to recall earlier discussion or ground an answer in real docs — e.g. \"what did we decide about X a couple weeks ago\". Returns ranked snippets with pointers.",
            parameters: json!({ "type": "object", "properties": { "query": str_schema("What to look for, in natural language"), "limit": num_schema("Max results (default 8)") }, "required": ["query"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "list_kb_spaces",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "List knowledgebase spaces (folders) you can access — org/public ones plus any shared with you. Use to find where a doc lives.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "list_kb_docs",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "List the docs in a knowledgebase space (id + title + tree position). Get the spaceId from list_kb_spaces.",
            parameters: json!({ "type": "object", "properties": { "spaceId": str_schema("Space id (from list_kb_spaces)") }, "required": ["spaceId"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "read_kb_doc",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "Read a knowledgebase doc in full — its markdown body + metadata. Use search_knowledge or list_kb_docs to find the id.",
            parameters: json!({ "type": "object", "properties": { "docId": str_schema("KB doc id") }, "required": ["docId"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "create_kb_space",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "Create a knowledge space (a top-level shelf like 'Company' or 'Engineering') when no existing space fits — check list_kb_spaces first. Find-or-create by name, so retries are safe.",
            parameters: json!({
                "type": "object",
                "properties": { "name": str_schema("Space name"), "description": str_schema("One line on what belongs here"), "icon": str_schema("An emoji for the space") },
                "required": ["name"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "create_kb_doc",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "Create a NEW knowledgebase doc in a space you can read (get the spaceId from list_kb_spaces) — use this when asked to add something to the knowledge base. Your doc starts as a draft; a human marks it official before it grounds the org brain. Returns the doc id (keep editing with edit_kb_doc).",
            parameters: json!({
                "type": "object",
                "properties": { "spaceId": str_schema("Space id (from list_kb_spaces)"), "title": str_schema("Doc title"), "markdown": str_schema("Initial markdown body"), "parentId": str_schema("Nest under this doc id (optional)") },
                "required": ["spaceId", "title"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "edit_kb_doc",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Knowledge,
            description: "Edit a knowledgebase doc you've been granted Editor access to: replace its title and/or markdown body. Each save is versioned. You can't change sharing or officialize — a human owns that.",
            parameters: json!({ "type": "object", "properties": { "docId": str_schema("KB doc id"), "title": str_schema("New title"), "markdown": str_schema("New full markdown body") }, "required": ["docId"] }),
            assistant_only: false,
            needs_google: false,
        },
        // ── Documents (artifacts) ──────────────────────────────────────────────
        SandboxTool {
            name: "create_document",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Documents,
            description: "Create a document (a rich markdown doc, Talaria's Google-Docs equivalent). Use it to draft deliverables — reports, specs, briefs, memos. It's versioned, shareable, and hostable. Returns the document id (use update_document to keep editing it).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": str_schema("Document title"),
                    "markdown": str_schema("Initial body as markdown (headings, lists, tables, code, links)"),
                    "visibility": one_of(&["private", "org", "public"], "Who can see it. Personal assistants always create private-to-owner docs (this field is ignored); org agents default to 'org' — the workspace"),
                    "folder": str_schema("File it under this folder name (find-or-create); omitted = your own folder"),
                },
                "required": ["title"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "update_document",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Documents,
            description: "Edit a document you created (or were granted Editor access to): replace its title and/or markdown body. Each save is versioned.",
            parameters: json!({
                "type": "object",
                "properties": { "documentId": str_schema("Document id (from create_document or list_documents)"), "title": str_schema("New title"), "markdown": str_schema("New full markdown body") },
                "required": ["documentId"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "list_documents",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Documents,
            description: "List the documents (artifacts) you can access — the workspace-visible ones plus any shared with you.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "get_document",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Documents,
            description: "Read one document's full content (markdown body + metadata).",
            parameters: json!({ "type": "object", "properties": { "documentId": str_schema("Document id") }, "required": ["documentId"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "save_image_artifact",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Documents,
            description: "Save an image from YOUR workspace (a file you created under /opt/data, e.g. a chart, screenshot, or meme) as a durable file artifact in Talaria — versioned, shareable, and visible on the Artifacts page. Optionally file it into a folder by name (created if missing). Use this when a human should keep the image beyond this conversation. Images only (png/jpg/gif/webp), max 25MB.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": str_schema("Absolute path inside your workspace, e.g. /opt/data/charts/q3.png"),
                    "title": str_schema("Artifact title (defaults to the filename)"),
                    "folder": str_schema("Folder name to file it under, e.g. \"Memes\" (find-or-create)"),
                },
                "required": ["path"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "export_to_google_doc",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Documents,
            description: "Export a document (or sheet/file artifact) into Google Drive as a native Google Doc / Sheet. It lands in the Google Drive of whoever you act for: if you're someone's personal assistant, their Drive; otherwise the shared organization Google account. Returns the Drive file link. Use it to hand a finished deliverable to a human in a format they can edit in Google. (Requires that Google account to be connected in Talaria.)",
            parameters: json!({ "type": "object", "properties": { "documentId": str_schema("Document/artifact id (from create_document or list_documents)") }, "required": ["documentId"] }),
            assistant_only: false,
            needs_google: true,
        },
        // ── Comms ──────────────────────────────────────────────────────────────
        SandboxTool {
            name: "list_channels",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Comms,
            description: "List the team channels you can see and read. A personal assistant sees its owner’s channels and DMs; any other agent sees the channels it has been added to. Returns id + name + topic — pass the id, never the name, to read/post.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "read_channel",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Comms,
            description: "Read recent messages in a channel you belong to. Pass sinceSeq to get only newer messages than a seq you already saw.",
            parameters: json!({
                "type": "object",
                "properties": { "channelId": str_schema("Channel id (from list_channels)"), "sinceSeq": num_schema("Only messages with seq greater than this (default: all recent)") },
                "required": ["channelId"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "post_to_channel",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Comms,
            description: "Post a message to a channel you belong to. Use @name to mention teammates. Post when you have something useful — progress, an answer, a blocker, a question — not chatter.",
            parameters: json!({ "type": "object", "properties": { "channelId": str_schema("Channel id (from list_channels)"), "content": str_schema("Markdown message") }, "required": ["channelId", "content"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "message_user",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Comms,
            description: "Start (or continue) a direct conversation with a human teammate — your message lands in their chat with you plus an inbox notification. Reach for it when something genuinely needs THEIR attention now: work of theirs is blocked on you, you found something they should decide on, a deadline is about to slip. Not for status updates (comment on the ticket) or things the whole room should see (post_to_channel). Rate-limited per person per day — if it declines, respect the returned reason.",
            parameters: json!({
                "type": "object",
                "properties": { "to": str_schema("The teammate's email (preferred) or exact display name"), "message": str_schema("Your message — lead with why this needs them") },
                "required": ["to", "message"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "list_teammates",
            caller: ToolCaller::Hermes,
            group: ToolGroup::People,
            description: "The company directory: every teammate's name and email. Use it to resolve a person's name into an email for draft_email, add_board_member, or board/channel questions ('who is Priya?').",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "report_problem",
            caller: ToolCaller::Hermes,
            group: ToolGroup::People,
            description: "Something on YOUR side is broken (a tool errors, a connection refuses, credentials missing) and a teammate is affected. This alerts the workspace admins and files a Helpdesk ticket with the technical details. Use it INSTEAD of explaining endpoints/ports/errors to non-technical people — then relay the returned plain-language reassurance and offer what you can still do. taskId is optional and gated the same way report_gap's is: a board you are not allowed on — or a ticket that is off the table — refuses it.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": str_schema("One plain sentence: what is broken, from the user's point of view"),
                    "details": str_schema("The full technical details for the admin (errors, endpoints, what you tried)"),
                    "context": str_schema("What you were trying to do for whom"),
                    "taskId": str_schema("The ticket you were working when it broke — keep it, so the report reaches the people who can see that work"),
                },
                "required": ["summary"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        // ── Google (reads are live; anything outbound is DRAFTED for a human) ──
        SandboxTool {
            name: "read_calendar",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "Read upcoming Google Calendar events — your owner's if you're a personal assistant, otherwise the shared org calendar. Requires that Google account to be connected in Talaria.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "draft_calendar_event",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "Draft a Google Calendar event (on your owner's calendar, or the shared org calendar). It is NOT created immediately — it's queued for a human to approve in Talaria first (confirm-sends). Times are RFC3339 (e.g. 2026-07-08T15:00:00Z), or YYYY-MM-DD with allDay=true.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": str_schema("Event title"),
                    "start": str_schema("Start — RFC3339 dateTime or YYYY-MM-DD"),
                    "end": str_schema("End — RFC3339 dateTime or YYYY-MM-DD"),
                    "description": str_schema("Event description"),
                    "location": str_schema("Where"),
                    "allDay": bool_schema("True for a whole-day event (dates, not times)"),
                    "attendees": strs_schema("Attendee emails"),
                },
                "required": ["summary", "start", "end"],
            }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "read_recent_email",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "Read recent Gmail (metadata + snippets) — your owner's if you're a personal assistant, otherwise the shared org mailbox. Optional Gmail search `q` (e.g. 'from:boss', 'is:unread'). Requires that Google account connected in Talaria.",
            parameters: json!({ "type": "object", "properties": { "q": str_schema("Gmail search query (default 'in:inbox')") }, "required": [] }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "read_email",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "Read ONE full email (headers + complete plain-text body) by its id — the id `read_recent_email` returns. Use it before summarizing a thread, quoting a question, or drafting a reply: the snippet is a teaser, not the message. Reads are free; only sending waits for approval.",
            parameters: json!({ "type": "object", "properties": { "id": str_schema("Message id from read_recent_email") }, "required": ["id"] }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "list_labels",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "List the Gmail labels on the account you act for — your owner's if you're a personal assistant, otherwise the shared org mailbox. Labels are Gmail's folders: a message is 'in a folder' by carrying its label, INBOX and UNREAD are system labels, and organizing mail means applying and removing them. Requires that Google account connected in Talaria.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "create_label",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "Create a Gmail label (Gmail's name for a folder) on the account you act for, find-or-create: an existing label of the same name is returned as-is, so a retry is safe. Create the label BEFORE organize_emails refers to it — organizing refuses a label that does not exist.",
            parameters: json!({ "type": "object", "properties": { "name": str_schema("Label name, e.g. \"Vendor\"") }, "required": ["name"] }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "organize_emails",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "File, archive or mark-read emails by id — the ids read_recent_email returns. addLabels/removeLabels take label NAMES (from list_labels or create_label, or the system labels): removing INBOX archives a message (it stays in All Mail and keeps its labels), removing UNREAD marks it read. Applies immediately and reversibly — nothing is ever deleted, TRASH/SPAM are refused, and mail you file stays recoverable. Read before you file: sort by what the messages actually say, never by subject alone.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "ids": strs_schema("Message ids from read_recent_email"),
                    "addLabels": strs_schema("Label names to apply"),
                    "removeLabels": strs_schema("Label names to remove (INBOX archives, UNREAD marks read)"),
                },
                "required": ["ids"],
            }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "search_drive",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "Find files in the Google Drive you act for — your owner's if you're a personal assistant, otherwise the shared org Drive. Returns names, types, modified dates and links. Read-only: locating and linking files, not changing them. Optional `q` matches file names.",
            parameters: json!({ "type": "object", "properties": { "q": str_schema("Name substring to match") }, "required": [] }),
            assistant_only: false,
            needs_google: true,
        },
        SandboxTool {
            name: "draft_email",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Google,
            description: "Draft an email to send (AS your owner, or from the shared org account). It is NOT sent immediately — it's queued for a human to approve in Talaria first (confirm-sends). Use this to prepare correspondence; a human reviews and sends with one click.",
            parameters: json!({
                "type": "object",
                "properties": { "to": str_schema("Recipient email(s), comma-separated"), "subject": str_schema("Subject line"), "body": str_schema("Plain-text body"), "cc": str_schema("Cc"), "bcc": str_schema("Bcc") },
                "required": ["to"],
            }),
            assistant_only: false,
            needs_google: true,
        },
        // ── Research ───────────────────────────────────────────────────────────
        SandboxTool {
            name: "research",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Research,
            description: "Start cited web research on a question in your field. Modes: 'recon' (one fast pass — a cited answer, ~1 min), 'brief' (planned angles → a briefing document, a few minutes), 'expedition' (iterative deep dive → a full report, can take a while). Runs in the background — you get a runId; poll research_status, then read the report with get_document. Every claim in the report carries [n] citations. Use recon for quick facts mid-task; brief/expedition for real questions worth a document.",
            parameters: json!({
                "type": "object",
                "properties": { "question": str_schema("What to research, in natural language — be specific"), "mode": one_of(&["recon", "brief", "expedition"], "Depth (default 'brief')") },
                "required": ["question"],
            }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "list_research",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Research,
            description: "Recent research runs across the workspace (yours and others) — question, mode, status, and the report documentId when done.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "research_status",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Research,
            description: "Check a research run: status (queued/running/done/error), current phase, and — when done — the report's documentId (read it with get_document) plus the source list.",
            parameters: json!({ "type": "object", "properties": { "runId": str_schema("Run id (from research)") }, "required": ["runId"] }),
            assistant_only: false,
            needs_google: false,
        },
        // ── Board governance (personal assistants only) ────────────────────────
        SandboxTool {
            name: "list_teams",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Governance,
            description: "List the teams your owner belongs to (personal assistants only). Use the names with move_board_to_team.",
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
            assistant_only: true,
            needs_google: false,
        },
        SandboxTool {
            name: "move_board_to_team",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Governance,
            description: "Move a board into a team, or back to Personal (personal assistants only, and only for boards your owner OWNS — it changes who can see the board). Team is matched by name; use 'personal' to remove it from any team.",
            parameters: json!({
                "type": "object",
                "properties": { "boardId": str_schema("Board id (from list_boards)"), "teamName": str_schema("Team name (see list_teams), or 'personal' for no team") },
                "required": ["boardId", "teamName"],
            }),
            assistant_only: true,
            needs_google: false,
        },
        SandboxTool {
            name: "list_board_members",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Governance,
            description: "Who's on a board (people + roles). Any agent allowed on the board may read this.",
            parameters: json!({ "type": "object", "properties": { "boardId": str_schema("Board id (from list_boards)") }, "required": ["boardId"] }),
            assistant_only: false,
            needs_google: false,
        },
        SandboxTool {
            name: "add_board_member",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Governance,
            description: "Share a board with a person by email (personal assistants only; your owner needs owner/editor on the board). Role 'editor' can change the board, 'viewer' is read-only.",
            parameters: json!({
                "type": "object",
                "properties": { "boardId": str_schema("Board id (from list_boards)"), "email": str_schema("The person's email"), "role": one_of(&["editor", "viewer"], "Access level (default 'editor')") },
                "required": ["boardId", "email"],
            }),
            assistant_only: true,
            needs_google: false,
        },
        SandboxTool {
            name: "remove_board_member",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Governance,
            description: "Remove a person from a board by email (personal assistants only; your owner needs owner/editor on the board). The board owner can't be removed.",
            parameters: json!({ "type": "object", "properties": { "boardId": str_schema("Board id (from list_boards)"), "email": str_schema("The person's email") }, "required": ["boardId", "email"] }),
            assistant_only: true,
            needs_google: false,
        },
        SandboxTool {
            name: "set_board_agents",
            caller: ToolCaller::Hermes,
            group: ToolGroup::Governance,
            description: "Add or remove fleet agents on a board (personal assistants only; your owner needs owner/editor). Pass agent model names (e.g. 'engineer-engineering'). Returns the board's resulting agent policy.",
            parameters: json!({
                "type": "object",
                "properties": { "boardId": str_schema("Board id (from list_boards)"), "add": strs_schema("Agent models to allow on the board"), "remove": strs_schema("Agent models to remove from the board") },
                "required": ["boardId"],
            }),
            assistant_only: true,
            needs_google: false,
        },
    ]
});

// ── The selectors ────────────────────────────────────────────────────────────

/// Every tool's name, in catalog order — the `dryRun.tools` vocabulary.
pub fn talaria_tool_names() -> Vec<&'static str> {
    TALARIA_TOOLS.iter().map(|t| t.name).collect()
}

/// The subset a given harness should be offered. A tool surface is a prompt:
/// handing a briefing chat the ticket-triage tools measures its tolerance for
/// irrelevant options rather than its ability to do the job.
///
/// UNKNOWN NAMES PANIC rather than being silently dropped. A `dryRun.tools` list
/// with a typo in it used to hand the model a SHORTER surface than its author
/// meant, and the fixture then failed the model for not calling a tool it was
/// never offered — our gap, scored as the model's.
pub fn tools_named(names: &[&str]) -> Vec<&'static SandboxTool> {
    let unknown: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !TALARIA_TOOLS.iter().any(|t| t.name == *n))
        .collect();
    if !unknown.is_empty() {
        panic!(
            "no such Talaria tool: {} — check dryRun.tools against talaria-tools.ts",
            unknown.join(", ")
        );
    }
    TALARIA_TOOLS
        .iter()
        .filter(|t| names.contains(&t.name))
        .collect()
}

/// Every tool in one group, for a fixture that means "the whole documents
/// surface" and should not have to keep a list in sync by hand.
pub fn tools_in_group(groups: &[ToolGroup]) -> Vec<&'static str> {
    TALARIA_TOOLS
        .iter()
        .filter(|t| groups.contains(&t.group))
        .map(|t| t.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::PathBuf;

    // THE LOCK ON THE COPY.
    //
    // This catalog is a hand-copy of registrations that live in
    // `mcp/src/index.ts`, because that module's body starts an MCP server and
    // every handler reaches for Talaria's HTTP API — importing it from a
    // benchmark would boot a server. A copy is the right call and a rotting
    // copy is not, so this reads the real source and fails when the two
    // disagree.
    //
    // WHAT IS COMPARED, and why it is the prefix rather than the whole string.
    // Several real descriptions interpolate a shared clause (`${LIVE_ONLY}`,
    // `${COMMENT_EXEMPTION}`, `${OFF_THE_TABLE}`) that only exists inside that
    // module. The sandbox spells the meaning out in its own words after that
    // point. So the assertion is: the sandbox description STARTS WITH the real
    // literal text up to the first interpolation. That is the part a copy can
    // be held to exactly, and it is where every hard rule lives ("you cannot
    // move it back", "use ONLY when you genuinely cannot", "Not for status
    // updates").
    //
    // IF THIS FAILS, the fix is to update the catalog above to match the
    // toolkit — never to loosen the comparison. A benchmark measuring a model
    // against tools Talaria does not have is a benchmark that flatters every
    // candidate.

    /// Resolved from `CARGO_MANIFEST_DIR`, not from the cwd: cargo test can be
    /// invoked from the repo root or from `api/`, and a cwd-relative path
    /// silently points at a different tree depending on which.
    fn mcp_source() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../mcp/src/index.ts")
    }

    fn source() -> String {
        fs::read_to_string(mcp_source())
            .expect("mcp/src/index.ts must be readable — the sync test does not silently skip")
    }

    /// The scans below run over a `Vec<char>` rather than slicing the `&str`:
    /// the source carries multi-byte punctuation (—, ’) and byte-offset
    /// arithmetic must never land mid-character, where a slice would panic
    /// instead of mis-hinting.
    fn char_index(source: &str, byte: usize) -> usize {
        source[..byte].chars().count()
    }

    /// Every `server.registerTool('name', { description: <string expr> …` in the
    /// source, with the description's leading literal segments concatenated and
    /// its first interpolation treated as the end of what a copy can promise.
    fn real_descriptions(source: &str) -> HashMap<String, String> {
        let re =
            Regex::new(r#"server\.registerTool\(\s*'([a-z_]+)',\s*\{\s*description:\s*"#).unwrap();
        let chars: Vec<char> = source.chars().collect();
        let mut out = HashMap::new();
        for caps in re.captures_iter(source) {
            let end = char_index(source, caps.get(0).unwrap().end());
            if let Some(literal) = read_string_expression(&chars, end) {
                out.insert(caps[1].to_string(), literal);
            }
        }
        out
    }

    /// Read a JS string expression — `'a'`, `"a"`, a template, or several joined
    /// by `+` — starting at `from`. Stops at the first `${`, because that is
    /// where a static copy stops being able to promise anything.
    fn read_string_expression(chars: &[char], from: usize) -> Option<String> {
        let mut i = from;
        let mut out = String::new();
        loop {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            let quote = chars.get(i).copied().unwrap_or('\0');
            if quote != '\'' && quote != '"' && quote != '`' {
                break;
            }
            i += 1;
            let mut done = false;
            while i < chars.len() {
                let ch = chars[i];
                if ch == '\\' {
                    // Only the escapes these descriptions actually use.
                    let next = chars.get(i + 1).copied().unwrap_or('\0');
                    out.push(match next {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    i += 2;
                    continue;
                }
                if ch == quote {
                    i += 1;
                    done = true;
                    break;
                }
                if quote == '`' && ch == '$' && chars.get(i + 1) == Some(&'{') {
                    return Some(out);
                }
                out.push(ch);
                i += 1;
            }
            if !done {
                return None;
            }
            // A `+` continues the same expression; anything else ends it.
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if chars.get(j) != Some(&'+') {
                break;
            }
            i = j + 1;
        }
        if out.is_empty() { None } else { Some(out) }
    }

    /// A registration's argument keys, in declaration order, and the subset the
    /// toolkit does not mark `.optional()`.
    #[derive(Debug, Default, Clone)]
    struct RealParams {
        keys: Vec<String>,
        required: Vec<String>,
    }

    /// Every `server.registerTool('name', { … inputSchema: { a: …, b: … } … })`'s
    /// ARGUMENT KEYS, and which of them are required.
    ///
    /// WHY THE NAMES AND NOT THE TYPES. A zod schema in another package cannot be
    /// evaluated here (that module boots an MCP server on import), and the parts
    /// of it a static reader can get exactly right are the keys and whether
    /// `.optional()` appears before the next key. That is also the part that was
    /// wrong: eight of sixteen tools carried invented argument names, so a model
    /// graded here learned a call production answers with a 400.
    ///
    /// The scan is deliberately shallow — top-level keys of the `inputSchema`
    /// object literal, at brace depth 1 — because that is what an MCP input
    /// schema is.
    fn real_parameters(source: &str) -> HashMap<String, RealParams> {
        let re = Regex::new(r#"server\.registerTool\(\s*'([a-z_]+)',\s*\{"#).unwrap();
        let key_re = Regex::new(r"^([A-Za-z_$][0-9A-Za-z_$]*)\s*:").unwrap();
        let chars: Vec<char> = source.chars().collect();
        let mut out = HashMap::new();
        for caps in re.captures_iter(source) {
            let start = caps.get(0).unwrap().start();
            // `inputSchema` must belong to THIS registration, not the next one's.
            let at = source[start..].find("inputSchema:").map(|p| start + p);
            let next_reg = source[start + 1..]
                .find("server.registerTool(")
                .map(|p| start + 1 + p);
            let at = match at {
                Some(at) if next_reg.is_none_or(|n| at < n) => at,
                _ => continue,
            };
            let open = match source[at..].find('{') {
                Some(p) => at + p,
                None => continue,
            };
            let mut keys: Vec<String> = Vec::new();
            let mut required: Vec<String> = Vec::new();
            let mut depth: i64 = 0;
            let mut i = char_index(source, open);
            while i < chars.len() {
                let ch = chars[i];
                if ch == '{' || ch == '[' || ch == '(' {
                    depth += 1;
                } else if ch == '}' || ch == ']' || ch == ')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else if depth == 1 {
                    // A key at the top level of the schema object: `name: z.…`
                    let window: String = chars[i..(i + 40).min(chars.len())].iter().collect();
                    let key = key_re.captures(&window);
                    let prev = chars[..i]
                        .iter()
                        .rev()
                        .find(|c| !c.is_whitespace())
                        .copied();
                    if let (Some(key), Some(prev)) = (key, prev)
                        && (prev == '{' || prev == ',')
                    {
                        let name = key[1].to_string();
                        let whole: Vec<char> = key.get(0).unwrap().as_str().chars().collect();
                        keys.push(name.clone());
                        i += whole.len() - 1;
                        // Everything up to the next top-level comma is this key's expression.
                        let mut j = i + 1;
                        let mut d: i64 = 0;
                        while j < chars.len() {
                            let c = chars[j];
                            if c == '{' || c == '[' || c == '(' {
                                d += 1;
                            } else if c == '}' || c == ']' || c == ')' {
                                if d == 0 {
                                    break;
                                }
                                d -= 1;
                            } else if c == ',' && d == 0 {
                                break;
                            }
                            j += 1;
                        }
                        let value: String = chars[i..j].iter().collect();
                        if !value.contains(".optional()") {
                            required.push(name);
                        }
                        i = j - 1;
                    }
                }
                i += 1;
            }
            out.insert(caps[1].to_string(), RealParams { keys, required });
        }
        out
    }

    /// The catalog's own argument keys and required set, from the JSON schema it
    /// carries. Insertion order (`serde_json`'s `preserve_order` keeps the
    /// object faithful to the literal), so the key-order assertion below
    /// compares like against like.
    fn params_of(tool: &SandboxTool) -> RealParams {
        let p = &tool.parameters;
        let keys = p
            .get("properties")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let required = p
            .get("required")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        RealParams { keys, required }
    }

    #[test]
    fn reads_the_real_registrations_at_all() {
        // The guard on the guard. If `registerTool` is ever spelled differently
        // this file must fail loudly rather than quietly assert over an empty map.
        let real = real_descriptions(&source());
        assert!(real.len() > 30);
        assert!(
            real.get("get_ticket")
                .is_some_and(|d| d.contains("Get a ticket in full"))
        );
    }

    #[test]
    fn offers_only_tools_the_toolkit_actually_registers() {
        // An invented tool is the worst failure available here: the model looks
        // capable in the benchmark and calls something that does not exist in
        // production.
        let real = real_descriptions(&source());
        let missing: Vec<&str> = TALARIA_TOOLS
            .iter()
            .map(|t| t.name)
            .filter(|n| !real.contains_key(*n))
            .collect();
        assert!(
            missing.is_empty(),
            "catalog carries tools the toolkit does not register: {missing:?}"
        );
    }

    #[test]
    fn carries_each_description_verbatim_up_to_the_first_interpolation() {
        let real = real_descriptions(&source());
        let drifted: Vec<&str> = TALARIA_TOOLS
            .iter()
            .filter(|t| {
                real.get(t.name)
                    .is_some_and(|want| !t.description.starts_with(want.as_str()))
            })
            .map(|t| t.name)
            .collect();
        assert!(
            drifted.is_empty(),
            "descriptions drifted from the toolkit: {drifted:?}"
        );
    }

    #[test]
    fn reads_the_real_input_schemas_at_all() {
        // The same guard, on the parameter scan.
        let real_args = real_parameters(&source());
        assert!(real_args.len() > 30);
        assert_eq!(real_args["add_time"].keys, ["taskId", "seconds"]);
        assert_eq!(real_args["message_user"].keys, ["to", "message"]);
        // A tool with no arguments must read as none rather than as unparsed.
        assert_eq!(real_args["list_boards"].keys, Vec::<String>::new());
    }

    #[test]
    fn names_every_argument_exactly_as_the_toolkit_does() {
        // THE FLATTERY THIS CLOSES. `comment(body)`, `add_time(minutes)`,
        // `read_channel(channel)`, `message_user(user, body)` — four of the eight
        // invented names the sandbox used to carry. A model that called them
        // "correctly" in the benchmark got a 400 in production, and the benchmark
        // called it competent. Never loosen this to make a tool pass: the fix is
        // to rename the sandbox's argument.
        let real_args = real_parameters(&source());
        let drifted: Vec<String> = TALARIA_TOOLS
            .iter()
            .filter_map(|t| {
                let want = real_args.get(t.name)?;
                let got = params_of(t);
                (got.keys != want.keys).then(|| {
                    format!(
                        "{}: sandbox has [{}], toolkit has [{}]",
                        t.name,
                        got.keys.join(", "),
                        want.keys.join(", ")
                    )
                })
            })
            .collect();
        assert!(
            drifted.is_empty(),
            "argument names drifted: {}",
            drifted.join("; ")
        );
    }

    #[test]
    fn requires_exactly_what_the_toolkit_requires() {
        // A sandbox that demands MORE fails a model for a call production accepts;
        // one that demands FEWER lets a model omit an argument the API rejects.
        let real_args = real_parameters(&source());
        let drifted: Vec<String> = TALARIA_TOOLS
            .iter()
            .filter_map(|t| {
                let want = real_args.get(t.name)?;
                let got = params_of(t);
                let mut a = got.required.clone();
                let mut b = want.required.clone();
                a.sort();
                b.sort();
                (a != b).then(|| {
                    format!(
                        "{}: sandbox requires [{}], toolkit requires [{}]",
                        t.name,
                        got.required.join(", "),
                        want.required.join(", ")
                    )
                })
            })
            .collect();
        assert!(
            drifted.is_empty(),
            "required sets drifted: {}",
            drifted.join("; ")
        );
    }

    #[test]
    fn models_every_tool_the_toolkit_registers() {
        // COVERAGE IS THE POINT, and it used to be sixteen of forty-four. Twenty-
        // eight tools an org's agents use every day had no simulated backend, no
        // fixture, and no way for anyone to notice. `scripts/check-invariants.mjs`
        // enforces the same thing in CI; this is the unit-level statement of it, so
        // a new registration fails in the suite a developer runs first.
        //
        // (The test's other half — every catalogued tool is BACKED by the
        // sandbox's dispatch — lives with the sandbox module, because
        // `BACKED_TOOLS` is sandbox.rs's export, not this catalog's.)
        let real = real_descriptions(&source());
        let unmodelled: Vec<&String> = real
            .keys()
            .filter(|n| !TALARIA_TOOLS.iter().any(|t| t.name == n.as_str()))
            .collect();
        assert!(
            unmodelled.is_empty(),
            "toolkit registrations with no catalogue entry: {unmodelled:?}"
        );
    }

    #[test]
    fn the_catalog_carries_fifty_one_distinct_tools() {
        // Fifty-one of the toolkit's fifty-one registrations, and a name that
        // appeared twice would shadow itself in `tools_named` and hand a harness
        // the wrong entry.
        assert_eq!(TALARIA_TOOLS.len(), 51);
        let names: HashSet<&str> = TALARIA_TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(names.len(), 51);
    }
}
