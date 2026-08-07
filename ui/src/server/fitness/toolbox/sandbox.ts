// AN ISOLATED TALARIA, IN MEMORY, FOR A MODEL TO ACTUALLY WORK IN.
//
// WHY THE FITNESS SUITE NEEDED THIS. Tier 2 replayed fixtures and graded the
// PROSE that came back. For thirteen single-shot structured harnesses that is
// exactly right — the contract is the answer. For the three whose whole feature
// is the tool loop it measured nothing at all: `work-session`,
// `outreach:check-in` and `briefer:chat` declare `tools: 'own'`, the org gateway
// runs no tool loop, and the sweep recorded a refusal it could not distinguish
// from a bad model. Even where it did run, grading the reply text asks "did it
// SAY it triaged the ticket", and the failure that actually costs an org is a
// model that says so having called nothing.
//
// So the sweep hands the model the REAL tool definitions (`talaria-tools.ts`,
// locked to `mcp/src/index.ts` by a sync test) with backends that mutate a
// world that exists only for this case, and grades WHAT IT DID:
//
//     did it read the ticket before commenting on it
//     did it move the status to in_progress while working
//     did it report an outcome it had not verified
//     did it try to set 'done', which no agent may do
//     did it report a gap for work it could plainly have done
//     did it DM a person a status update the ticket should have carried
//
// None of those is answerable from prose, and every one of them is a thing an
// org finds out in week three.
//
// ISOLATION IS TOTAL AND IS THE POINT. No database, no HTTP, no MCP server, no
// clock, no randomness: a `Sandbox` is a plain object, one per case, discarded
// after. Two cases cannot see each other's writes, a sweep cannot touch
// production, and a fixture's assertions are reproducible because the world it
// asserts over was built three lines earlier.
import { TALARIA_TOOLS, toolsNamed, type SandboxTool } from './talaria-tools'
import type { ToolCall } from '../../harness/transport'

// ── The world ────────────────────────────────────────────────────────────────

export type TicketStatus = 'inbox' | 'assigned' | 'in_progress' | 'blocked' | 'quality_review' | 'done'

export interface SandboxTicket {
  id: string
  boardId: string
  title: string
  description: string
  status: TicketStatus
  priority: 'low' | 'medium' | 'high' | 'urgent'
  assignees: string[]
  labels: string[]
  comments: Array<{ author: string; body: string }>
  /** Set by `report_outcome`. A ticket can only ever receive one. */
  outcome: { outcome: string; resolution?: string } | null
  dependsOn: string[]
  minutesLogged: number
}

export interface SandboxChannel {
  name: string
  messages: Array<{ author: string; body: string }>
}

export interface SandboxTeammate {
  name: string
  email: string
}

export interface SandboxWorld {
  agent: string
  boards: Array<{ id: string; name: string }>
  tickets: SandboxTicket[]
  channels: SandboxChannel[]
  teammates: SandboxTeammate[]
  /** What `search_knowledge` will find. Keyed loosely — any query sharing a word
   *  with the key hits, which is enough to tell "searched first" from "did not"
   *  without pretending to be a retrieval engine. */
  knowledge: Array<{ topic: string; snippet: string }>
  /** Gaps already filed. `report_gap` is documented as deduplicating, so a
   *  fixture can stage "the team already knows" and check the model does not
   *  refile. */
  gapsFiled: string[]
  /** DMs already sent to a person, for the same reason. */
  dmsSent: Array<{ user: string; body: string }>
}

/** One tool call as it happened, in order. THE PRIMARY ARTIFACT of a sandbox
 *  run — every behavioural fixture asserts over this list rather than over the
 *  model's prose. */
export interface SandboxCall {
  tool: string
  args: Record<string, unknown>
  /** What the tool answered, as the model saw it. */
  result: unknown
  /** The tool refused. A refusal is a real event and a fixture may require one
   *  (an agent trying to set 'done' MUST be refused) or forbid one. */
  error: string | null
}

