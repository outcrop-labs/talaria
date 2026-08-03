#!/usr/bin/env node
// talaria-mcp — the agent-facing MCP server for Talaria's PM suite.
//
// Exposes ONLY the safe tools: agents can discover boards, read and create
// tickets, triage, comment, report outcomes, log time, and link dependencies.
// There is no assign tool and no complete tool — the guardrails hold by
// construction here, and the Talaria API enforces them again server-side
// (assign → 403, done → quality_review).
//
// The lifecycle is ONE-WAY for an agent, and the tool descriptions below say so
// because an agent that doesn't know burns turns on writes that 403
// (server/tasks.ts agentSafePatch):
//   • blocked → in_progress   refused — only a person restarts parked work
//   • anything out of review  refused — review is the human sign-off queue
//   • ANY write to a closed   refused (done / failed / cancelled). The ticket
//                             patch, add_time, log_usage and add_dependency
//                             alike — the last two enforce it on their own
//                             routes because they never reach updateTask.
//                             Comments stay open: that is the agent's channel
//                             on work it can no longer edit.
//
// Nothing here re-implements those rules — Talaria enforces them and this
// process only has to STATE them, so a description that drifts from the API is
// a bug in the description.
//
// Env:
//   TALARIA_URL        base URL of the Talaria app (default http://localhost:5273)
//   TALARIA_AGENT_KEY  this agent's own Talaria credential (required). Talaria
//                      resolves identity FROM it; the name below is a cross-check.
//   TALARIA_AGENT_NAME this agent's fleet model name (required in stdio mode)
//   MCP_HTTP_PORT      serve the whole fleet over HTTP instead of stdio
//   MCP_HTTP_HOST      comma-separated bind addresses for that listener
//                      (default: loopback + the docker bridges — see below)
//   TALARIA_MCP_VERIFY_PATH
//                      the Talaria route HTTP mode authenticates callers against
//                      (default /api/users). Load-bearing — see `verify()`.
//
// BUILD: this compiles to mcp/dist, which is gitignored and which the app
// SPAWNS (ui/src/server/mcp-service.ts). Nothing about editing this file makes
// the running toolkit change — `npm run build` here does, and scripts/dev.sh +
// scripts/setup.sh run it for you. A stale dist is silent: it serves last
// month's tool descriptions and last month's auth to the whole fleet.

import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { z } from 'zod'

const BASE = (process.env.TALARIA_URL ?? 'http://localhost:5273').replace(/\/+$/, '')
const KEY = process.env.TALARIA_AGENT_KEY ?? ''
const AGENT = process.env.TALARIA_AGENT_NAME ?? ''
/** When set, serve MCP over streamable HTTP for the WHOLE fleet instead of
 *  stdio for one agent. Multi-tenant, so it carries NO identity of its own:
 *  each request's credential is passed straight through to Talaria, which is
 *  the only thing that can say who the caller is. */
const HTTP_PORT = Number(process.env.MCP_HTTP_PORT ?? 0)

function makeApi(agent: string, key: string) {
  return async function api(method: string, path: string, body?: unknown): Promise<unknown> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      'x-api-key': key,
      'x-agent-name': agent,
      ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  const text = await res.text()
  let data: unknown
  try {
    data = JSON.parse(text)
  } catch {
    data = text
  }
  if (!res.ok) {
    const msg = typeof data === 'object' && data && 'error' in data ? (data as { error: string }).error : text
    throw new Error(`Talaria API ${res.status}: ${msg}`)
  }
  return data
  }
}

const ok = (data: unknown) => ({ content: [{ type: 'text' as const, text: JSON.stringify(data, null, 2) }] })

