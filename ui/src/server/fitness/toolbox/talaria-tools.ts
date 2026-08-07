// THE TALARIA TOOLKIT, AS A MODEL SEES IT — for the fitness suite to hand over.
//
// WHY A COPY AND NOT AN IMPORT. The real registrations live in `mcp/src/index.ts`,
// a separate package whose module body STARTS AN MCP SERVER and whose every
// handler calls Talaria's HTTP API. Importing it from the fitness suite would
// boot a server and reach for a network on `import`. So the definitions are
// copied here — and `talaria-tools.sync.test.ts` reads `mcp/src/index.ts` and
// fails if a name or a description has drifted, which is what keeps "a copy of
// our base tools" true six months from now rather than merely true today.
//
// WHY THE DESCRIPTIONS MATTER MORE THAN THE SCHEMAS. This is a probe of whether
// a model can WORK here, and the toolkit's description text is most of the
// instruction it gets: "you cannot move it back", "never twice for the same kind
// of work", "not for status updates". A sandbox with tidied-up descriptions
// would measure a model against a toolkit that does not exist and flatter every
// candidate — the descriptions are the hard part, so they are the part that is
// copied exactly.
//
// WHAT IS DELIBERATELY NOT HERE: the forty-odd tools the harnesses under test
// never touch. A tool surface is a prompt, and padding it with thirty
// irrelevant tools would measure a model's tolerance for noise rather than its
// ability to do the job. `hermes-tools.ts` carries the base agent surface for
// the same reason, kept separate because it is a different vendor's contract.
import type { ToolDefinition } from '../../harness/transport'

/** `${...}` in a real description interpolates a shared clause (LIVE_ONLY,
 *  COMMENT_EXEMPTION). The sandbox spells those out; the sync test therefore
 *  compares the STATIC PREFIX — everything up to the first interpolation — which
 *  is the part a copy can be held to exactly. */
export interface SandboxTool extends ToolDefinition {
  /** The tool this stands in for, as registered in `mcp/src/index.ts`. */
  name: string
}

const str = (description: string) => ({ type: 'string', description })

