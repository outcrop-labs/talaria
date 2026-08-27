// THE TALARIA TOOLKIT, AS A MODEL SEES IT — for the fitness suite to hand over.
//
// WHOSE TOOLKIT THIS IS, said first because the answer is not "the platform's".
// Every tool in this file is registered by `mcp/src/index.ts` and reached OVER
// MCP from inside a Hermes agent container. A fleet agent holds them; a native
// platform harness — the Muse, the librarian, the briefer's structured turns —
// does not, and cannot, because nothing in the UI process speaks MCP to itself.
// The platform's own tool surface is a different, much smaller thing — one
// tool, `web_search`, held out by the research harness (`harness/defs/research.ts`
// owns its schema). Conflating the two is the mistake this header exists to
// prevent: a fitness report that says "this model can use Talaria's tools" has
// to mean one of those two surfaces, and they have different sizes, different
// callers and different failure modes.
//
//     hermes      44 tools, over MCP, inside the agent container   THIS FILE
//     platform     handed to a model by a harness via `toolDefs`   research.ts (search)
//
// WHY A COPY AND NOT AN IMPORT. The real registrations live in `mcp/src/index.ts`,
// a separate package whose module body STARTS AN MCP SERVER and whose every
// handler calls Talaria's HTTP API. Importing it from the fitness suite would
// boot a server and reach for a network on `import`. So the definitions are
// copied here — and `talaria-tools.sync.test.ts` reads `mcp/src/index.ts` and
// fails if a name, a DESCRIPTION or a PARAMETER NAME has drifted, which is what
// keeps "a copy of our base tools" true six months from now rather than merely
// true today.
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
import type { ToolDefinition } from '../../harness/transport'

/** WHERE A TOOL IS CALLED FROM. One value today and that is the finding, not an
 *  oversight: every tool below is MCP, and Talaria's own harnesses hold almost
 *  nothing. The platform's own native surface is the research harness's single
 *  `web_search` tool (`harness/defs/research.ts`). */
export type ToolCaller = 'hermes'

/** What the tool is FOR — the axis an admin reads a fitness report along, and
 *  the tab grouping on the model-fitness page. Kept here rather than in the UI
 *  so one tool cannot be filed under two headings in two places. */
export type ToolGroup = 'boards' | 'tickets' | 'ledger' | 'knowledge' | 'documents' | 'comms' | 'people' | 'google' | 'research' | 'governance'

/** `${...}` in a real description interpolates a shared clause (LIVE_ONLY,
 *  COMMENT_EXEMPTION). The sandbox spells those out; the sync test therefore
 *  compares the STATIC PREFIX — everything up to the first interpolation — which
 *  is the part a copy can be held to exactly. */
export interface SandboxTool extends ToolDefinition {
  /** The tool this stands in for, as registered in `mcp/src/index.ts`. */
  name: string
  caller: ToolCaller
  group: ToolGroup
  /** Talaria refuses this for an agent that is not somebody's PERSONAL
   *  ASSISTANT (401/403, server-side). The sandbox refuses it the same way, so a
   *  fixture can measure whether a model respects "personal assistants only"
   *  rather than discovering it in production. */
  assistantOnly?: true
  /** Needs a connected Google account. Staged as absent in some fixtures — a
   *  model that invents calendar entries when the integration is off is the
   *  failure worth catching. */
  needsGoogle?: true
}

const str = (description: string) => ({ type: 'string', description })
const num = (description: string) => ({ type: 'number', description })
const bool = (description: string) => ({ type: 'boolean', description })
const strs = (description: string) => ({ type: 'array', items: { type: 'string' }, description })
const oneOf = (values: readonly string[], description: string) => ({ type: 'string', enum: [...values], description })

const PRIORITIES = ['low', 'medium', 'high', 'urgent'] as const
const EFFORTS = ['xs', 's', 'm', 'l', 'xl'] as const
const COLORS = ['slate', 'bronze', 'green', 'amber', 'red', 'blue', 'purple', 'teal', 'pink', 'orange', 'lime', 'cyan', 'indigo', 'magenta', 'olive', 'brown'] as const

