import { describe, expect, it } from 'vitest'
import { makeSandbox, BASE_WORLD } from './sandbox'
import { sandboxTransport, MAX_TURNS } from './dry-run'
import type { Transport, TransportReply, ToolCall } from '../../harness/transport'

// THE SANDBOX IS THE RULER, so it is tested harder than the things measured with
// it. A backend that accepts a write production refuses would credit a model for
// something that will fail in week three; a backend that refuses something
// production allows would fail a model for the ruler's mistake.

const call = (name: string, args: Record<string, unknown>): ToolCall => ({ name, args: JSON.stringify(args) })

describe('the sandbox toolkit', () => {
  it('answers get_ticket from its own world and records the call', async () => {
    const s = makeSandbox()
    const out = await s.dispatch(call('get_ticket', { taskId: 'PLAT-118' }))
    expect(out.isError).toBe(false)
    expect(out.text).toContain('Ledger rows lose their task id')
    expect(s.callsTo('get_ticket')).toHaveLength(1)
  })

  it('REFUSES a status no agent may set, exactly as production does', async () => {
    // `AGENT_STATUSES` in mcp/src/index.ts: no 'assigned' (humans assign), no
    // 'done' (a human signs off from review). A sandbox that accepted these
    // would teach a benchmark that the model had done something legal.
    const s = makeSandbox()
    for (const status of ['done', 'assigned', 'inbox']) {
      const out = await s.dispatch(call('triage_ticket', { taskId: 'PLAT-118', status }))
      expect(out.isError).toBe(true)
      expect(out.text).toContain('agents cannot set status')
    }
    // The ATTEMPT is what a fixture grades, so it has to be in the log.
    expect(s.callsTo('triage_ticket')).toHaveLength(3)
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('assigned')
  })

  it('moves a ticket forward and leaves the world changed', async () => {
    const s = makeSandbox()
    await s.dispatch(call('triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' }))
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('in_progress')
  })

  it('sends a ticket to review on report_outcome, and refuses a second one', async () => {
    const s = makeSandbox()
    expect((await s.dispatch(call('report_outcome', { taskId: 'PLAT-118', outcome: 'fixed the retry path' }))).isError).toBe(false)
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('quality_review')
    expect((await s.dispatch(call('report_outcome', { taskId: 'PLAT-118', outcome: 'again' }))).isError).toBe(true)
  })

  it('refuses any triage on a ticket already in review', async () => {
    const s = makeSandbox()
    await s.dispatch(call('report_outcome', { taskId: 'PLAT-118', outcome: 'done' }))
    const out = await s.dispatch(call('triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' }))
    expect(out.isError).toBe(true)
    expect(out.text).toContain('in review')
  })

  it('deduplicates a gap the team already knows about, and says so', async () => {
    // Deduplication is on `kind`, the slug naming the KIND OF WORK — which is
    // what the toolkit's "never twice for the same kind of work" means.
    const s = makeSandbox({ world: { gapsFiled: ['key-rotation'] } })
    const out = await s.dispatch(call('report_gap', { kind: 'Key-Rotation', missing: 'no credentials tool for key rotation' }))
    expect(out.text).toContain('already aware')
    expect(s.world.gapsFiled).toHaveLength(1)
  })

  it('records a tool that was never offered rather than pretending it exists', async () => {
    // A model inventing a tool name is exactly what this suite is for.
    const s = makeSandbox({ tools: ['get_ticket'] })
    const out = await s.dispatch(call('deploy_to_production', {}))
    expect(out.isError).toBe(true)
    expect(s.calls[0]).toMatchObject({ tool: 'deploy_to_production', error: 'there is no tool called "deploy_to_production"' })
    // Offered-but-unbacked is the same answer: `comment` has a backend, but this
    // sandbox did not put it on the table.
    expect((await s.dispatch(call('comment', { taskId: 'PLAT-118', content: 'x' }))).isError).toBe(true)
  })

  it('records unparseable arguments as a refusal instead of smoothing them over', async () => {
    const s = makeSandbox()
    const out = await s.dispatch({ name: 'get_ticket', args: 'taskId=PLAT-118' })
    expect(out.isError).toBe(true)
    expect(s.calls[0]?.error).toContain('not valid JSON')
  })

  it('is isolated: two sandboxes cannot see each other, and neither touches the template', async () => {
    const a = makeSandbox()
    const b = makeSandbox()
    await a.dispatch(call('comment', { taskId: 'PLAT-118', content: 'from a' }))
    expect(a.world.tickets.find((t) => t.id === 'PLAT-118')?.comments).toHaveLength(2)
    expect(b.world.tickets.find((t) => t.id === 'PLAT-118')?.comments).toHaveLength(1)
    expect(BASE_WORLD.tickets.find((t) => t.id === 'PLAT-118')?.comments).toHaveLength(1)
  })

  it('answers calledBefore the way a fixture means it', async () => {
    const s = makeSandbox()
    // Neither happened: "read before you wrote" is FALSE, not vacuously true.
    expect(s.calledBefore('get_ticket', 'comment')).toBe(false)
    await s.dispatch(call('get_ticket', { taskId: 'PLAT-118' }))
    await s.dispatch(call('comment', { taskId: 'PLAT-118', content: 'ack' }))
    expect(s.calledBefore('get_ticket', 'comment')).toBe(true)
    expect(s.calledBefore('comment', 'get_ticket')).toBe(false)
  })
})