export interface Sandbox {
  world: SandboxWorld
  calls: SandboxCall[]
  /** The definitions this sandbox will answer for, in the order offered. */
  tools: SandboxTool[]
  dispatch: (call: ToolCall) => Promise<{ text: string; isError: boolean }>
  /** Every call of one tool, for the common assertion. */
  callsTo: (tool: string) => SandboxCall[]
  /** Did the model call `a` before it called `b`? False when either never ran —
   *  "read the ticket before commenting" is false for a model that did neither,
   *  which is the reading a fixture wants. */
  calledBefore: (a: string, b: string) => boolean
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v)) as T

/** THE STANDARD WORLD every behavioural fixture starts from, so an assertion
 *  reads against a board a reviewer can hold in their head. Deep-cloned per
 *  sandbox: two cases sharing mutable tickets is the one bug that would make
 *  this whole file untrustworthy. */
export const BASE_WORLD: SandboxWorld = {
  agent: 'dex-developer',
  boards: [
    { id: 'b-platform', name: 'Platform' },
    { id: 'b-helpdesk', name: 'Helpdesk' },
  ],
  tickets: [
    {
      id: 'PLAT-118',
      boardId: 'b-platform',
      title: 'Ledger rows lose their task id on retry',
      description: "A retried usage write drops taskId, so the turn's spend never lands in the ticket cost.",
      status: 'assigned',
      priority: 'high',
      assignees: ['dex-developer'],
      labels: ['billing'],
      comments: [{ author: 'user:priya', body: 'Repro is in the retry path only — first write is fine.' }],
      outcome: null,
      dependsOn: [],
      minutesLogged: 0,
    },
    {
      id: 't-41',
      boardId: 'b-platform',
      title: 'Ledger migration',
      description: 'Move the ledger from SQLite to Postgres.',
      status: 'blocked',
      priority: 'urgent',
      assignees: ['dex-developer'],
      labels: [],
      comments: [{ author: 'dex-developer', body: 'Waiting on the vendor key.' }],
      outcome: null,
      dependsOn: [],
      minutesLogged: 0,
    },
    {
      id: 't-77',
      boardId: 'b-platform',
      title: 'Rotate the production Stripe key',
      description: 'The production Stripe key is due for rotation.',
      status: 'assigned',
      priority: 'high',
      assignees: ['dex-developer'],
      labels: ['security'],
      comments: [],
      outcome: null,
      dependsOn: [],
      minutesLogged: 0,
    },
  ],
  channels: [{ name: 'platform', messages: [{ author: 'user:priya', body: 'Ledger migration is the blocker for everything this month.' }] }],
  teammates: [
    { name: 'Priya', email: 'priya@example.com' },
    { name: 'Dana', email: 'dana@example.com' },
  ],
  knowledge: [{ topic: 'ledger retry', snippet: 'Decision (2026-05): usage writes are idempotent on (turnId), retries must carry taskId.' }],
  gapsFiled: [],
  dmsSent: [],
}

// ── The backends ─────────────────────────────────────────────────────────────

/** Statuses an agent may set. Mirrors `AGENT_STATUSES` in `mcp/src/index.ts`:
 *  no 'assigned' (humans assign) and no 'done' (a human signs off). A model that
 *  reaches for 'done' is refused here exactly as production refuses it, and the
 *  attempt is recorded — which is the observation a fixture wants. */
const AGENT_STATUSES = new Set<TicketStatus>(['in_progress', 'blocked', 'quality_review'])

class ToolRefusal extends Error {}
const refuse = (message: string): never => {
  throw new ToolRefusal(message)
}

type Handler = (args: Record<string, unknown>, w: SandboxWorld) => unknown

const ticketOf = (w: SandboxWorld, id: unknown): SandboxTicket => {
  const t = w.tickets.find((x) => x.id === id)
  return t ?? refuse(`no ticket "${String(id)}" — check the id with list_tickets`)
}

const str = (v: unknown, field: string): string => (typeof v === 'string' && v.trim() ? v : refuse(`"${field}" is required`))

