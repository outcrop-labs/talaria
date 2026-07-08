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

if (!KEY || !AGENT) {
  console.error('talaria-mcp: TALARIA_AGENT_KEY and TALARIA_AGENT_NAME are required')
  process.exit(1)
}

async function api(method: string, path: string, body?: unknown): Promise<unknown> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      'x-api-key': KEY,
      'x-agent-name': AGENT,
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

const ok = (data: unknown) => ({ content: [{ type: 'text' as const, text: JSON.stringify(data, null, 2) }] })

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

const transport = new StdioServerTransport()
await server.connect(transport)
console.error(`talaria-mcp: connected (agent "${AGENT}" → ${BASE})`)