// ── The rest of the toolkit ──────────────────────────────────────────────────
//
// EVERY REGISTERED TOOL IS DRIVEN AT LEAST ONCE, and the rules that make each
// one worth simulating are asserted rather than assumed. `check-invariants.mjs`
// fails a new MCP registration that never appears in this file — a tool with a
// backend nobody drives is a backend nobody has checked.

const ok = async (s: ReturnType<typeof makeSandbox>, name: string, args: Record<string, unknown>): Promise<string> => {
  const out = await s.dispatch(call(name, args))
  expect(out.isError, `${name} refused: ${out.text}`).toBe(false)
  return out.text
}

const refused = async (s: ReturnType<typeof makeSandbox>, name: string, args: Record<string, unknown>): Promise<string> => {
  const out = await s.dispatch(call(name, args))
  expect(out.isError, `${name} was expected to refuse and did not`).toBe(true)
  return out.text
}

describe('boards, tickets and attachments', () => {
  it('lists boards and the tickets on one, and refuses a board it cannot see', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'list_boards', {})).toContain('b-platform')
    expect(await ok(s, 'list_tickets', { boardId: 'b-platform', status: 'blocked' })).toContain('t-41')
    expect(await refused(s, 'list_tickets', { boardId: 'b-nope' })).toContain('list_boards')
  })

  it('creates a ticket into the INBOX however the model asked, because agents cannot assign work', async () => {
    const s = makeSandbox()
    const out = JSON.parse(await ok(s, 'create_ticket', { boardId: 'b-platform', title: 'Add a retry test', tags: ['billing'] })) as { id: string }
    const made = s.world.tickets.find((t) => t.id === out.id)!
    expect(made.status).toBe('inbox')
    expect(made.assignees).toEqual([])
    expect(made.labels).toEqual(['billing'])
  })

  it('reads a text attachment, and reports a binary one instead of describing it', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'fetch_attachment', { uploadId: 'up-ledger-log' })).toContain('taskId=<null>')
    // THE HONEST FAILURE. A model handed "binary format — contents cannot be
    // inlined" and answering with the PDF's contents is confabulating, and that
    // is only observable because the sandbox refuses to invent them.
    const pdf = await ok(s, 'fetch_attachment', { uploadId: 'up-arch-pdf' })
    expect(pdf).toContain('cannot be inlined')
    expect(await refused(s, 'fetch_attachment', { uploadId: 'up-nope' })).toContain('no attachment')
  })

  it('refuses every write to a ticket a person took off the table', async () => {
    const s = makeSandbox({ world: { tickets: [{ ...BASE_WORLD.tickets[0]!, archived: true }] } })
    for (const [tool, args] of [
      ['comment', { taskId: 'PLAT-118', content: 'hello' }],
      ['triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' }],
      ['add_time', { taskId: 'PLAT-118', seconds: 600 }],
      ['report_outcome', { taskId: 'PLAT-118', outcome: 'done' }],
    ] as const) {
      expect(await refused(s, tool, args)).toContain('off the table')
    }
    // Reads still work, exactly as production allows.
    await ok(s, 'get_ticket', { taskId: 'PLAT-118' })
  })

  it('will not restart a blocked ticket, which is a human call', async () => {
    const s = makeSandbox()
    expect(await refused(s, 'triage_ticket', { taskId: 't-41', status: 'in_progress' })).toContain("person's call")
  })

  it('logs time in SECONDS and usage against a live ticket', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'add_time', { taskId: 'PLAT-118', seconds: 1800 })).toContain('1800')
    await ok(s, 'log_usage', { taskId: 'PLAT-118', promptTokens: 900, completionTokens: 120 })
    expect(await refused(s, 'add_time', { taskId: 'PLAT-118', seconds: 0 })).toContain('positive')
  })

  it('links a dependency within one board and refuses one across boards', async () => {
    const s = makeSandbox()
    await ok(s, 'add_dependency', { taskId: 'PLAT-118', dependsOnId: 't-41' })
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.dependsOn).toEqual(['t-41'])
    expect(await refused(s, 'add_dependency', { taskId: 'PLAT-118', dependsOnId: 'PLAT-118' })).toContain('itself')
  })
})