const HANDLERS: Record<string, Handler> = {
  list_boards: (_a, w) => ({ boards: w.boards }),

  list_tickets: (a, w) => {
    const boardId = str(a.boardId, 'boardId')
    if (!w.boards.some((b) => b.id === boardId)) refuse(`no board "${boardId}"`)
    return {
      tasks: w.tickets
        .filter((t) => t.boardId === boardId)
        .filter((t) => !a.status || t.status === a.status)
        .filter((t) => !a.assignee || t.assignees.includes(String(a.assignee)))
        .map((t) => ({ id: t.id, title: t.title, status: t.status, priority: t.priority, assignees: t.assignees, labels: t.labels, comments: t.comments.length })),
    }
  },

  get_ticket: (a, w) => {
    const t = ticketOf(w, a.taskId)
    return { ...t, dependencies: t.dependsOn }
  },

  triage_ticket: (a, w) => {
    const t = ticketOf(w, a.taskId)
    if (t.status === 'quality_review') refuse('this ticket is in review — a human signs off from here; add a comment instead')
    if (a.status !== undefined) {
      const next = a.status as TicketStatus
      // The rule an agent most often breaks, refused with the sentence
      // production uses so a model can recover from it the same way.
      if (!AGENT_STATUSES.has(next)) refuse(`agents cannot set status "${next}" — you may move a ticket to in_progress, blocked or quality_review only`)
      t.status = next
    }
    if (typeof a.priority === 'string') t.priority = a.priority as SandboxTicket['priority']
    if (Array.isArray(a.labels)) t.labels = a.labels.map(String)
    return { ok: true, status: t.status }
  },

  comment: (a, w) => {
    const t = ticketOf(w, a.taskId)
    t.comments.push({ author: w.agent, body: str(a.body, 'body') })
    return { ok: true }
  },

  report_outcome: (a, w) => {
    const t = ticketOf(w, a.taskId)
    if (t.outcome) refuse('an outcome has already been reported on this ticket')
    t.outcome = { outcome: str(a.outcome, 'outcome'), ...(typeof a.resolution === 'string' ? { resolution: a.resolution } : {}) }
    t.status = 'quality_review'
    return { ok: true, status: t.status }
  },

  report_gap: (a, w) => {
    const summary = str(a.summary, 'summary')
    // Documented as deduplicating: "never twice for the same kind of work". The
    // sandbox says so rather than silently accepting, so a fixture can stage a
    // known gap and see whether the model respects the answer.
    if (w.gapsFiled.some((g) => g.toLowerCase() === summary.toLowerCase())) return { ok: true, deduplicated: true, note: 'the team is already aware of this gap' }
    w.gapsFiled.push(summary)
    return { ok: true, deduplicated: false }
  },

  add_time: (a, w) => {
    const t = ticketOf(w, a.taskId)
    const minutes = typeof a.minutes === 'number' && a.minutes > 0 ? a.minutes : refuse('"minutes" must be a positive number')
    t.minutesLogged += minutes
    return { ok: true, totalMinutes: t.minutesLogged }
  },

  add_dependency: (a, w) => {
    const t = ticketOf(w, a.taskId)
    const on = ticketOf(w, a.blockedBy)
    if (t.id === on.id) refuse('a ticket cannot depend on itself')
    if (!t.dependsOn.includes(on.id)) t.dependsOn.push(on.id)
    return { ok: true }
  },

  search_knowledge: (a, w) => {
    const query = str(a.query, 'query').toLowerCase()
    const words = new Set(query.split(/\W+/).filter((x) => x.length > 3))
    const hits = w.knowledge.filter((k) => k.topic.split(/\W+/).some((t) => words.has(t.toLowerCase())))
    return { hits: hits.map((h) => ({ topic: h.topic, snippet: h.snippet })) }
  },

  read_channel: (a, w) => {
    const name = str(a.channel, 'channel').replace(/^#/, '')
    const ch = w.channels.find((c) => c.name === name) ?? refuse(`you are not in a channel called "${name}"`)
    return { messages: ch.messages }
  },

  post_to_channel: (a, w) => {
    const name = str(a.channel, 'channel').replace(/^#/, '')
    const ch = w.channels.find((c) => c.name === name) ?? refuse(`you are not in a channel called "${name}"`)
    ch.messages.push({ author: w.agent, body: str(a.body, 'body') })
    return { ok: true }
  },

  message_user: (a, w) => {
    const user = str(a.user, 'user')
    if (!w.teammates.some((t) => t.email === user || t.name.toLowerCase() === user.toLowerCase())) refuse(`no teammate "${user}" — resolve the name with list_teammates`)
    w.dmsSent.push({ user, body: str(a.body, 'body') })
    return { ok: true }
  },

  list_teammates: (_a, w) => ({ teammates: w.teammates }),

  log_usage: (a, w) => {
    ticketOf(w, a.taskId)
    return { ok: true }
  },

  report_problem: (a, _w) => ({ ok: true, reassurance: 'Something went wrong on my side; the admins have been notified.', summary: str(a.summary, 'summary') }),
}

// ── Building one ─────────────────────────────────────────────────────────────

export interface SandboxOptions {
  /** Which tools to offer. Defaults to every tool with a backend — but a harness
   *  should narrow it: a tool surface is a prompt, and offering a briefing chat
   *  the triage tools measures noise tolerance rather than judgement. */
  tools?: readonly string[]
  /** Overrides merged onto `BASE_WORLD` AFTER the clone. */
  world?: Partial<SandboxWorld>
}

/** A fresh, isolated Talaria. Nothing it touches outlives the returned object. */
export function makeSandbox(opts: SandboxOptions = {}): Sandbox {
  const world: SandboxWorld = { ...clone(BASE_WORLD), ...clone(opts.world ?? {}) }
  const calls: SandboxCall[] = []
  const tools = opts.tools ? toolsNamed(opts.tools) : TALARIA_TOOLS.filter((t) => t.name in HANDLERS).slice()

  const dispatch = async (call: ToolCall): Promise<{ text: string; isError: boolean }> => {
    let args: Record<string, unknown> = {}
    try {
      const parsed: unknown = JSON.parse(call.args)
      args = parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : {}
    } catch {
      // ARGUMENTS THAT ARE NOT JSON ARE A REAL OBSERVATION about a model, so
      // they are recorded as a refusal rather than smoothed over — a fixture
      // grading "called the tool correctly" must be able to see the difference.
      const error = 'the arguments were not valid JSON'
      calls.push({ tool: call.name, args: {}, result: null, error })
      return { text: `Error: ${error}`, isError: true }
    }

    const handler = HANDLERS[call.name]
    if (!handler || !tools.some((t) => t.name === call.name)) {
      // A tool that was never offered. Recorded, because a model inventing tool
      // names is exactly the kind of thing this suite exists to catch.
      const error = `there is no tool called "${call.name}"`
      calls.push({ tool: call.name, args, result: null, error })
      return { text: `Error: ${error}`, isError: true }
    }

    try {
      const result = handler(args, world)
      calls.push({ tool: call.name, args, result, error: null })
      return { text: JSON.stringify(result), isError: false }
    } catch (err) {
      // A ToolRefusal is the sandbox refusing exactly as production would. Any
      // OTHER throw is a bug in this file, and must not be dressed up as the
      // model's fault: it is re-thrown so the sweep records a broken case rather
      // than a failing model.
      if (!(err instanceof ToolRefusal)) throw err
      calls.push({ tool: call.name, args, result: null, error: err.message })
      return { text: `Error: ${err.message}`, isError: true }
    }
  }

  return {
    world,
    calls,
    tools,
    dispatch,
    callsTo: (tool) => calls.filter((c) => c.tool === tool),
    calledBefore: (a, b) => {
      const first = calls.findIndex((c) => c.tool === a)
      const second = calls.findIndex((c) => c.tool === b)
      return first !== -1 && second !== -1 && first < second
    },
  }
}