export const TALARIA_TOOLS: readonly SandboxTool[] = [
  // ── Boards & tickets ───────────────────────────────────────────────────────
  {
    name: 'list_boards',
    caller: 'hermes',
    group: 'boards',
    description:
      'List the boards this agent is allowed to work on. Boards a person archived are out of service and are not listed — every tool refuses them, so a board id you got from somewhere else and cannot see here will 403.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'list_tickets',
    caller: 'hermes',
    group: 'tickets',
    description:
      "List a board's tickets. Each carries status, priority, assignees (agent ids and humans as user:<id>), labels, due date, human estimate (estimatedHours), sub-task parent (parentId), and comment count. Archived tickets are not listed — a person took them off the table and they refuse every agent write. Optional filters narrow the list.",
    parameters: {
      type: 'object',
      properties: {
        boardId: str('Board id (from list_boards)'),
        status: str('Only this status (inbox/assigned/in_progress/blocked/quality_review/done)'),
        assignee: str('Only tickets assigned to this agent id or user:<id>'),
        label: str('Only tickets carrying this label'),
        overdue: bool('Only tickets past their due date and not done'),
        parentId: str('Only sub-tasks of this ticket'),
      },
      required: ['boardId'],
    },
  },
  {
    name: 'get_ticket',
    caller: 'hermes',
    group: 'tickets',
    description:
      'Get a ticket in full: fields (incl. human estimate, sub-task parentId, mixed assignees — agents by id, humans as user:<id>), comments, activity, watchers, reviews, dependencies. May include `workflows` — how this kind of work is done here; follow their instructions. Tickets may carry an `attachments` array (files + knowledge/artifact refs) — read a file with fetch_attachment.',
    parameters: { type: 'object', properties: { taskId: str('Ticket id') }, required: ['taskId'] },
  },
  {
    name: 'fetch_attachment',
    caller: 'hermes',
    group: 'tickets',
    description:
      'Read an attached file by upload id (from a ticket or chat attachments array; ref-type entries are knowledge docs/artifacts — use read_kb_doc/get_document for those). Text files come back as text, images as an image you can see; other binary formats report metadata only.',
    parameters: { type: 'object', properties: { uploadId: str('Attachment upload id') }, required: ['uploadId'] },
  },
  {
    name: 'create_ticket',
    caller: 'hermes',
    group: 'tickets',
    description:
      'Create a ticket on a board. It lands in the inbox for a human to assign — agents cannot assign work, and hour estimates stay human too (use effort for your own sizing). Pass parentId to create it as a SUB-TASK of an existing ticket (work breakdown; one level deep).',
    parameters: {
      type: 'object',
      properties: {
        boardId: str('Board id (from list_boards)'),
        title: str('Ticket title'),
        description: str('Markdown description'),
        priority: oneOf(PRIORITIES, 'Priority'),
        effort: oneOf(EFFORTS, 'T-shirt size estimate'),
        tags: strs('Labels'),
        dueDate: str('Due date, RFC3339'),
        startDate: str('When work should begin (Gantt bars run start → due)'),
        color: oneOf(COLORS, 'Color-code the ticket (shows on cards + gantt)'),
        parentId: str('Create as a sub-task of this ticket (same board, one level deep)'),
      },
      required: ['boardId', 'title'],
    },
  },
  {
    name: 'triage_ticket',
    caller: 'hermes',
    group: 'tickets',
    description:
      'Triage a ticket: adjust priority, effort, labels, description, due date — and move its status FORWARD. Forward means: start work you were already assigned (→ in_progress), park it (→ blocked), or hand it to review (→ quality_review). You cannot move it back. Restarting your own blocked ticket, taking anything out of quality_review, and any write at all to a ticket a person has taken off the table — all refuse with 403, and assigning or marking done are human-only.',
    parameters: {
      type: 'object',
      properties: {
        taskId: str('Ticket id'),
        title: str('New title'),
        description: str('New markdown description'),
        priority: oneOf(PRIORITIES, 'New priority'),
        effort: oneOf(EFFORTS, 'T-shirt size estimate'),
        tags: strs('Labels (replaces the set)'),
        dueDate: str('Due date, RFC3339'),
        startDate: str('When work begins (Gantt bars run start → due)'),
        color: oneOf(COLORS, 'Color-code the ticket (shows on cards + gantt)'),
        status: oneOf(
          ['in_progress', 'blocked', 'quality_review'],
          'Forward only. in_progress is legal from a ticket already assigned to you or already in progress — never from blocked. blocked and quality_review are one-way: a person moves it after that.',
        ),
      },
      required: ['taskId'],
    },
  },
  {
    name: 'comment',
    caller: 'hermes',
    group: 'tickets',
    description: 'Comment on a ticket. Comments stay allowed on a ticket in review, which is where anything further goes once you have reported an outcome.',
    parameters: { type: 'object', properties: { taskId: str('Ticket id'), content: str('Markdown comment body') }, required: ['taskId', 'content'] },
  },
  {
    name: 'report_outcome',
    caller: 'hermes',
    group: 'tickets',
    description:
      'Report the outcome of work on a ticket and hand it to the board\'s review column ("Quality review" unless your board renamed it). A human signs off on done. This is your LAST status move on that ticket: once it is in review, triage_ticket cannot take it back out, so add anything further as a comment. Only ever on a live ticket.',
    parameters: {
      type: 'object',
      properties: {
        taskId: str('Ticket id'),
        outcome: str('What was accomplished'),
        resolution: str('How it was resolved'),
        errorMessage: str('If the work failed, what went wrong'),
      },
      required: ['taskId', 'outcome'],
    },
  },
  {
    name: 'report_gap',
    caller: 'hermes',
    group: 'people',
    description:
      "Report a capability gap — use ONLY when you genuinely cannot do assigned work properly: a tool or access you're missing, or an org-specific process you'd be guessing at. NOT for small tasks you can simply do with your own judgment, and never twice for the same kind of work (repeats are deduplicated; the team is already aware). The team sees gaps ranked by recurrence and can build a workflow/skill to cover them. taskId is optional and gated: it writes a line onto that ticket, so a board you are not allowed on — or a ticket that is off the table — refuses it.",
    parameters: {
      type: 'object',
      properties: {
        kind: str('Short slug naming the kind of work, e.g. "invoice-reconciliation"'),
        missing: str("One line: what you can't do properly and why"),
        needs: str('What a flow would need to cover: steps, tools, access, decisions'),
        taskId: str('The ticket that surfaced the gap — keep it, so the report reaches the people who can see that work'),
      },
      required: ['kind', 'missing'],
    },
  },
  {
    name: 'add_time',
    caller: 'hermes',
    group: 'ledger',
    description: "Add time spent to a ticket's auto-accumulated total. Log it as you work. Only ever on a live ticket, and time you never logged before sign-off cannot be added afterwards.",
    parameters: { type: 'object', properties: { taskId: str('Ticket id'), seconds: num('Seconds of work to add') }, required: ['taskId', 'seconds'] },
  },
  {
    name: 'add_dependency',
    caller: 'hermes',
    group: 'tickets',
    description:
      'Mark a ticket as blocked by another ticket on the same board. The edge lands on BOTH tickets, so BOTH must be live: one that is off the table refuses with 403 on either side of the edge, and the error names which. Removing an edge is a human call.',
    parameters: { type: 'object', properties: { taskId: str('The blocked ticket'), dependsOnId: str('The ticket it depends on') }, required: ['taskId', 'dependsOnId'] },
  },
  {
    name: 'log_usage',
    caller: 'hermes',
    group: 'ledger',
    description: "Report the LLM tokens you burned working a ticket. Feeds the ticket's cost rollup and the fleet ledger. Log it as you go: only ever on a live ticket.",
    parameters: {
      type: 'object',
      properties: {
        taskId: str('Ticket id'),
        promptTokens: num('Prompt/input tokens used'),
        completionTokens: num('Completion/output tokens used'),
        tier: str('Model tier the work ran on (alias name), if not your main model'),
        estimated: bool('True when the counts are estimates'),
      },
      required: ['taskId', 'promptTokens', 'completionTokens'],
    },
  },

  // ── Knowledge ──────────────────────────────────────────────────────────────
  {
    name: 'web_search',
    caller: 'hermes',
    group: 'knowledge',
    description:
      "Search the live web and get back ranked results with their URLs. Use it for anything that happened or changed recently, anything you would otherwise be guessing at, and any claim a reader would expect a source for — then cite the URLs you used. This searches the PUBLIC web: for this organization's own documents and past conversations use search_knowledge instead.",
    parameters: {
      type: 'object',
      properties: {
        query: str('What to look up, as you would type it into a search engine'),
        limit: num('How many results to return (default 10)'),
      },
      required: ['query'],
    },
  },
  {
    name: 'describe_image',
    caller: 'hermes',
    group: 'knowledge',
    description:
      "Read an image you cannot see: attaches it to this workspace's vision model and returns a description in text. Use it for a screenshot, a photo, a chart or a scanned document — ask a specific question rather than \"what is this\", because the answer is written against your question. The reply is another model's reading of the image, not yours: quote it as a description, and if it says something is not visible, do not fill the gap yourself.",
    parameters: {
      type: 'object',
      properties: {
        uploadId: str('Attachment upload id (from a ticket or chat attachments array)'),
        question: str('What you need to know from the image'),
      },
      required: ['uploadId', 'question'],
    },
  },
  {
    name: 'search_knowledge',
    caller: 'hermes',
    group: 'knowledge',
    description:
      'Search the workspace and knowledge you have access to (org knowledgebase, departmental collections, and past chats/channels/plans/research). Use it to recall earlier discussion or ground an answer in real docs — e.g. "what did we decide about X a couple weeks ago". Returns ranked snippets with pointers.',
    parameters: { type: 'object', properties: { query: str('What to look for, in natural language'), limit: num('Max results (default 8)') }, required: ['query'] },
  },
  {
    name: 'list_kb_spaces',
    caller: 'hermes',
    group: 'knowledge',
    description: 'List knowledgebase spaces (folders) you can access — org/public ones plus any shared with you. Use to find where a doc lives.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'list_kb_docs',
    caller: 'hermes',
    group: 'knowledge',
    description: 'List the docs in a knowledgebase space (id + title + tree position). Get the spaceId from list_kb_spaces.',
    parameters: { type: 'object', properties: { spaceId: str('Space id (from list_kb_spaces)') }, required: ['spaceId'] },
  },
  {
    name: 'read_kb_doc',
    caller: 'hermes',
    group: 'knowledge',
    description: 'Read a knowledgebase doc in full — its markdown body + metadata. Use search_knowledge or list_kb_docs to find the id.',
    parameters: { type: 'object', properties: { docId: str('KB doc id') }, required: ['docId'] },
  },
  {
    name: 'create_kb_space',
    caller: 'hermes',
    group: 'knowledge',
    description:
      "Create a knowledge space (a top-level shelf like 'Company' or 'Engineering') when no existing space fits — check list_kb_spaces first. Find-or-create by name, so retries are safe.",
    parameters: {
      type: 'object',
      properties: { name: str('Space name'), description: str('One line on what belongs here'), icon: str('An emoji for the space') },
      required: ['name'],
    },
  },
  {
    name: 'create_kb_doc',
    caller: 'hermes',
    group: 'knowledge',
    description:
      'Create a NEW knowledgebase doc in a space you can read (get the spaceId from list_kb_spaces) — use this when asked to add something to the knowledge base. Your doc starts as a draft; a human marks it official before it grounds the org brain. Returns the doc id (keep editing with edit_kb_doc).',
    parameters: {
      type: 'object',
      properties: { spaceId: str('Space id (from list_kb_spaces)'), title: str('Doc title'), markdown: str('Initial markdown body'), parentId: str('Nest under this doc id (optional)') },
      required: ['spaceId', 'title'],
    },
  },
  {
    name: 'edit_kb_doc',
    caller: 'hermes',
    group: 'knowledge',
    description:
      "Edit a knowledgebase doc you've been granted Editor access to: replace its title and/or markdown body. Each save is versioned. You can't change sharing or officialize — a human owns that.",
    parameters: { type: 'object', properties: { docId: str('KB doc id'), title: str('New title'), markdown: str('New full markdown body') }, required: ['docId'] },
  },

  // ── Documents (artifacts) ──────────────────────────────────────────────────
  {
    name: 'create_document',
    caller: 'hermes',
    group: 'documents',
    description:
      "Create a document (a rich markdown doc, Talaria's Google-Docs equivalent). Use it to draft deliverables — reports, specs, briefs, memos. It's versioned, shareable, and hostable. Returns the document id (use update_document to keep editing it).",
    parameters: {
      type: 'object',
      properties: {
        title: str('Document title'),
        markdown: str('Initial body as markdown (headings, lists, tables, code, links)'),
        visibility: oneOf(['private', 'org', 'public'], "Who can see it. Personal assistants always create private-to-owner docs (this field is ignored); org agents default to 'org' — the workspace"),
        folder: str('File it under this folder name (find-or-create); omitted = your own folder'),
      },
      required: ['title'],
    },
  },
  {
    name: 'update_document',
    caller: 'hermes',
    group: 'documents',
    description: 'Edit a document you created (or were granted Editor access to): replace its title and/or markdown body. Each save is versioned.',
    parameters: {
      type: 'object',
      properties: { documentId: str('Document id (from create_document or list_documents)'), title: str('New title'), markdown: str('New full markdown body') },
      required: ['documentId'],
    },
  },
  {
    name: 'list_documents',
    caller: 'hermes',
    group: 'documents',
    description: 'List the documents (artifacts) you can access — the workspace-visible ones plus any shared with you.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'get_document',
    caller: 'hermes',
    group: 'documents',
    description: "Read one document's full content (markdown body + metadata).",
    parameters: { type: 'object', properties: { documentId: str('Document id') }, required: ['documentId'] },
  },
  {
    name: 'save_image_artifact',
    caller: 'hermes',
    group: 'documents',
    description:
      'Save an image from YOUR workspace (a file you created under /opt/data, e.g. a chart, screenshot, or meme) as a durable file artifact in Talaria — versioned, shareable, and visible on the Artifacts page. Optionally file it into a folder by name (created if missing). Use this when a human should keep the image beyond this conversation. Images only (png/jpg/gif/webp), max 25MB.',
    parameters: {
      type: 'object',
      properties: {
        path: str('Absolute path inside your workspace, e.g. /opt/data/charts/q3.png'),
        title: str('Artifact title (defaults to the filename)'),
        folder: str('Folder name to file it under, e.g. "Memes" (find-or-create)'),
      },
      required: ['path'],
    },
  },
  {
    name: 'export_to_google_doc',
    caller: 'hermes',
    group: 'documents',
    needsGoogle: true,
    description:
      "Export a document (or sheet/file artifact) into Google Drive as a native Google Doc / Sheet. It lands in the Google Drive of whoever you act for: if you're someone's personal assistant, their Drive; otherwise the shared organization Google account. Returns the Drive file link. Use it to hand a finished deliverable to a human in a format they can edit in Google. (Requires that Google account to be connected in Talaria.)",
    parameters: { type: 'object', properties: { documentId: str('Document/artifact id (from create_document or list_documents)') }, required: ['documentId'] },
  },

  // ── Comms ──────────────────────────────────────────────────────────────────
  {
    name: 'list_channels',
    caller: 'hermes',
    group: 'comms',
    description:
      'List the team channels you can see and read. A personal assistant sees its owner’s channels and DMs; any other agent sees the channels it has been added to. Returns id + name + topic — pass the id, never the name, to read/post.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'read_channel',
    caller: 'hermes',
    group: 'comms',
    description: 'Read recent messages in a channel you belong to. Pass sinceSeq to get only newer messages than a seq you already saw.',
    parameters: {
      type: 'object',
      properties: { channelId: str('Channel id (from list_channels)'), sinceSeq: num('Only messages with seq greater than this (default: all recent)') },
      required: ['channelId'],
    },
  },
  {
    name: 'post_to_channel',
    caller: 'hermes',
    group: 'comms',
    description: 'Post a message to a channel you belong to. Use @name to mention teammates. Post when you have something useful — progress, an answer, a blocker, a question — not chatter.',
    parameters: { type: 'object', properties: { channelId: str('Channel id (from list_channels)'), content: str('Markdown message') }, required: ['channelId', 'content'] },
  },
  {
    name: 'message_user',
    caller: 'hermes',
    group: 'comms',
    description:
      'Start (or continue) a direct conversation with a human teammate — your message lands in their chat with you plus an inbox notification. Reach for it when something genuinely needs THEIR attention now: work of theirs is blocked on you, you found something they should decide on, a deadline is about to slip. Not for status updates (comment on the ticket) or things the whole room should see (post_to_channel). Rate-limited per person per day — if it declines, respect the returned reason.',
    parameters: {
      type: 'object',
      properties: { to: str("The teammate's email (preferred) or exact display name"), message: str('Your message — lead with why this needs them') },
      required: ['to', 'message'],
    },
  },
  {
    name: 'list_teammates',
    caller: 'hermes',
    group: 'people',
    description:
      "The company directory: every teammate's name and email. Use it to resolve a person's name into an email for draft_email, add_board_member, or board/channel questions ('who is Priya?').",
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'report_problem',
    caller: 'hermes',
    group: 'people',
    description:
      'Something on YOUR side is broken (a tool errors, a connection refuses, credentials missing) and a teammate is affected. This alerts the workspace admins and files a Helpdesk ticket with the technical details. Use it INSTEAD of explaining endpoints/ports/errors to non-technical people — then relay the returned plain-language reassurance and offer what you can still do. taskId is optional and gated the same way report_gap\'s is: a board you are not allowed on — or a ticket that is off the table — refuses it.',
    parameters: {
      type: 'object',
      properties: {
        summary: str("One plain sentence: what is broken, from the user's point of view"),
        details: str('The full technical details for the admin (errors, endpoints, what you tried)'),
        context: str('What you were trying to do for whom'),
        taskId: str('The ticket you were working when it broke — keep it, so the report reaches the people who can see that work'),
      },
      required: ['summary'],
    },
  },

  // ── Google (reads are live; anything outbound is DRAFTED for a human) ───────
  {
    name: 'read_calendar',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      "Read upcoming Google Calendar events — your owner's if you're a personal assistant, otherwise the shared org calendar. Requires that Google account to be connected in Talaria.",
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'draft_calendar_event',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      "Draft a Google Calendar event (on your owner's calendar, or the shared org calendar). It is NOT created immediately — it's queued for a human to approve in Talaria first (confirm-sends). Times are RFC3339 (e.g. 2026-07-08T15:00:00Z), or YYYY-MM-DD with allDay=true.",
    parameters: {
      type: 'object',
      properties: {
        summary: str('Event title'),
        start: str('Start — RFC3339 dateTime or YYYY-MM-DD'),
        end: str('End — RFC3339 dateTime or YYYY-MM-DD'),
        description: str('Event description'),
        location: str('Where'),
        allDay: bool('True for a whole-day event (dates, not times)'),
        attendees: strs('Attendee emails'),
      },
      required: ['summary', 'start', 'end'],
    },
  },
  {
    name: 'read_recent_email',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      "Read recent Gmail (metadata + snippets) — your owner's if you're a personal assistant, otherwise the shared org mailbox. Optional Gmail search `q` (e.g. 'from:boss', 'is:unread'). Requires that Google account connected in Talaria.",
    parameters: { type: 'object', properties: { q: str("Gmail search query (default 'in:inbox')") }, required: [] },
  },
  {
    name: 'read_email',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      'Read ONE full email (headers + complete plain-text body) by its id — the id `read_recent_email` returns. Use it before summarizing a thread, quoting a question, or drafting a reply: the snippet is a teaser, not the message. Reads are free; only sending waits for approval.',
    parameters: { type: 'object', properties: { id: str('Message id from read_recent_email') }, required: ['id'] },
  },
  {
    name: 'list_labels',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      "List the Gmail labels on the account you act for — your owner's if you're a personal assistant, otherwise the shared org mailbox. Labels are Gmail's folders: a message is 'in a folder' by carrying its label, INBOX and UNREAD are system labels, and organizing mail means applying and removing them. Requires that Google account connected in Talaria.",
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'create_label',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      "Create a Gmail label (Gmail's name for a folder) on the account you act for, find-or-create: an existing label of the same name is returned as-is, so a retry is safe. Create the label BEFORE organize_emails refers to it — organizing refuses a label that does not exist.",
    parameters: { type: 'object', properties: { name: str('Label name, e.g. "Vendor"') }, required: ['name'] },
  },
  {
    name: 'organize_emails',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      'File, archive or mark-read emails by id — the ids read_recent_email returns. addLabels/removeLabels take label NAMES (from list_labels or create_label, or the system labels): removing INBOX archives a message (it stays in All Mail and keeps its labels), removing UNREAD marks it read. Applies immediately and reversibly — nothing is ever deleted, TRASH/SPAM are refused, and mail you file stays recoverable. Read before you file: sort by what the messages actually say, never by subject alone.',
    parameters: {
      type: 'object',
      properties: {
        ids: strs('Message ids from read_recent_email'),
        addLabels: strs('Label names to apply'),
        removeLabels: strs('Label names to remove (INBOX archives, UNREAD marks read)'),
      },
      required: ['ids'],
    },
  },
  {
    name: 'search_drive',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      "Find files in the Google Drive you act for — your owner's if you're a personal assistant, otherwise the shared org Drive. Returns names, types, modified dates and links. Read-only: locating and linking files, not changing them. Optional `q` matches file names.",
    parameters: { type: 'object', properties: { q: str('Name substring to match') }, required: [] },
  },
  {
    name: 'draft_email',
    caller: 'hermes',
    group: 'google',
    needsGoogle: true,
    description:
      "Draft an email to send (AS your owner, or from the shared org account). It is NOT sent immediately — it's queued for a human to approve in Talaria first (confirm-sends). Use this to prepare correspondence; a human reviews and sends with one click.",
    parameters: {
      type: 'object',
      properties: { to: str('Recipient email(s), comma-separated'), subject: str('Subject line'), body: str('Plain-text body'), cc: str('Cc'), bcc: str('Bcc') },
      required: ['to'],
    },
  },

  // ── Research ───────────────────────────────────────────────────────────────
  {
    name: 'research',
    caller: 'hermes',
    group: 'research',
    description:
      "Start cited web research on a question in your field. Modes: 'recon' (one fast pass — a cited answer, ~1 min), 'brief' (planned angles → a briefing document, a few minutes), 'expedition' (iterative deep dive → a full report, can take a while). Runs in the background — you get a runId; poll research_status, then read the report with get_document. Every claim in the report carries [n] citations. Use recon for quick facts mid-task; brief/expedition for real questions worth a document.",
    parameters: {
      type: 'object',
      properties: { question: str('What to research, in natural language — be specific'), mode: oneOf(['recon', 'brief', 'expedition'], "Depth (default 'brief')") },
      required: ['question'],
    },
  },
  {
    name: 'list_research',
    caller: 'hermes',
    group: 'research',
    description: 'Recent research runs across the workspace (yours and others) — question, mode, status, and the report documentId when done.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'research_status',
    caller: 'hermes',
    group: 'research',
    description:
      "Check a research run: status (queued/running/done/error), current phase, and — when done — the report's documentId (read it with get_document) plus the source list.",
    parameters: { type: 'object', properties: { runId: str('Run id (from research)') }, required: ['runId'] },
  },

  // ── Board governance (personal assistants only) ────────────────────────────
  {
    name: 'list_teams',
    caller: 'hermes',
    group: 'governance',
    assistantOnly: true,
    description: 'List the teams your owner belongs to (personal assistants only). Use the names with move_board_to_team.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'move_board_to_team',
    caller: 'hermes',
    group: 'governance',
    assistantOnly: true,
    description:
      "Move a board into a team, or back to Personal (personal assistants only, and only for boards your owner OWNS — it changes who can see the board). Team is matched by name; use 'personal' to remove it from any team.",
    parameters: {
      type: 'object',
      properties: { boardId: str('Board id (from list_boards)'), teamName: str("Team name (see list_teams), or 'personal' for no team") },
      required: ['boardId', 'teamName'],
    },
  },
  {
    name: 'list_board_members',
    caller: 'hermes',
    group: 'governance',
    description: "Who's on a board (people + roles). Any agent allowed on the board may read this.",
    parameters: { type: 'object', properties: { boardId: str('Board id (from list_boards)') }, required: ['boardId'] },
  },
  {
    name: 'add_board_member',
    caller: 'hermes',
    group: 'governance',
    assistantOnly: true,
    description:
      "Share a board with a person by email (personal assistants only; your owner needs owner/editor on the board). Role 'editor' can change the board, 'viewer' is read-only.",
    parameters: {
      type: 'object',
      properties: { boardId: str('Board id (from list_boards)'), email: str("The person's email"), role: oneOf(['editor', 'viewer'], "Access level (default 'editor')") },
      required: ['boardId', 'email'],
    },
  },
  {
    name: 'remove_board_member',
    caller: 'hermes',
    group: 'governance',
    assistantOnly: true,
    description:
      "Remove a person from a board by email (personal assistants only; your owner needs owner/editor on the board). The board owner can't be removed.",
    parameters: { type: 'object', properties: { boardId: str('Board id (from list_boards)'), email: str("The person's email") }, required: ['boardId', 'email'] },
  },
  {
    name: 'set_board_agents',
    caller: 'hermes',
    group: 'governance',
    assistantOnly: true,
    description:
      "Add or remove fleet agents on a board (personal assistants only; your owner needs owner/editor). Pass agent model names (e.g. 'engineer-engineering'). Returns the board's resulting agent policy.",
    parameters: {
      type: 'object',
      properties: { boardId: str('Board id (from list_boards)'), add: strs('Agent models to allow on the board'), remove: strs('Agent models to remove from the board') },
      required: ['boardId'],
    },
  },
] as const

export const talariaToolNames = (): string[] => TALARIA_TOOLS.map((t) => t.name)

/** The subset a given harness should be offered. A tool surface is a prompt:
 *  handing a briefing chat the ticket-triage tools measures its tolerance for
 *  irrelevant options rather than its ability to do the job.
 *
 *  UNKNOWN NAMES THROW rather than being silently dropped. A `dryRun.tools` list
 *  with a typo in it used to hand the model a SHORTER surface than its author
 *  meant, and the fixture then failed the model for not calling a tool it was
 *  never offered — our gap, scored as the model's. */
export const toolsNamed = (names: readonly string[]): SandboxTool[] => {
  const unknown = names.filter((n) => !TALARIA_TOOLS.some((t) => t.name === n))
  if (unknown.length) throw new Error(`no such Talaria tool: ${unknown.join(', ')} — check dryRun.tools against talaria-tools.ts`)
  return TALARIA_TOOLS.filter((t) => names.includes(t.name))
}

/** Every tool in one group, for a fixture that means "the whole documents
 *  surface" and should not have to keep a list in sync by hand. */
export const toolsInGroup = (...groups: readonly ToolGroup[]): string[] => TALARIA_TOOLS.filter((t) => groups.includes(t.group)).map((t) => t.name)