export const TALARIA_TOOLS: readonly SandboxTool[] = [
  {
    name: 'list_boards',
    description:
      'List the boards this agent is allowed to work on. Boards a person archived are out of service and are not listed — every tool refuses them, so a board id you got from somewhere else and cannot see here will 403.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'list_tickets',
    description:
      "List a board's tickets. Each carries status, priority, assignees (agent ids and humans as user:<id>), labels, due date, human estimate (estimatedHours), sub-task parent (parentId), and comment count. Archived tickets are not listed — a person took them off the table and they refuse every agent write. Optional filters narrow the list.",
    parameters: {
      type: 'object',
      properties: { boardId: str('Board id (from list_boards)'), status: str('Only this status'), assignee: str('Only tickets assigned to this agent id or user:<id>') },
      required: ['boardId'],
    },
  },
  {
    name: 'get_ticket',
    description:
      'Get a ticket in full: fields (incl. human estimate, sub-task parentId, mixed assignees — agents by id, humans as user:<id>), comments, activity, watchers, reviews, dependencies. May include `workflows` — how this kind of work is done here; follow their instructions. Tickets may carry an `attachments` array (files + knowledge/artifact refs) — read a file with fetch_attachment.',
    parameters: { type: 'object', properties: { taskId: str('Ticket id') }, required: ['taskId'] },
  },
  {
    name: 'triage_ticket',
    description:
      'Triage a ticket: adjust priority, effort, labels, description, due date — and move its status FORWARD. Forward means: start work you were already assigned (→ in_progress), park it (→ blocked), or hand it to review (→ quality_review). You cannot move it back. Restarting your own blocked ticket, taking anything out of quality_review, and any write at all to a ticket a person has taken off the table — all refuse.',
    parameters: {
      type: 'object',
      properties: {
        taskId: str('Ticket id'),
        status: { type: 'string', enum: ['in_progress', 'blocked', 'quality_review'], description: 'Move the ticket forward to this status' },
        priority: { type: 'string', enum: ['low', 'medium', 'high', 'urgent'], description: 'New priority' },
        labels: { type: 'array', items: { type: 'string' }, description: 'Replacement label set' },
      },
      required: ['taskId'],
    },
  },
  {
    name: 'comment',
    description: 'Comment on a ticket. Comments stay allowed on a ticket in review, which is where anything further goes once you have reported an outcome.',
    parameters: { type: 'object', properties: { taskId: str('Ticket id'), body: str('The comment, in markdown') }, required: ['taskId', 'body'] },
  },
  {
    name: 'report_outcome',
    description:
      'Report the outcome of work on a ticket and hand it to the board\'s review column ("Quality review" unless your board renamed it). A human signs off on done. This is your LAST status move on that ticket: once it is in review, triage_ticket cannot take it back out, so add anything further as a comment. Only ever on a live ticket.',
    parameters: {
      type: 'object',
      properties: { taskId: str('Ticket id'), outcome: str('WHAT changed'), resolution: str('HOW it was done') },
      required: ['taskId', 'outcome'],
    },
  },
  {
    name: 'report_gap',
    description:
      "Report a capability gap — use ONLY when you genuinely cannot do assigned work properly: a tool or access you're missing, or an org-specific process you'd be guessing at. NOT for small tasks you can simply do with your own judgment, and never twice for the same kind of work (repeats are deduplicated; the team is already aware). The team sees gaps ranked by recurrence and can build a workflow/skill to cover them. taskId is optional and gated: it writes a line onto that ticket, so a board you are not allowed on — or a ticket that is off the table — refuses it.",
    parameters: {
      type: 'object',
      properties: { summary: str('What a flow would need, in one line'), taskId: str('The ticket this blocks, if any') },
      required: ['summary'],
    },
  },
  {
    name: 'add_time',
    description: "Add time spent to a ticket's auto-accumulated total. Log it as you work. Only ever on a live ticket.",
    parameters: { type: 'object', properties: { taskId: str('Ticket id'), minutes: { type: 'number', description: 'Real minutes spent' } }, required: ['taskId', 'minutes'] },
  },
  {
    name: 'add_dependency',
    description:
      'Mark a ticket as blocked by another ticket on the same board. The edge lands on BOTH tickets, so BOTH must be live: one that is off the table refuses with 403 on either side of the edge, and the error names which. Removing an edge is a human call.',
    parameters: { type: 'object', properties: { taskId: str('The blocked ticket'), blockedBy: str('The ticket it waits on') }, required: ['taskId', 'blockedBy'] },
  },
  {
    name: 'search_knowledge',
    description:
      'Search the workspace and knowledge you have access to (org knowledgebase, departmental collections, and past chats/channels/plans/research). Use it to recall earlier discussion or ground an answer in real docs — e.g. "what did we decide about X a couple weeks ago". Returns ranked snippets with pointers.',
    parameters: { type: 'object', properties: { query: str('What to look for') }, required: ['query'] },
  },
  {
    name: 'read_channel',
    description: 'Read recent messages in a channel you belong to. Pass sinceSeq to get only newer messages than a seq you already saw.',
    parameters: { type: 'object', properties: { channel: str('Channel name'), sinceSeq: { type: 'number', description: 'Only messages after this seq' } }, required: ['channel'] },
  },
  {
    name: 'post_to_channel',
    description: 'Post a message to a channel you belong to. Use @name to mention teammates. Post when you have something useful — progress, an answer, a blocker, a question — not chatter.',
    parameters: { type: 'object', properties: { channel: str('Channel name'), body: str('The message') }, required: ['channel', 'body'] },
  },
  {
    name: 'message_user',
    description:
      'Start (or continue) a direct conversation with a human teammate — your message lands in their chat with you plus an inbox notification. Reach for it when something genuinely needs THEIR attention now: work of theirs is blocked on you, you found something they should decide on, a deadline is about to slip. Not for status updates (comment on the ticket) or things the whole room should see (post_to_channel). Rate-limited per person per day — if it declines, respect the returned reason.',
    parameters: { type: 'object', properties: { user: str('Their email or user id'), body: str('The message') }, required: ['user', 'body'] },
  },
  {
    name: 'list_teammates',
    description:
      "The company directory: every teammate's name and email. Use it to resolve a person's name into an email for draft_email, add_board_member, or board/channel questions ('who is Priya?').",
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'log_usage',
    description: "Report the LLM tokens you burned working a ticket. Feeds the ticket's cost rollup and the fleet ledger. Log it as you go: only ever on a live ticket.",
    parameters: {
      type: 'object',
      properties: { taskId: str('Ticket id'), promptTokens: { type: 'number' }, completionTokens: { type: 'number' } },
      required: ['taskId'],
    },
  },
  {
    name: 'report_problem',
    description:
      'Something on YOUR side is broken (a tool errors, a connection refuses, credentials missing) and a teammate is affected. This alerts the workspace admins and files a Helpdesk ticket with the technical details. Use it INSTEAD of explaining endpoints/ports/errors to non-technical people — then relay the returned plain-language reassurance and offer what you can still do. taskId is optional and gated the same way report_gap\'s is: a board you are not allowed on — or a ticket that is off the table — refuses it.',
    parameters: { type: 'object', properties: { summary: str('What broke, technically'), taskId: str('The ticket it affects, if any') }, required: ['summary'] },
  },
] as const

export const talariaToolNames = (): string[] => TALARIA_TOOLS.map((t) => t.name)

/** The subset a given harness should be offered. A tool surface is a prompt:
 *  handing a briefing chat the ticket-triage tools measures its tolerance for
 *  irrelevant options rather than its ability to answer a question. */
export const toolsNamed = (names: readonly string[]): SandboxTool[] => TALARIA_TOOLS.filter((t) => names.includes(t.name))
