#!/usr/bin/env node
// talaria-mcp — the agent-facing MCP server for Talaria's PM suite.
//
// Exposes ONLY the safe tools: agents can discover boards, read and create
// tickets, triage, comment, report outcomes, log time, and link dependencies.
// There is no assign tool and no complete tool — the guardrails hold by
// construction here, and the Talaria API enforces them again server-side
// (assign → 403, done → quality_review).
//
// Env:
//   TALARIA_URL        base URL of the Talaria app (default http://localhost:5273)
//   TALARIA_AGENT_KEY  the fleet agent key (required)
//   TALARIA_AGENT_NAME this agent's fleet model name (required; scopes board
//                      policy and attributes all activity)

import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import { z } from 'zod'

const BASE = (process.env.TALARIA_URL ?? 'http://localhost:5273').replace(/\/+$/, '')
const KEY = process.env.TALARIA_AGENT_KEY ?? ''
const AGENT = process.env.TALARIA_AGENT_NAME ?? ''
/** When set, serve MCP over streamable HTTP for the WHOLE fleet (per-request
 *  agent identity via X-Agent-Name) instead of stdio for one agent. */
const HTTP_PORT = Number(process.env.MCP_HTTP_PORT ?? 0)

if (!KEY) {
  console.error('talaria-mcp: TALARIA_AGENT_KEY is required')
  process.exit(1)
}

function makeApi(agent: string) {
  return async function api(method: string, path: string, body?: unknown): Promise<unknown> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      'x-api-key': KEY,
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
// HTTP mode builds per request (stateless) for the connecting agent.
function buildServer(agent: string): McpServer {
  const api = makeApi(agent)
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
    description: "List a board's tickets.",
    inputSchema: { boardId: z.string().describe('Board id (from list_boards)') },
  },
  async ({ boardId }) => ok(await api('GET', `/api/boards/${encodeURIComponent(boardId)}/tasks`)),
)

server.registerTool(
  'get_ticket',
  {
    description: 'Get a ticket in full: fields, comments, activity, watchers, reviews, dependencies.',
    inputSchema: { taskId: z.string().describe('Ticket id') },
  },
  async ({ taskId }) => ok(await api('GET', `/api/tasks/${encodeURIComponent(taskId)}`)),
)

server.registerTool(
  'create_ticket',
  {
    description:
      'Create a ticket on a board. It lands in the inbox for a human to assign — agents cannot assign work.',
    inputSchema: {
      boardId: z.string().describe('Board id (from list_boards)'),
      title: z.string().min(1).max(300),
      description: z.string().max(20_000).optional().describe('Markdown description'),
      priority: z.enum(PRIORITIES).optional(),
      effort: z.enum(EFFORTS).optional().describe('T-shirt size estimate'),
    },
  },
  async ({ boardId, ...body }) => ok(await api('POST', `/api/boards/${encodeURIComponent(boardId)}/tasks`, body)),
)

server.registerTool(
  'triage_ticket',
  {
    description:
      'Triage a ticket: adjust priority, effort, labels, description, due date, or move it between in_progress / blocked / quality_review. Assigning and completing stay human-only.',
    inputSchema: {
      taskId: z.string(),
      title: z.string().min(1).max(300).optional(),
      description: z.string().max(20_000).optional(),
      priority: z.enum(PRIORITIES).optional(),
      effort: z.enum(EFFORTS).optional(),
      tags: z.array(z.string().max(40)).max(20).optional().describe('Labels (replaces the set)'),
      dueDate: z.string().datetime().optional(),
      status: z.enum(AGENT_STATUSES).optional(),
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
      'Report the outcome of work on a ticket and hand it to quality review. A human signs off on done.',
    inputSchema: {
      taskId: z.string(),
      outcome: z.string().max(50_000).describe('What was accomplished'),
      resolution: z.string().max(50_000).optional().describe('How it was resolved'),
      errorMessage: z.string().max(50_000).optional().describe('If the work failed, what went wrong'),
    },
  },
  async ({ taskId, ...body }) => ok(await api('PUT', `/api/tasks/${encodeURIComponent(taskId)}`, { ...body, status: 'quality_review' })),
)

server.registerTool(
  'add_time',
  {
    description: "Add time spent to a ticket's auto-accumulated total.",
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
      visibility: z.enum(['private', 'org', 'public']).optional().describe("Who can see it (default 'org' — the workspace)"),
    },
  },
  async ({ title, markdown, visibility }) => ok(await api('POST', '/api/artifacts', { kind: 'doc', title, body: markdown, visibility })),
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
      "Report the LLM tokens you burned working a ticket. Feeds the ticket's cost rollup and the fleet ledger.",
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
    description: 'Mark a ticket as blocked by another ticket on the same board.',
    inputSchema: {
      taskId: z.string().describe('The blocked ticket'),
      dependsOnId: z.string().describe('The ticket it depends on'),
    },
  },
  async ({ taskId, dependsOnId }) => ok(await api('POST', `/api/tasks/${encodeURIComponent(taskId)}/dependencies`, { dependsOnId })),
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
  // handled by a fresh server bound to the calling agent's identity. Auth:
  // the fleet key must ride along — the port is host-bound for containers.
  const { StreamableHTTPServerTransport } = await import('@modelcontextprotocol/sdk/server/streamableHttp.js')
  const { createServer } = await import('node:http')
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

  createServer((req, res) => {
    void (async () => {
      if (!req.url?.startsWith('/mcp') || req.method !== 'POST') {
        res.writeHead(req.method === 'GET' || req.method === 'DELETE' ? 405 : 404).end()
        return
      }
      if (req.headers['x-api-key'] !== KEY) {
        res.writeHead(401, { 'content-type': 'application/json' }).end(JSON.stringify({ error: 'bad key' }))
        return
      }
      const agent = String(req.headers['x-agent-name'] ?? '')
      if (!agent) {
        res.writeHead(400, { 'content-type': 'application/json' }).end(JSON.stringify({ error: 'x-agent-name required' }))
        return
      }
      const body = await readBody(req)
      const server = buildServer(agent)
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
  }).listen(HTTP_PORT, '0.0.0.0', () => console.error(`talaria-mcp: fleet HTTP on :${HTTP_PORT} → ${BASE}`))
} else {
  if (!AGENT) {
    console.error('talaria-mcp: TALARIA_AGENT_NAME is required in stdio mode')
    process.exit(1)
  }
  const server = buildServer(AGENT)
  await server.connect(new StdioServerTransport())
  console.error(`talaria-mcp: connected (agent "${AGENT}" → ${BASE})`)
}