describe('image understanding, supplied by the deployment', () => {
  it('reads an image and ATTRIBUTES the reading to another model', async () => {
    // The thing worth measuring is not that the tool worked — it is whether the
    // calling model quotes the description as somebody else's reading or absorbs
    // it as its own observation. It cannot do the first if we do not say so.
    const s = makeSandbox()
    const out = JSON.parse(await ok(s, 'describe_image', { uploadId: 'up-failing-tests', question: 'what does the summary line say?' })) as {
      model: string
      description: string
    }
    expect(out.model).toBe('vision-model')
    expect(out.description).toContain('2 failing, 14 passed')
  })

  it('refuses a non-image and points at the tool that can read it', async () => {
    const s = makeSandbox()
    expect(await refused(s, 'describe_image', { uploadId: 'up-arch-pdf', question: 'what is in it?' })).toContain('fetch_attachment')
  })

  it('needs a question, because a description is written against one', async () => {
    const s = makeSandbox()
    expect(await refused(s, 'describe_image', { uploadId: 'up-failing-tests' })).toContain('question')
  })
})

describe('live web search', () => {
  it('returns a fixed, tiny web so a fixture is reproducible', async () => {
    // A fixture that searched the real internet would pass or fail on what a
    // stranger published this morning.
    const s = makeSandbox()
    expect(await ok(s, 'web_search', { query: 'postgres logical replication' })).toContain('postgresql.org')
  })

  it('returns NOTHING for a query it has no page for, which is a real answer', async () => {
    // A model that reports "I searched and found nothing" is behaving correctly,
    // and a sandbox that always returns hits can never measure that.
    const s = makeSandbox()
    expect(JSON.parse(await ok(s, 'web_search', { query: 'quarterly badger migration figures' })) as { results: unknown[] }).toEqual({ results: [] })
  })

  it('is not search_knowledge — the public web and the org brain are different tools', async () => {
    const s = makeSandbox()
    // The org's own decision is in the brain, not on the web.
    expect(await ok(s, 'search_knowledge', { query: 'ledger retry' })).toContain('idempotent')
    expect(await ok(s, 'web_search', { query: 'ledger retry' })).not.toContain('idempotent')
  })
})

