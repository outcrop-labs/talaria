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