// One server instance per identity: stdio builds it once for the env agent;
// HTTP mode builds per request (stateless) for the connecting agent, with the
// credential THAT agent presented.
function buildServer(agent: string, key: string): McpServer {
  const api = makeApi(agent, key)
  const server = new McpServer({ name: 'talaria', version: '0.1.0' })

// Statuses an agent may set. No 'assigned' (humans assign), no 'done' (the API
// would coerce it to quality_review anyway — we don't even offer it).
const AGENT_STATUSES = ['in_progress', 'blocked', 'quality_review'] as const
const PRIORITIES = ['low', 'medium', 'high', 'urgent'] as const
const EFFORTS = ['xs', 's', 'm', 'l', 'xl'] as const

server.registerTool(
  'list_boards',
  {
    description: 'List the boards this agent is allowed to work on.',
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/boards')),
)

server.registerTool(
  'list_tickets',
  {
    description:
      "List a board's tickets. Each carries status, priority, assignees (agent ids and humans as user:<id>), labels, due date, human estimate (estimatedHours), sub-task parent (parentId), and comment count. Optional filters narrow the list.",
    inputSchema: {
      boardId: z.string().describe('Board id (from list_boards)'),
      status: z.string().optional().describe('Only this status (inbox/assigned/in_progress/blocked/quality_review/done)'),
      assignee: z.string().optional().describe('Only tickets assigned to this agent id or user:<id>'),
      label: z.string().optional().describe('Only tickets carrying this label'),
      overdue: z.boolean().optional().describe('Only tickets past their due date and not done'),
      parentId: z.string().optional().describe('Only sub-tasks of this ticket'),
    },
  },
  async ({ boardId, status, assignee, label, overdue, parentId }) => {
    const res = (await api('GET', `/api/boards/${encodeURIComponent(boardId)}/tasks`)) as {
      tasks: Array<{ status: string; assignees: string[]; tags: string[]; dueDate: string | null; parentId: string | null }>
    }
    const now = Date.now()
    const tasks = res.tasks.filter(
      (t) =>
        (!status || t.status === status) &&
        (!assignee || t.assignees.includes(assignee)) &&
        (!label || t.tags.includes(label)) &&
        (!parentId || t.parentId === parentId) &&
        (!overdue || (!!t.dueDate && Date.parse(t.dueDate) < now && !['done', 'cancelled'].includes(t.status))),
    )
    return ok({ tasks })
  },
)

server.registerTool(
  'get_ticket',
  {
    description:
      'Get a ticket in full: fields (incl. human estimate, sub-task parentId, mixed assignees — agents by id, humans as user:<id>), comments, activity, watchers, reviews, dependencies. May include `workflows` — how this kind of work is done here; follow their instructions. Tickets may carry an `attachments` array (files + knowledge/artifact refs) — read a file with fetch_attachment.',
    inputSchema: { taskId: z.string().describe('Ticket id') },
  },
  async ({ taskId }) => ok(await api('GET', `/api/tasks/${encodeURIComponent(taskId)}`)),
)

const TEXTUAL = /^text\/|^application\/(json|xml|javascript|x-yaml|yaml|csv|toml|sql|markdown)|(\+json|\+xml)$/
const MAX_INLINE = 4 * 1024 * 1024 // keep image payloads model-sized
const MAX_TEXT = 50_000

server.registerTool(
  'fetch_attachment',
  {
    description:
      'Read an attached file by upload id (from a ticket or chat attachments array; ref-type entries are knowledge docs/artifacts — use read_kb_doc/get_document for those). Text files come back as text, images as an image you can see; other binary formats report metadata only.',
    inputSchema: { uploadId: z.string().describe('Attachment upload id') },
  },
  async ({ uploadId }) => {
    // Raw bytes, not JSON — bypass the shared api() helper.
    const res = await fetch(`${BASE}/api/uploads/${encodeURIComponent(uploadId)}`, {
      headers: { 'x-api-key': key, 'x-agent-name': agent },
    })
    if (!res.ok) throw new Error(`Talaria API ${res.status}: ${(await res.text()).slice(0, 300)}`)
    const mime = res.headers.get('content-type') ?? 'application/octet-stream'
    const filename = /filename="([^"]*)"/.exec(res.headers.get('content-disposition') ?? '')?.[1] ?? uploadId
    const bytes = Buffer.from(await res.arrayBuffer())
    if (TEXTUAL.test(mime)) {
      const text = bytes.toString('utf8')
      return ok({
        filename,
        mime,
        size: bytes.byteLength,
        content: text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT)}\n[clipped at ${MAX_TEXT} chars]` : text,
      })
    }
    if (mime.startsWith('image/')) {
      if (bytes.byteLength > MAX_INLINE) {
        return ok({ filename, mime, size: bytes.byteLength, note: 'image too large to inline (4 MB cap)' })
      }
      return {
        content: [
          { type: 'text' as const, text: JSON.stringify({ filename, mime, size: bytes.byteLength }) },
          { type: 'image' as const, data: bytes.toString('base64'), mimeType: mime },
        ],
      }
    }
    return ok({
      filename,
      mime,
      size: bytes.byteLength,
      note: 'binary format — contents cannot be inlined. Tell the requester what you can and cannot read, or ask a teammate for a text export.',
    })
  },
)

server.registerTool(
  'create_ticket',
  {
    description:
      'Create a ticket on a board. It lands in the inbox for a human to assign — agents cannot assign work, and hour estimates stay human too (use effort for your own sizing). Pass parentId to create it as a SUB-TASK of an existing ticket (work breakdown; one level deep).',
    inputSchema: {
      boardId: z.string().describe('Board id (from list_boards)'),
      title: z.string().min(1).max(300),
      description: z.string().max(20_000).optional().describe('Markdown description'),
      priority: z.enum(PRIORITIES).optional(),
      effort: z.enum(EFFORTS).optional().describe('T-shirt size estimate'),
      tags: z.array(z.string().max(40)).max(20).optional().describe('Labels'),
      dueDate: z.string().datetime().optional(),
      startDate: z.string().datetime().optional().describe('When work should begin (Gantt bars run start → due)'),
      color: z.enum(['slate', 'bronze', 'green', 'amber', 'red', 'blue', 'purple', 'teal', 'pink', 'orange', 'lime', 'cyan', 'indigo', 'magenta', 'olive', 'brown']).optional().describe('Color-code the ticket (shows on cards + gantt)'),
      parentId: z.string().optional().describe('Create as a sub-task of this ticket (same board, one level deep)'),
    },
  },
  async ({ boardId, ...body }) => ok(await api('POST', `/api/boards/${encodeURIComponent(boardId)}/tasks`, body)),
)


server.registerTool(
  'triage_ticket',
  {
    description:
      'Triage a ticket: adjust priority, effort, labels, description, due date — and move its status FORWARD. Forward means: start work you were already assigned (→ in_progress), park it (→ blocked), or hand it to review (→ quality_review). You cannot move it back. Restarting your own blocked ticket, taking anything out of quality_review, and any write at all to a closed ticket (done / failed / cancelled) are a person\'s call and return 403 — so once you have parked or reported, stop moving the ticket and use comment instead, which stays open on work you can no longer edit. Assigning and marking done are human-only.',
    inputSchema: {
      taskId: z.string(),
      title: z.string().min(1).max(300).optional(),
      description: z.string().max(20_000).optional(),
      priority: z.enum(PRIORITIES).optional(),
      effort: z.enum(EFFORTS).optional(),
      tags: z.array(z.string().max(40)).max(20).optional().describe('Labels (replaces the set)'),
      dueDate: z.string().datetime().optional(),
      startDate: z.string().datetime().optional().describe('When work begins (Gantt bars run start → due)'),
      color: z.enum(['slate', 'bronze', 'green', 'amber', 'red', 'blue', 'purple', 'teal', 'pink', 'orange', 'lime', 'cyan', 'indigo', 'magenta', 'olive', 'brown']).optional().describe('Color-code the ticket (shows on cards + gantt)'),
      status: z
        .enum(AGENT_STATUSES)
        .optional()
        .describe(
          'Forward only. in_progress is legal from a ticket already assigned to you or already in progress — never from blocked. blocked and quality_review are one-way: a person moves it after that.',
        ),
    },
  },
  async ({ taskId, ...body }) => ok(await api('PUT', `/api/tasks/${encodeURIComponent(taskId)}`, body)),
)

server.registerTool(
  'comment',
  {
    description: 'Comment on a ticket.',
    inputSchema: {
      taskId: z.string(),
      content: z.string().min(1).max(20_000).describe('Markdown comment body'),
    },
  },
  async ({ taskId, content }) => ok(await api('POST', `/api/tasks/${encodeURIComponent(taskId)}/comments`, { content })),
)

server.registerTool(
  'report_outcome',
  {
    description:
      "Report the outcome of work on a ticket and hand it to the board's review column (\"Quality review\" unless your board renamed it). A human signs off on done. This is your LAST status move on that ticket: once it is in review, triage_ticket cannot take it back out, so add anything further as a comment.",
    inputSchema: {
      taskId: z.string(),
      outcome: z.string().max(50_000).describe('What was accomplished'),
      resolution: z.string().max(50_000).optional().describe('How it was resolved'),
      errorMessage: z.string().max(50_000).optional().describe('If the work failed, what went wrong'),
    },
  },
  async ({ taskId, ...body }) => {
    // The review column belongs to the BOARD, not to this file. 'quality_review'
    // is only the default board's key; a board that renamed or replaced its
    // review column has no such status, and updateTask rejects the patch with
    // "is not a status on this board" (400) BEFORE the agent invariant ever
    // runs — so the hardcoded literal 400s exactly where handing work over
    // matters most, and no endpoint tells an agent what the column is called.
    //
    // Ask the invariant instead of guessing: agentSafePatch redirects EVERY
    // terminal move to that board's review key, and the off-board terminals
    // ('failed' / 'cancelled') are legal on every board by construction. So on
    // the one failure that means "this board is named differently", re-send a
    // terminal move and let the server pick the column. The ticket lands in
    // review either way; only the spelling was ever in question.
    const put = (status: string) => api('PUT', `/api/tasks/${encodeURIComponent(taskId)}`, { ...body, status })
    try {
      return ok(await put('quality_review'))
    } catch (e) {
      if (!/is not a status on this board/.test((e as Error).message)) throw e
      return ok(await put('cancelled'))
    }
  },
)

server.registerTool(
  'report_gap',
  {
    description:
      "Report a capability gap — use ONLY when you genuinely cannot do assigned work properly: a tool or access you're missing, or an org-specific process you'd be guessing at. NOT for small tasks you can simply do with your own judgment, and never twice for the same kind of work (repeats are deduplicated; the team is already aware). The team sees gaps ranked by recurrence and can build a workflow/skill to cover them.",
    inputSchema: {
      kind: z.string().min(2).max(80).describe('Short slug naming the kind of work, e.g. "invoice-reconciliation"'),
      missing: z.string().min(5).max(300).describe("One line: what you can't do properly and why"),
      needs: z.string().max(5000).optional().describe('What a flow would need to cover: steps, tools, access, decisions'),
      taskId: z.string().optional().describe('The ticket that surfaced the gap'),
    },
  },
  async (body) => ok(await api('POST', '/api/agent/gap', body)),
)

server.registerTool(
  'add_time',
  {
    description:
      "Add time spent to a ticket's auto-accumulated total. Log it as you work. Only an OPEN ticket accepts it — a closed one (done / failed / cancelled) refuses every agent write including this, with 403, so time you never logged before sign-off cannot be added afterwards. Still fine while the ticket sits in blocked or quality_review.",
    inputSchema: {
      taskId: z.string(),
      seconds: z.number().int().min(1).max(86_400).describe('Seconds of work to add'),
    },
  },
  async ({ taskId, seconds }) => ok(await api('PUT', `/api/tasks/${encodeURIComponent(taskId)}`, { addTimeSpentSeconds: seconds })),
)

server.registerTool(
  'search_knowledge',
  {
    description:
      'Search the workspace and knowledge you have access to (org knowledgebase, departmental collections, and past chats/channels/plans/research). Use it to recall earlier discussion or ground an answer in real docs — e.g. "what did we decide about X a couple weeks ago". Returns ranked snippets with pointers.',
    inputSchema: {
      query: z.string().min(1).describe('What to look for, in natural language'),
      limit: z.number().int().min(1).max(20).optional().describe('Max results (default 8)'),
    },
  },
  async ({ query, limit }) => ok(await api('POST', '/api/rag/search', { query, limit })),
)

// ── Documents (artifacts) ─────────────────────────────────────────────────
// A "document" here is a doc-kind artifact: rich markdown, versioned, shareable,
// and hostable — Talaria's Google-Docs equivalent. Agents author these.
server.registerTool(
  'create_document',
  {
    description:
      "Create a document (a rich markdown doc, Talaria's Google-Docs equivalent). Use it to draft deliverables — reports, specs, briefs, memos. It's versioned, shareable, and hostable. Returns the document id (use update_document to keep editing it).",
    inputSchema: {
      title: z.string().min(1).max(200).describe('Document title'),
      markdown: z.string().max(1_000_000).optional().describe('Initial body as markdown (headings, lists, tables, code, links)'),
      visibility: z.enum(['private', 'org', 'public']).optional().describe("Who can see it. Personal assistants always create private-to-owner docs (this field is ignored); org agents default to 'org' — the workspace"),
      folder: z.string().max(120).optional().describe('File it under this folder name (find-or-create); omitted = your own folder'),
    },
  },
  async ({ title, markdown, visibility, folder }) => ok(await api('POST', '/api/artifacts', { kind: 'doc', title, body: markdown, visibility, folder })),
)

server.registerTool(
  'update_document',
  {
    description: 'Edit a document you created (or were granted Editor access to): replace its title and/or markdown body. Each save is versioned.',
    inputSchema: {
      documentId: z.string().describe('Document id (from create_document or list_documents)'),
      title: z.string().max(200).optional(),
      markdown: z.string().max(1_000_000).optional().describe('New full markdown body'),
    },
  },
  async ({ documentId, title, markdown }) => ok(await api('PUT', `/api/artifacts/${encodeURIComponent(documentId)}`, { title, body: markdown })),
)

server.registerTool(
  'list_documents',
  {
    description: 'List the documents (artifacts) you can access — the workspace-visible ones plus any shared with you.',
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/artifacts')),
)

server.registerTool(
  'get_document',
  {
    description: "Read one document's full content (markdown body + metadata).",
    inputSchema: { documentId: z.string().describe('Document id') },
  },
  async ({ documentId }) => ok(await api('GET', `/api/artifacts/${encodeURIComponent(documentId)}`)),
)

server.registerTool(
  'save_image_artifact',
  {
    description:
      'Save an image from YOUR workspace (a file you created under /opt/data, e.g. a chart, screenshot, or meme) as a durable file artifact in Talaria — versioned, shareable, and visible on the Artifacts page. Optionally file it into a folder by name (created if missing). Use this when a human should keep the image beyond this conversation. Images only (png/jpg/gif/webp), max 25MB.',
    inputSchema: {
      path: z.string().min(1).max(1000).describe('Absolute path inside your workspace, e.g. /opt/data/charts/q3.png'),
      title: z.string().max(200).optional().describe('Artifact title (defaults to the filename)'),
      folder: z.string().max(120).optional().describe('Folder name to file it under, e.g. "Memes" (find-or-create)'),
    },
  },
  async ({ path, title, folder }) =>
    ok(await api('POST', `/api/agent-media/${encodeURIComponent(agent)}/save`, { path, title, folder })),
)

server.registerTool(
  'export_to_google_doc',
  {
    description:
      "Export a document (or sheet/file artifact) into Google Drive as a native Google Doc / Sheet. It lands in the Google Drive of whoever you act for: if you're someone's personal assistant, their Drive; otherwise the shared organization Google account. Returns the Drive file link. Use it to hand a finished deliverable to a human in a format they can edit in Google. (Requires that Google account to be connected in Talaria.)",
    inputSchema: {
      documentId: z.string().describe('Document/artifact id (from create_document or list_documents)'),
    },
  },
  async ({ documentId }) => ok(await api('POST', `/api/artifacts/${encodeURIComponent(documentId)}/export/google`)),
)

// ── Channels (team chat you've been added to) ──────────────────────────────
// You can read and post in channels a human has added you to. Use this to
// collaborate with the team — answer questions, share progress, ask when blocked.
server.registerTool(
  'list_channels',
  {
    description: 'List the team channels you (this agent) have been added to. Returns id + name + topic.',
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/channels')),
)

server.registerTool(
  'read_channel',
  {
    description: 'Read recent messages in a channel you belong to. Pass sinceSeq to get only newer messages than a seq you already saw.',
    inputSchema: {
      channelId: z.string().describe('Channel id (from list_channels)'),
      sinceSeq: z.number().int().optional().describe('Only messages with seq greater than this (default: all recent)'),
    },
  },
  async ({ channelId, sinceSeq }) =>
    ok(await api('GET', `/api/channels/${encodeURIComponent(channelId)}/messages${sinceSeq !== undefined ? `?since=${sinceSeq}` : ''}`)),
)

server.registerTool(
  'post_to_channel',
  {
    description:
      "Post a message to a channel you belong to. Use @name to mention teammates. Post when you have something useful — progress, an answer, a blocker, a question — not chatter.",
    inputSchema: {
      channelId: z.string().describe('Channel id (from list_channels)'),
      content: z.string().min(1).max(20_000).describe('Markdown message'),
    },
  },
  async ({ channelId, content }) => ok(await api('POST', `/api/channels/${encodeURIComponent(channelId)}/messages`, { content })),
)

// ── Knowledgebase (browse + read + edit granted docs) ──────────────────────
// search_knowledge finds things across everything; these let you navigate the
// curated KB directly and edit docs you've been granted Editor on.
server.registerTool(
  'list_kb_spaces',
  {
    description: 'List knowledgebase spaces (folders) you can access — org/public ones plus any shared with you. Use to find where a doc lives.',
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/kb/spaces')),
)

server.registerTool(
  'list_kb_docs',
  {
    description: "List the docs in a knowledgebase space (id + title + tree position). Get the spaceId from list_kb_spaces.",
    inputSchema: { spaceId: z.string().describe('Space id (from list_kb_spaces)') },
  },
  async ({ spaceId }) => ok(await api('GET', `/api/kb/spaces/${encodeURIComponent(spaceId)}/docs`)),
)

server.registerTool(
  'read_kb_doc',
  {
    description: "Read a knowledgebase doc in full — its markdown body + metadata. Use search_knowledge or list_kb_docs to find the id.",
    inputSchema: { docId: z.string().describe('KB doc id') },
  },
  async ({ docId }) => ok(await api('GET', `/api/kb/docs/${encodeURIComponent(docId)}`)),
)

server.registerTool(
  'create_kb_space',
  {
    description:
      "Create a knowledge space (a top-level shelf like 'Company' or 'Engineering') when no existing space fits — check list_kb_spaces first. Find-or-create by name, so retries are safe.",
    inputSchema: {
      name: z.string().min(1).max(80).describe('Space name'),
      description: z.string().max(400).optional().describe('One line on what belongs here'),
      icon: z.string().max(8).optional().describe('An emoji for the space'),
    },
  },
  async (input) => ok(await api('POST', '/api/kb/spaces', input)),
)

server.registerTool(
  'create_kb_doc',
  {
    description:
      "Create a NEW knowledgebase doc in a space you can read (get the spaceId from list_kb_spaces) — use this when asked to add something to the knowledge base. Your doc starts as a draft; a human marks it official before it grounds the org brain. Returns the doc id (keep editing with edit_kb_doc).",
    inputSchema: {
      spaceId: z.string().describe('Space id (from list_kb_spaces)'),
      title: z.string().min(1).max(200).describe('Doc title'),
      markdown: z.string().max(500_000).optional().describe('Initial markdown body'),
      parentId: z.string().optional().describe('Nest under this doc id (optional)'),
    },
  },
  async ({ spaceId, title, markdown, parentId }) =>
    ok(await api('POST', `/api/kb/spaces/${encodeURIComponent(spaceId)}/docs`, { title, body: markdown, parentId })),
)

server.registerTool(
  'edit_kb_doc',
  {
    description:
      "Edit a knowledgebase doc you've been granted Editor access to: replace its title and/or markdown body. Each save is versioned. You can't change sharing or officialize — a human owns that.",
    inputSchema: {
      docId: z.string().describe('KB doc id'),
      title: z.string().max(200).optional(),
      markdown: z.string().max(500_000).optional().describe('New full markdown body'),
    },
  },
  async ({ docId, title, markdown }) => ok(await api('PUT', `/api/kb/docs/${encodeURIComponent(docId)}`, { title, body: markdown })),
)

// ── Calendar & Gmail (acts as your owner, or the shared org account) ────────
// Reads are immediate; anything outbound is DRAFTED and waits for a human to
// approve in Talaria before it sends. A personal assistant acts as its owner
// (the owner approves); a general agent acts on the shared org account (an admin
// approves).
server.registerTool(
  'read_calendar',
  {
    description:
      "Read upcoming Google Calendar events — your owner's if you're a personal assistant, otherwise the shared org calendar. Requires that Google account to be connected in Talaria.",
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/integrations/google/agent/calendar')),
)

server.registerTool(
  'draft_calendar_event',
  {
    description:
      "Draft a Google Calendar event (on your owner's calendar, or the shared org calendar). It is NOT created immediately — it's queued for a human to approve in Talaria first (confirm-sends). Times are RFC3339 (e.g. 2026-07-08T15:00:00Z), or YYYY-MM-DD with allDay=true.",
    inputSchema: {
      summary: z.string().min(1).max(500).describe('Event title'),
      start: z.string().describe('Start — RFC3339 dateTime or YYYY-MM-DD'),
      end: z.string().describe('End — RFC3339 dateTime or YYYY-MM-DD'),
      description: z.string().max(8000).optional(),
      location: z.string().max(500).optional(),
      allDay: z.boolean().optional(),
      attendees: z.array(z.string()).max(50).optional().describe('Attendee emails'),
    },
  },
  async (args) => ok(await api('POST', '/api/integrations/google/agent/calendar', args)),
)

server.registerTool(
  'read_recent_email',
  {
    description:
      "Read recent Gmail (metadata + snippets) — your owner's if you're a personal assistant, otherwise the shared org mailbox. Optional Gmail search `q` (e.g. 'from:boss', 'is:unread'). Requires that Google account connected in Talaria.",
    inputSchema: { q: z.string().optional().describe("Gmail search query (default 'in:inbox')") },
  },
  async ({ q }) => ok(await api('GET', `/api/integrations/google/agent/gmail${q ? `?q=${encodeURIComponent(q)}` : ''}`)),
)

server.registerTool(
  'draft_email',
  {
    description:
      "Draft an email to send (AS your owner, or from the shared org account). It is NOT sent immediately — it's queued for a human to approve in Talaria first (confirm-sends). Use this to prepare correspondence; a human reviews and sends with one click.",
    inputSchema: {
      to: z.string().describe('Recipient email(s), comma-separated'),
      subject: z.string().max(500).optional(),
      body: z.string().max(50_000).optional().describe('Plain-text body'),
      cc: z.string().optional(),
      bcc: z.string().optional(),
    },
  },
  async (args) => ok(await api('POST', '/api/integrations/google/agent/gmail', args)),
)

server.registerTool(
  'log_usage',
  {
    description:
      "Report the LLM tokens you burned working a ticket. Feeds the ticket's cost rollup and the fleet ledger. Log it as you go: only an OPEN ticket accepts it — a closed one (done / failed / cancelled) refuses with 403, so spend you never reported before sign-off cannot be attached afterwards. Still fine while the ticket sits in blocked or quality review.",
    inputSchema: {
      taskId: z.string(),
      promptTokens: z.number().int().min(0).describe('Prompt/input tokens used'),
      completionTokens: z.number().int().min(0).describe('Completion/output tokens used'),
      tier: z.string().optional().describe('Model tier the work ran on (alias name), if not your main model'),
      estimated: z.boolean().optional().describe('True when the counts are estimates'),
    },
  },
  async ({ taskId, ...body }) => ok(await api('POST', `/api/tasks/${encodeURIComponent(taskId)}/usage`, body)),
)

server.registerTool(
  'add_dependency',
  {
    description:
      'Mark a ticket as blocked by another ticket on the same board. Both tickets must be OPEN — the edge shows on both, and a closed one (done / failed / cancelled) refuses with 403 like every other agent write. Removing an edge is a human call.',
    inputSchema: {
      taskId: z.string().describe('The blocked ticket'),
      dependsOnId: z.string().describe('The ticket it depends on'),
    },
  },
  async ({ taskId, dependsOnId }) => ok(await api('POST', `/api/tasks/${encodeURIComponent(taskId)}/dependencies`, { dependsOnId })),
)

server.registerTool(
  'list_teammates',
  {
    description:
      "The company directory: every teammate's name and email. Use it to resolve a person's name into an email for draft_email, add_board_member, or board/channel questions ('who is Priya?').",
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/users')),
)

server.registerTool(
  'message_user',
  {
    description:
      "Start (or continue) a direct conversation with a human teammate — your message lands in their chat with you plus an inbox notification. Reach for it when something genuinely needs THEIR attention now: work of theirs is blocked on you, you found something they should decide on, a deadline is about to slip. Not for status updates (comment on the ticket) or things the whole room should see (post_to_channel). Rate-limited per person per day — if it declines, respect the returned reason.",
    inputSchema: {
      to: z.string().min(1).max(200).describe("The teammate's email (preferred) or exact display name"),
      message: z.string().min(1).max(4000).describe('Your message — lead with why this needs them'),
    },
  },
  async (args) => ok(await api('POST', '/api/agent/message-user', args)),
)

server.registerTool(
  'report_problem',
  {
    description:
      "Something on YOUR side is broken (a tool errors, a connection refuses, credentials missing) and a teammate is affected. This alerts the workspace admin and files a Helpdesk ticket with the technical details. Use it INSTEAD of explaining endpoints/ports/errors to non-technical people — then relay the returned plain-language reassurance and offer what you can still do.",
    inputSchema: {
      summary: z.string().min(5).max(300).describe('One plain sentence: what is broken, from the user\'s point of view'),
      details: z.string().max(20_000).optional().describe('The full technical details for the admin (errors, endpoints, what you tried)'),
      context: z.string().max(500).optional().describe('What you were trying to do for whom'),
    },
  },
  async (args) => ok(await api('POST', '/api/agent/problem', args)),
)

// ── Research (Recon / Brief / Expedition) ────────────────────────────────────
// Cited web research, run server-side by YOUR persona + a search model — your
// chat context stays clean. Results are org-visible report documents.
server.registerTool(
  'research',
  {
    description:
      "Start cited web research on a question in your field. Modes: 'recon' (one fast pass — a cited answer, ~1 min), 'brief' (planned angles → a briefing document, a few minutes), 'expedition' (iterative deep dive → a full report, can take a while). Runs in the background — you get a runId; poll research_status, then read the report with get_document. Every claim in the report carries [n] citations. Use recon for quick facts mid-task; brief/expedition for real questions worth a document.",
    inputSchema: {
      question: z.string().min(8).max(4000).describe('What to research, in natural language — be specific'),
      mode: z.enum(['recon', 'brief', 'expedition']).optional().describe("Depth (default 'brief')"),
    },
  },
  async ({ question, mode }) => ok(await api('POST', '/api/research', { question, mode })),
)

server.registerTool(
  'list_research',
  {
    description: 'Recent research runs across the workspace (yours and others) — question, mode, status, and the report documentId when done.',
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/research')),
)

server.registerTool(
  'research_status',
  {
    description:
      "Check a research run: status (queued/running/done/error), current phase, and — when done — the report's documentId (read it with get_document) plus the source list.",
    inputSchema: { runId: z.string().describe('Run id (from research)') },
  },
  async ({ runId }) => {
    const r = (await api('GET', `/api/research/${encodeURIComponent(runId)}`)) as {
      run?: { status?: string; phase?: string | null; artifactId?: string | null; error?: string | null }
      sources?: unknown[]
    }
    return ok({
      status: r.run?.status,
      phase: r.run?.phase,
      documentId: r.run?.artifactId,
      error: r.run?.error,
      sources: r.sources?.length ?? 0,
    })
  },
)

// ── Board governance (personal assistants only) ─────────────────────────────
// These act AS your owner (identity proxy). They only work if you're someone's
// personal assistant AND your owner has the required board role — general
// agents get 401/403. Talaria enforces both server-side.
server.registerTool(
  'list_teams',
  {
    description:
      "List the teams your owner belongs to (personal assistants only). Use the names with move_board_to_team.",
    inputSchema: {},
  },
  async () => ok(await api('GET', '/api/teams')),
)

server.registerTool(
  'move_board_to_team',
  {
    description:
      "Move a board into a team, or back to Personal (personal assistants only, and only for boards your owner OWNS — it changes who can see the board). Team is matched by name; use 'personal' to remove it from any team.",
    inputSchema: {
      boardId: z.string().describe('Board id (from list_boards)'),
      teamName: z.string().max(120).describe("Team name (see list_teams), or 'personal' for no team"),
    },
  },
  async ({ boardId, teamName }) => ok(await api('PATCH', `/api/boards/${encodeURIComponent(boardId)}`, { teamName })),
)

server.registerTool(
  'list_board_members',
  {
    description: "Who's on a board (people + roles). Any agent allowed on the board may read this.",
    inputSchema: { boardId: z.string().describe('Board id (from list_boards)') },
  },
  async ({ boardId }) => ok(await api('GET', `/api/boards/${encodeURIComponent(boardId)}/members`)),
)

server.registerTool(
  'add_board_member',
  {
    description:
      "Share a board with a person by email (personal assistants only; your owner needs owner/editor on the board). Role 'editor' can change the board, 'viewer' is read-only.",
    inputSchema: {
      boardId: z.string().describe('Board id (from list_boards)'),
      email: z.string().describe("The person's email"),
      role: z.enum(['editor', 'viewer']).optional().describe("Access level (default 'editor')"),
    },
  },
  async ({ boardId, email, role }) => ok(await api('POST', `/api/boards/${encodeURIComponent(boardId)}/members`, { email, role })),
)

server.registerTool(
  'remove_board_member',
  {
    description:
      "Remove a person from a board by email (personal assistants only; your owner needs owner/editor on the board). The board owner can't be removed.",
    inputSchema: {
      boardId: z.string().describe('Board id (from list_boards)'),
      email: z.string().describe("The person's email"),
    },
  },
  async ({ boardId, email }) => ok(await api('DELETE', `/api/boards/${encodeURIComponent(boardId)}/members`, { email })),
)

server.registerTool(
  'set_board_agents',
  {
    description:
      "Add or remove fleet agents on a board (personal assistants only; your owner needs owner/editor). Pass agent model names (e.g. 'dex-developer'). Returns the board's resulting agent policy.",
    inputSchema: {
      boardId: z.string().describe('Board id (from list_boards)'),
      add: z.array(z.string().max(200)).max(100).optional().describe('Agent models to allow on the board'),
      remove: z.array(z.string().max(200)).max(100).optional().describe('Agent models to remove from the board'),
    },
  },
  async ({ boardId, add, remove }) => ok(await api('PUT', `/api/boards/${encodeURIComponent(boardId)}/agents`, { add, remove })),
)

  return server
}

if (HTTP_PORT) {
  // Fleet mode: one HTTP endpoint every containerized agent connects to
  // (config-injected url + headers). Stateless streamable HTTP: each POST is
  // handled by a fresh server bound to the calling agent's identity. Auth is
  // PASS-THROUGH: this process cannot validate a per-agent credential (no DB),
  // so it forwards whatever the caller presented and lets Talaria decide.
  //
  // Pass-through is right for tool CALLS — every one of them ends in a request
  // Talaria authenticates. It is not the whole story for `initialize` and
  // `tools/list`, which the MCP SDK answers from this process and which
  // therefore never reach Talaria at all: without the two guards below, the
  // complete toolkit catalog (every tool name, description and JSON schema —
  // a map of the fleet's write surface) is readable by anyone who can open a
  // socket to this port. Both guards are applied, because each covers what the
  // other cannot:
  //   · BIND — loopback plus the docker bridges the fleet reaches us on,
  //     instead of 0.0.0.0. Takes the port off the LAN entirely.
  //   · VERIFY — the presented credential is checked against Talaria before a
  //     server is built for it, so a container on the bridge still cannot
  //     enumerate the catalog without a credential Talaria accepts.
  const { StreamableHTTPServerTransport } = await import('@modelcontextprotocol/sdk/server/streamableHttp.js')
  const { createServer } = await import('node:http')
  const { networkInterfaces } = await import('node:os')
  const { createHash, timingSafeEqual } = await import('node:crypto')

  /** Where to listen. `0.0.0.0` published the catalog on every interface the
   *  host has; the fleet only ever arrives over the docker bridge (the
   *  chassis' `extra_hosts: host.docker.internal:host-gateway` resolves to a
   *  bridge address) and Talaria's own service calls arrive over loopback.
   *  Bind exactly those. MCP_HTTP_HOST overrides with an explicit
   *  comma-separated list for topologies this can't infer — a published
   *  container port needs `0.0.0.0` and the network policy around it. */
  const bindHosts = (): string[] => {
    const override = (process.env.MCP_HTTP_HOST ?? '').trim()
    if (override) return [...new Set(override.split(',').map((h) => h.trim()).filter(Boolean))]
    const hosts = new Set(['127.0.0.1'])
    for (const [iface, addrs] of Object.entries(networkInterfaces())) {
      if (!/^(docker|br-|podman|cni-)/.test(iface)) continue
      for (const a of addrs ?? []) if (a.family === 'IPv4' && !a.internal) hosts.add(a.address)
    }
    return [...hosts]
  }

  const sameSecret = (a: string, b: string): boolean => {
    const ab = Buffer.from(a)
    const bb = Buffer.from(b)
    return ab.length === bb.length && timingSafeEqual(ab, bb)
  }

  /** Latched so a broken probe logs once per outage, not once per request. */
  let probeBroken = false

  // Verified callers, so a chatty client doesn't pay a round trip per request.
  // Keyed by (name, hash of credential) — never the credential itself.
  const VERIFY_TTL_MS = 60_000
  const REFUSED_TTL_MS = 5_000
  const seen = new Map<string, { ok: boolean; status: number; at: number }>()

  /** The Talaria route this process authenticates against — see the coupling
   *  warning on `verify`. The env var is the escape hatch: it repoints the probe
   *  without a release, which is what makes narrowing `/api/users` a survivable
   *  change instead of a fleet-wide outage. */
  const VERIFY_PATH = (process.env.TALARIA_MCP_VERIFY_PATH ?? '/api/users').trim()

  /** Is this credential one Talaria accepts, for this name? This process holds
   *  no DB and cannot answer that itself, so it asks the only thing that can —
   *  a cheap authenticated GET, which exercises exactly the resolution
   *  (`agent-auth`) every tool call would hit anyway. Four outcomes:
   *    ok            serve the request
   *    401/403       refuse with the same code — the credential is not one
   *                  Talaria knows, or contradicts the claimed name
   *    404/405       the probe route is gone, renamed, or no longer answers an
   *                  agent GET. NOT a verdict about the caller: 503, uncached,
   *                  and shouted about (see below)
   *    unreachable   503, NOT cached, and never waved through: an unverifiable
   *                  caller is not a verified one, and every tool this process
   *                  exposes is broken anyway while Talaria is down.
   *
   *  ⚠ LOAD-BEARING COUPLING. Using a product route as the probe makes it an
   *  AUTHENTICATION ORACLE for the entire toolkit: narrow `/api/users` to
   *  admins or to session auth and every agent's `initialize`/`tools/list`
   *  starts 401ing, which takes the fleet toolkit dark with no error that
   *  points at the cause. `/api/users` was chosen because its agent branch is
   *  nothing but `agentCaller()` — the same resolution a tool call hits — but
   *  the contract it has to keep is narrow: agent-credential auth, GET, cheap,
   *  401/403 (not 404) on a bad credential. That contract is written down in
   *  mcp/README.md ("Authentication") and mirrored at the Talaria end in
   *  `ui/src/server/agent-auth.ts`, so neither side can be changed without
   *  reading it. Repoint with TALARIA_MCP_VERIFY_PATH rather than editing this. */
  const verify = async (agent: string, key: string): Promise<{ ok: boolean; status: number }> => {
    // Talaria's own service-to-service calls (the MCP gateway's catalog
    // refresh, the admin connection test) present the org-wide fleet key this
    // process was started with. Recognizing it here costs no round trip and
    // grants nothing beyond the catalog — it is still a secret, and every tool
    // call made with it is authenticated by Talaria on arrival.
    if (KEY && sameSecret(key, KEY)) return { ok: true, status: 200 }
    const id = `${agent}\u0000${createHash('sha256').update(key).digest('hex')}`
    const hit = seen.get(id)
    if (hit && Date.now() - hit.at < (hit.ok ? VERIFY_TTL_MS : REFUSED_TTL_MS)) return { ok: hit.ok, status: hit.status }
    const res = await fetch(`${BASE}${VERIFY_PATH}`, {
      method: 'GET',
      headers: { 'x-api-key': key, 'x-agent-name': agent },
      signal: AbortSignal.timeout(8_000),
    }).catch(() => null)
    if (!res) return { ok: false, status: 503 }
    // The probe route itself is gone/narrowed. Say so LOUDLY and name the fix:
    // the symptom an operator sees is "the whole toolkit stopped working", and
    // nothing else in either process would connect that to a route change.
    if (res.status === 404 || res.status === 405) {
      if (!probeBroken) {
        probeBroken = true
        console.error(
          `talaria-mcp: verification probe GET ${BASE}${VERIFY_PATH} returned ${res.status} — the toolkit cannot authenticate ANY agent and is dark fleet-wide. That route is this process's authentication oracle; if it moved or was narrowed, set TALARIA_MCP_VERIFY_PATH to an agent-authenticated GET (see mcp/README.md "Authentication").`,
        )
      }
      return { ok: false, status: 503 }
    }
    if (!res.ok && res.status !== 401 && res.status !== 403) return { ok: false, status: 503 } // Talaria unwell, not the caller
    probeBroken = false
    const verdict = { ok: res.ok, status: res.ok ? 200 : res.status, at: Date.now() }
    seen.set(id, verdict)
    if (seen.size > 500) for (const [k, v] of seen) if (Date.now() - v.at > VERIFY_TTL_MS) seen.delete(k)
    return { ok: verdict.ok, status: verdict.status }
  }

  const readBody = (req: import('node:http').IncomingMessage): Promise<unknown> =>
    new Promise((resolve, reject) => {
      const chunks: Buffer[] = []
      req.on('data', (c: Buffer) => chunks.push(c))
      req.on('end', () => {
        try {
          resolve(chunks.length ? JSON.parse(Buffer.concat(chunks).toString('utf8')) : undefined)
        } catch (e) {
          reject(e as Error)
        }
      })
      req.on('error', reject)
    })

  const handle = (req: import('node:http').IncomingMessage, res: import('node:http').ServerResponse): void => {
    void (async () => {
      if (!req.url?.startsWith('/mcp') || req.method !== 'POST') {
        res.writeHead(req.method === 'GET' || req.method === 'DELETE' ? 405 : 404).end()
        return
      }
      const auth = String(req.headers.authorization ?? '')
      const callerKey = String(req.headers['x-api-key'] ?? '') || (auth.startsWith('Bearer ') ? auth.slice(7) : '')
      if (!callerKey) {
        res.writeHead(401, { 'content-type': 'application/json' }).end(JSON.stringify({ error: 'agent credential required' }))
        return
      }
      const agent = String(req.headers['x-agent-name'] ?? '')
      if (!agent) {
        res.writeHead(400, { 'content-type': 'application/json' }).end(JSON.stringify({ error: 'x-agent-name required' }))
        return
      }
      // BEFORE a server is built for this caller — building one is what makes
      // the catalog answerable, and `initialize`/`tools/list` never reach
      // Talaria to be authenticated on the way.
      const check = await verify(agent, callerKey)
      if (!check.ok) {
        res.writeHead(check.status, { 'content-type': 'application/json' }).end(
          JSON.stringify({
            error:
              check.status === 503
                ? 'cannot reach Talaria to authenticate this credential'
                : `Talaria rejected this credential for "${agent}" (${check.status})`,
          }),
        )
        return
      }
      const body = await readBody(req)
      const server = buildServer(agent, callerKey)
      const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined })
      res.on('close', () => {
        void transport.close()
        void server.close()
      })
      await server.connect(transport)
      await transport.handleRequest(req, res, body)
    })().catch((e) => {
      try {
        res.writeHead(500, { 'content-type': 'application/json' }).end(JSON.stringify({ error: String(e) }))
      } catch {
        /* headers already sent */
      }
    })
  }

  // One listener per address rather than one wildcard listener — and RESCANNED,
  // because the address set is not static: docker brings a compose network's
  // bridge device up when the first container attaches and down when the last
  // one leaves, and `os.networkInterfaces()` only reports interfaces that are
  // UP. A one-shot scan would therefore miss the fleet's own bridge whenever
  // this process starts before the fleet does. Re-scan on a slow interval,
  // bind what is new, and let a failed bind be retried by the next pass; the
  // interval stays REFERENCED so a process that has bound nothing yet keeps
  // trying (and keeps saying so) instead of exiting into silence.
  const listening = new Set<string>()
  const bind = (host: string): void => {
    if (listening.has(host)) return
    listening.add(host)
    const srv = createServer(handle)
    srv.on('error', (e: Error) => {
      listening.delete(host)
      console.error(`talaria-mcp: could not bind ${host}:${HTTP_PORT} — ${e.message}`)
    })
    srv.listen(HTTP_PORT, host, () => console.error(`talaria-mcp: fleet HTTP on ${host}:${HTTP_PORT} → ${BASE}`))
  }
  const scan = (): void => {
    const hosts = bindHosts()
    if (!hosts.length) console.error('talaria-mcp: no bindable address found — set MCP_HTTP_HOST')
    for (const host of hosts) bind(host)
  }
  scan()
  setInterval(scan, 30_000)
} else {
  // stdio serves ONE agent, so this process holds that agent's credential.
  // (HTTP mode holds none — every request brings its own.)
  if (!KEY) {
    console.error('talaria-mcp: TALARIA_AGENT_KEY is required in stdio mode')
    process.exit(1)
  }
  if (!AGENT) {
    console.error('talaria-mcp: TALARIA_AGENT_NAME is required in stdio mode')
    process.exit(1)
  }
  const server = buildServer(AGENT, KEY)
  await server.connect(new StdioServerTransport())
  console.error(`talaria-mcp: connected (agent "${AGENT}" → ${BASE})`)
}