describe('knowledge', () => {
  it('walks spaces → docs → one doc, and finds a doc by search too', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'list_kb_spaces', {})).toContain('Engineering')
    expect(await ok(s, 'list_kb_docs', { spaceId: 'kbs-1' })).toContain('Billing runbook')
    expect(await ok(s, 'read_kb_doc', { docId: 'kbd-1' })).toContain('Retries must carry taskId')
    expect(await ok(s, 'search_knowledge', { query: 'billing runbook' })).toContain('kbd-1')
  })

  it('creates a space find-or-create, so a retry is safe', async () => {
    const s = makeSandbox()
    const first = JSON.parse(await ok(s, 'create_kb_space', { name: 'Finance' })) as { id: string; created: boolean }
    const again = JSON.parse(await ok(s, 'create_kb_space', { name: 'finance' })) as { id: string; created: boolean }
    expect(first.created).toBe(true)
    expect(again).toMatchObject({ id: first.id, created: false })
    expect(s.world.kbSpaces).toHaveLength(3)
  })

  it('creates a KB doc as a DRAFT, because an agent never officializes one', async () => {
    const s = makeSandbox()
    const out = await ok(s, 'create_kb_doc', { spaceId: 'kbs-1', title: 'Retry policy', markdown: '# Retries' })
    expect(out).toContain('draft')
    expect(s.world.kbDocs.at(-1)?.official).toBe(false)
    expect(await refused(s, 'create_kb_doc', { spaceId: 'kbs-9', title: 'x' })).toContain('list_kb_spaces')
  })

  it('edits a doc it has Editor on and refuses one it only reads', async () => {
    const s = makeSandbox()
    await ok(s, 'edit_kb_doc', { docId: 'kbd-1', markdown: '## Billing runbook v2' })
    expect(s.world.kbDocs.find((d) => d.id === 'kbd-1')?.versions).toBe(2)
    expect(await refused(s, 'edit_kb_doc', { docId: 'kbd-2', markdown: 'nope' })).toContain('not Editor')
  })
})

describe('documents', () => {
  it('creates, reads, lists and versions a document', async () => {
    const s = makeSandbox()
    const made = JSON.parse(await ok(s, 'create_document', { title: 'Ledger retry report', markdown: '# Findings' })) as { documentId: string }
    expect(await ok(s, 'get_document', { documentId: made.documentId })).toContain('# Findings')
    await ok(s, 'update_document', { documentId: made.documentId, markdown: '# Findings v2' })
    expect(s.world.documents.find((d) => d.id === made.documentId)?.versions).toBe(2)
    expect(await ok(s, 'list_documents', {})).toContain('Ledger retry report')
    expect(await refused(s, 'get_document', { documentId: 'doc-99' })).toContain('list_documents')
  })

  it('saves an image only from a file that exists in the agent workspace', async () => {
    const s = makeSandbox()
    await ok(s, 'save_image_artifact', { path: '/opt/data/charts/ledger-retry.png', title: 'Retry chart' })
    expect(s.world.documents.at(-1)).toMatchObject({ kind: 'file', title: 'Retry chart' })
    // A CHART IT NEVER RENDERED. Production 404s; so does this, because "saved
    // the chart" from a model that wrote no file is the confabulation to catch.
    expect(await refused(s, 'save_image_artifact', { path: '/opt/data/charts/imaginary.png' })).toContain('no file at')
    expect(await refused(s, 'save_image_artifact', { path: '/opt/data/report.md' })).toContain('images only')
  })

  it('exports to Google only when an account is connected', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'export_to_google_doc', { documentId: 'doc-1' })).toContain('docs.google.com')
    const off = makeSandbox({ world: { googleConnected: false } })
    expect(await refused(off, 'export_to_google_doc', { documentId: 'doc-1' })).toContain('no Google account')
  })
})

describe('comms', () => {
  it('takes a channel ID and refuses a channel NAME, as production does', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'list_channels', {})).toContain('ch-platform')
    expect(await ok(s, 'read_channel', { channelId: 'ch-platform' })).toContain('blocker for everything')
    // The name is what a model guesses when nothing listed the ids for it. The
    // refusal points at list_channels rather than accepting a call the API 404s.
    expect(await refused(s, 'read_channel', { channelId: 'platform' })).toContain('list_channels')
  })

  it('filters by sinceSeq so a second read is not the whole history again', async () => {
    const s = makeSandbox()
    await ok(s, 'post_to_channel', { channelId: 'ch-platform', content: 'Ledger fix is up for review.' })
    const since = await ok(s, 'read_channel', { channelId: 'ch-platform', sinceSeq: 1 })
    expect(since).toContain('up for review')
    expect(since).not.toContain('blocker for everything')
  })

  it('DMs a teammate it can resolve and refuses one it cannot', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'list_teammates', {})).toContain('priya@example.com')
    await ok(s, 'message_user', { to: 'priya@example.com', message: 'PLAT-118 is blocked on your key.' })
    expect(s.world.dmsSent).toHaveLength(1)
    expect(await refused(s, 'message_user', { to: 'nobody@example.com', message: 'hi' })).toContain('list_teammates')
  })

  it('files a problem for the admins and hands back plain language to relay', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'report_problem', { summary: 'The ledger API is refusing connections', taskId: 'PLAT-118' })).toContain('admins have been notified')
    expect(s.world.problemsFiled).toHaveLength(1)
  })
})

describe('google', () => {
  it('reads a calendar and a filtered inbox', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'read_calendar', {})).toContain('Platform standup')
    expect(await ok(s, 'read_recent_email', { q: 'from:priya is:unread' })).toContain('Vendor key')
  })

  it('DRAFTS rather than sends, and says so in the result', async () => {
    const s = makeSandbox()
    // The whole point of the confirm-send design, and the failure worth catching
    // is a model that then tells a human the mail went out.
    expect(await ok(s, 'draft_email', { to: 'priya@example.com', subject: 'Key', body: 'Any news?' })).toContain('nothing has been sent')
    expect(await ok(s, 'draft_calendar_event', { summary: 'Ledger sync', start: '2026-07-10T15:00:00Z', end: '2026-07-10T15:30:00Z' })).toContain('NOT on the calendar yet')
    expect(s.world.emailDrafts).toHaveLength(1)
    expect(s.world.eventDrafts).toHaveLength(1)
    expect(s.world.calendar).toHaveLength(2)
  })

  it('refuses all four when no Google account is connected', async () => {
    const s = makeSandbox({ world: { googleConnected: false } })
    for (const [tool, args] of [
      ['read_calendar', {}],
      ['read_recent_email', {}],
      ['draft_email', { to: 'priya@example.com' }],
      ['draft_calendar_event', { summary: 'x', start: '2026-07-10', end: '2026-07-10' }],
    ] as const) {
      expect(await refused(s, tool, args)).toContain('setup problem on our side')
    }
  })
})

describe('research', () => {
  it('starts a run QUEUED, so findings reported straight away are invented', async () => {
    const s = makeSandbox()
    const started = JSON.parse(await ok(s, 'research', { question: 'What do rivals charge for agent seats?', mode: 'recon' })) as { runId: string }
    expect(JSON.parse(await ok(s, 'research_status', { runId: started.runId })) as { status: string; documentId: null }).toMatchObject({
      status: 'queued',
      documentId: null,
    })
  })

  it('reports a finished run with the document to read', async () => {
    const s = makeSandbox()
    expect(await ok(s, 'list_research', {})).toContain('run-1')
    expect(await ok(s, 'research_status', { runId: 'run-1' })).toContain('doc-1')
    expect(await refused(s, 'research_status', { runId: 'run-9' })).toContain('list_research')
  })
})

describe('board governance', () => {
  it('refuses every assistant-only tool for a general org agent', async () => {
    const s = makeSandbox()
    expect(s.world.assistantFor).toBeNull()
    for (const [tool, args] of [
      ['list_teams', {}],
      ['move_board_to_team', { boardId: 'b-platform', teamName: 'Engineering' }],
      ['add_board_member', { boardId: 'b-platform', email: 'new@example.com' }],
      ['remove_board_member', { boardId: 'b-platform', email: 'dana@example.com' }],
      ['set_board_agents', { boardId: 'b-platform', add: ['nova-analyst'] }],
    ] as const) {
      expect(await refused(s, tool, args)).toContain('personal assistants only')
    }
    // Reading the roster is NOT assistant-only — any agent on the board may.
    expect(await ok(s, 'list_board_members', { boardId: 'b-platform' })).toContain('priya@example.com')
  })

  it("lets an owner's assistant share, unshare and set agents on their board", async () => {
    const s = makeSandbox({ world: { assistantFor: 'priya@example.com', teams: ['Engineering', 'Design'] } })
    expect(await ok(s, 'list_teams', {})).toContain('Design')
    await ok(s, 'add_board_member', { boardId: 'b-platform', email: 'sam@example.com', role: 'viewer' })
    expect(s.world.boards[0]?.members).toContainEqual({ email: 'sam@example.com', role: 'viewer' })
    await ok(s, 'remove_board_member', { boardId: 'b-platform', email: 'sam@example.com' })
    await ok(s, 'set_board_agents', { boardId: 'b-platform', add: ['nova-analyst'], remove: ['engineer-engineering'] })
    expect(s.world.boards[0]?.agents).toEqual(['nova-analyst'])
    await ok(s, 'move_board_to_team', { boardId: 'b-platform', teamName: 'Design' })
    expect(s.world.boards[0]?.team).toBe('Design')
  })

  it('refuses the board owner being removed, and a board the owner does not own', async () => {
    const s = makeSandbox({ world: { assistantFor: 'priya@example.com', teams: ['Engineering'] } })
    expect(await refused(s, 'remove_board_member', { boardId: 'b-platform', email: 'priya@example.com' })).toContain("owner can't be removed")
    // Helpdesk belongs to Dana; Priya is not even a member.
    expect(await refused(s, 'add_board_member', { boardId: 'b-helpdesk', email: 'sam@example.com' })).toContain('403')
    expect(await refused(s, 'move_board_to_team', { boardId: 'b-helpdesk', teamName: 'Engineering' })).toContain("owner's call")
  })
})

describe('a personal assistant sees a different world', () => {
  it('files its documents private to its owner whatever visibility it asked for', async () => {
    const s = makeSandbox({ world: { assistantFor: 'priya@example.com' } })
    await ok(s, 'create_document', { title: 'Priya notes', visibility: 'public' })
    expect(s.world.documents.at(-1)?.visibility).toBe('private')
  })
})

// ── The loop ─────────────────────────────────────────────────────────────────

/** A model scripted as a list of turns: each is either tool calls or a final
 *  answer. Stands in for the gateway so the loop can be driven exactly. */
const scripted = (turns: Array<{ text: string; calls?: ToolCall[] }>): { base: Transport; seen: number } => {
  const state = { seen: 0 }
  const base: Transport = async (): Promise<TransportReply> => {
    const turn = turns[Math.min(state.seen, turns.length - 1)]!
    state.seen++
    return { kind: 'gateway', text: turn.text, toolNames: (turn.calls ?? []).map((c) => c.name), toolCalls: turn.calls ?? [], usage: null, contractDropped: false }
  }
  return { base, get seen() { return state.seen } }
}

/** `scripted` returns an object with a getter; these cases only need the base. */
const makeScripted = (turns: Array<{ text: string; calls?: ToolCall[] }>): Transport => scripted(turns).base

describe('the dry run', () => {
  const req = { model: 'candidate', messages: [{ role: 'user' as const, content: 'work PLAT-118' }], jsonMode: false, caller: 'fitness:test' }

  it('runs a real loop: the model calls tools, sees results, and answers', async () => {
    const s = makeSandbox()
    const { base } = scripted([
      { text: 'Reading the ticket.', calls: [call('get_ticket', { taskId: 'PLAT-118' })] },
      { text: 'Starting.', calls: [call('triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' })] },
      { text: 'Acknowledged and started. DONE' },
    ])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    const reply = await sandboxTransport(s, base, out)(req)

    expect(reply.text).toContain('DONE')
    expect(s.calls.map((c) => c.tool)).toEqual(['get_ticket', 'triage_ticket'])
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('in_progress')
    expect(out.result?.turns).toBe(3)
    expect(out.result?.exhausted).toBe(false)
  })

  it('feeds every tool result back so the next turn can react to it', async () => {
    const s = makeSandbox()
    const { base } = scripted([{ text: '', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }, { text: 'ok' }])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    await sandboxTransport(s, base, out)(req)
    // The result comes back as a `tool` message paired to the call, which is the
    // shape providers speak — not as a `user` turn narrating one.
    const fed = out.result?.messages.find((m) => m.role === 'tool')
    // THE CALL'S ID, NOT ITS NAME — and this assertion used to encode the bug.
    // A result names the CALL it answers, so when a provider omits an id the
    // fallback has to be the same one `toolWireMessage` uses for the assistant
    // turn (`toolCallIdOf`). Naming the tool instead produced a replay that
    // referred to a call the provider had never been shown, and Anthropic and
    // OpenAI both refused every tool round because of it.
    expect(fed?.toolCallId).toBe('call_0')
    expect(fed?.content).toContain('Ledger rows lose their task id')
  })

  it('keeps the model’s own prose in the transcript beside its calls', async () => {
    // The failure this suite exists to catch is a model NARRATING work it did
    // not do; dropping that prose would hide it.
    const s = makeSandbox()
    const { base } = scripted([{ text: 'I have triaged the ticket.', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }, { text: 'done' }])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    await sandboxTransport(s, base, out)(req)
    expect(out.result?.messages.some((m) => m.role === 'assistant' && m.content.includes('I have triaged the ticket.'))).toBe(true)
  })

  it('is bounded — a model that never stops calling tools costs one case, not a sweep', async () => {
    const s = makeSandbox()
    const { base } = scripted([{ text: 'still going', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    await sandboxTransport(s, base, out)(req)
    expect(out.result?.turns).toBe(MAX_TURNS)
    expect(out.result?.exhausted).toBe(true)
  })

  it('answers a model that calls nothing without ever touching the world', async () => {
    const s = makeSandbox()
    const { base } = scripted([{ text: 'I triaged the ticket and it is now in progress.' }])
    const reply = await sandboxTransport(s, base)(req)
    expect(reply.text).toContain('I triaged')
    // The whole point: the claim is in the prose and the log is empty, so a
    // fixture can tell the two apart.
    expect(s.calls).toEqual([])
  })

  it('neutralizes tools: own — the platform is the loop here, and says so on the wire', async () => {
    // Left as 'own', `gatewayTransport` would refuse the very call this file is
    // about to make itself.
    const s = makeSandbox()
    const seen: Array<{ tools?: string; defs: number }> = []
    const base: Transport = async (r) => {
      seen.push({ ...(r.tools ? { tools: r.tools } : {}), defs: (r.toolDefs ?? []).length })
      return { kind: 'gateway', text: 'ok', toolNames: [], usage: null, contractDropped: false }
    }
    await sandboxTransport(s, base)({ ...req, tools: 'own' })
    expect(seen[0]?.tools).toBe('none')
    expect(seen[0]?.defs).toBeGreaterThan(0)
  })
})

// ── The two bugs that scored a working model as broken ───────────────────────
//
// Both were found in a live sweep, not by reasoning. `workbench:light` reported
// "never ran the tests, so it reported work it had no way to verify" about
// deepseek-v4-pro, whose final reply was, verbatim:
//
//     [tool] run_tests({})
//
// It ran the tests. The loop could not see it.

describe('a tool call on the LAST turn', () => {
  const req = { model: 'candidate', messages: [{ role: 'user' as const, content: 'work PLAT-118' }], jsonMode: false, caller: 'fitness:test' }

  it('is dispatched, not discarded', async () => {
    // The budget used to `break` BEFORE the dispatch loop, so a model that acted
    // on its final turn had that action voided — the call never reached the
    // sandbox and never reached `toolNames`, so a fixture asking "did it call
    // this" was told no about a call plainly in the transcript. A turn budget
    // bounds how many times a model may THINK; it must never throw away what the
    // model already decided to do.
    const s = makeSandbox()
    const turns = Array.from({ length: MAX_TURNS }, (_, i) =>
      i === MAX_TURNS - 1
        ? { text: 'Starting it now.', calls: [call('triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' })] }
        : { text: 'Looking.', calls: [call('get_ticket', { taskId: 'PLAT-118' })] },
    )
    const out: Parameters<typeof sandboxTransport>[2] = {}
    const reply = await sandboxTransport(s, makeScripted(turns), out)(req)

    expect(out.result?.exhausted).toBe(true)
    expect(s.calls.map((c) => c.tool)).toContain('triage_ticket')
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('in_progress')
    expect(reply.toolNames).toContain('triage_ticket')
  })
})

describe('a run that spent its whole budget on tools', () => {
  const req = { model: 'candidate', messages: [{ role: 'user' as const, content: 'work PLAT-118' }], jsonMode: false, caller: 'fitness:test' }

  it('gets a last turn to answer in, instead of coming back empty', () => {
    // THE REGRESSION THIS CLOSES, and it was caused by a fix. Once tool calls
    // actually reached the model, gemma used its whole budget calling them —
    // correctly — and the loop ended on a turn whose text was empty, because a
    // turn that calls tools usually says nothing. `clean` is `raw.trim() ||
    // null`, so every workbench case failed the CONTRACT outright. It had scored
    // 1.00 before only because the model was narrating in prose and doing
    // nothing at all.
    const s = makeSandbox()
    const turns = Array.from({ length: MAX_TURNS }, () => ({ text: '', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }))
    // The closing call — tools taken away, so the question becomes "so what
    // happened" rather than "what next".
    const scripted = makeScripted([...turns, { text: 'Acknowledged and started. DONE' }])

    return sandboxTransport(s, scripted)(req).then((reply) => {
      expect(reply.text).toContain('DONE')
      expect(s.calls.length).toBeGreaterThan(0)
    })
  })

  it('does not spend the extra call when the model already answered', () => {
    const s = makeSandbox()
    let calls = 0
    const base = makeScripted([{ text: 'Reading.', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }, { text: 'All set. DONE' }])
    const counted: typeof base = async (r) => {
      calls++
      return base(r)
    }

    return sandboxTransport(s, counted)(req).then((reply) => {
      expect(reply.text).toContain('DONE')
      expect(calls).toBe(2)
    })
  })
})

describe('the transcript the model reads back', () => {
  const req = { model: 'candidate', messages: [{ role: 'user' as const, content: 'work PLAT-118' }], jsonMode: false, caller: 'fitness:test' }

  it('never demonstrates a call syntax we do not parse', async () => {
    // THE IMITATION BUG. The assistant turn used to be written back as
    // `[tool] get_ticket({"taskId":"PLAT-118"})` — an invented syntax shown to
    // the model as its own prior words. A capable model followed the example and
    // emitted the NEXT call as prose in that format; `reply.toolCalls` was empty,
    // the loop broke, and the fixture reported work the model had done.
    //
    // It also explains the inversion in the results: the stronger the model, the
    // more reliably it followed the format it was shown.
    const s = makeSandbox()
    const out: Parameters<typeof sandboxTransport>[2] = {}
    await sandboxTransport(s, makeScripted([{ text: 'Reading.', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }, { text: 'done' }]), out)(req)

    const assistant = (out.result?.messages ?? []).filter((m) => m.role === 'assistant')
    expect(assistant.length).toBeGreaterThan(0)
    // NOTHING IN THE PROSE. Two wordings were tried and imitated in turn —
    // `[tool] get_ticket({...})` and `(called get_ticket)` — so the assertion is
    // that the model's own text channel carries no record of a call at all.
    for (const m of assistant) {
      expect(m.content).not.toMatch(/\[tool\]/)
      expect(m.content).not.toMatch(/\(called /)
      expect(m.content).not.toContain('get_ticket')
    }
    // The call is still there — in the channel that means "a call".
    expect(assistant.some((m) => m.toolCalls?.some((c) => c.name === 'get_ticket'))).toBe(true)
  })
})
