import type { Artifact } from '@/lib/artifacts'
import type { Conversation } from '@/lib/conversations.svelte'
import type { BriefEntry, BriefLine, BriefView, CommsState } from '@/lib/daily-brief-types'
import type { ResearchRun } from '@/lib/research'
import type { TaskStatus } from '@/lib/task-const'
import type { HomeSummary, WorkItem } from '@/routes/app/home/home'

export type InboxLane = 'needs' | 'running' | 'done'
export type InboxState = 'waiting' | 'failed' | 'running' | 'done'
export type InboxSource = 'decision' | 'message' | 'ticket' | 'session' | 'research' | 'doc'
export type InboxNeed = 'blocked' | 'signoff' | 'triage'
export type InboxGlyph = InboxState | InboxNeed

export interface InboxPerson {
  name: string
  agent: boolean
}

export interface InboxStep {
  label: string
  at: string | null
}

export interface InboxCard {
  /** Source-prefixed and stable: `brief:<lineKey>`, `ticket:<id>`, `session:<id>`, `run:<id>`, `doc:<id>`. */
  key: string
  source: InboxSource
  state: InboxState
  /** Set only on a waiting card whose source says what it waits for. */
  need: InboxNeed | null
  /** The source's own word for where it stands ("Review", "Blocked"); null reads the state's word. */
  label: string | null
  title: string
  summary: string | null
  /** Who raised it, when the title named them ("[Doug] …"). */
  from: string | null
  /** The agent or person on it. */
  who: string | null
  /** A ticket's assignees: empty when nobody is on it, null when the card is no ticket
   *  or its board has not been read. On a ticket this, not `who`, is the owner. */
  assignees: readonly InboxPerson[] | null
  /** Board, channel or space. */
  where: string | null
  ref: string | null
  /** ISO of the last movement. */
  at: string
  /** Where a running clock counts from. */
  startedAt: string | null
  href: string | null
  taskId: string | null
  /** Oldest first. */
  steps: InboxStep[]
  line?: BriefLine
}

export const LANE_OF: Record<InboxState, InboxLane> = {
  waiting: 'needs',
  failed: 'needs',
  running: 'running',
  done: 'done',
}

export const LANES: ReadonlyArray<{ id: InboxLane; label: string; empty: string; hasStatusColumn: boolean; timeHeader: string }> = [
  { id: 'needs', label: 'Needs you', empty: 'Decisions that need you appear here', hasStatusColumn: true, timeHeader: 'Age' },
  { id: 'running', label: 'Running', empty: 'Agent work appears here', hasStatusColumn: false, timeHeader: 'Elapsed' },
  { id: 'done', label: 'Done today', empty: 'Finished work appears here', hasStatusColumn: false, timeHeader: 'Finished' },
]

export const STATE_WORD: Record<InboxState, string> = {
  waiting: 'Waiting',
  failed: 'Failed',
  running: 'Running',
  done: 'Done',
}

export const NEED_WORD: Record<InboxNeed, string> = {
  blocked: 'Blocked',
  signoff: 'Awaiting sign-off',
  triage: 'Triage',
}

export const GLYPH_WORD: Record<InboxGlyph, string> = { ...STATE_WORD, ...NEED_WORD }

export const glyphOf = (card: Pick<InboxCard, 'state' | 'need'>): InboxGlyph =>
  card.state === 'waiting' && card.need ? card.need : card.state

/** Who a card belongs to, the same in its footer, the list's Owner column and the drawer:
 *  a ticket's assignees (none is "No owner"), else the agent on it, which `who` always
 *  is. Who raised it is never the owner; null is an owner nobody has read yet. */
export const ownersOf = (card: Pick<InboxCard, 'assignees' | 'who'>): readonly InboxPerson[] | null =>
  card.assignees ?? (card.who ? [{ name: card.who, agent: true }] : null)

const BLOCKER_LABEL = { blocked: "Why it's blocked", failed: 'Why it failed' } as const

export function blockerOf(card: Pick<InboxCard, 'state' | 'need' | 'summary'>): { glyph: InboxGlyph; label: string; text: string | null } | null {
  const glyph = glyphOf(card)
  return glyph === 'blocked' || glyph === 'failed' ? { glyph, label: BLOCKER_LABEL[glyph], text: card.summary } : null
}

const OPEN_LABEL: Record<InboxSource, string> = {
  decision: 'Open',
  message: 'Open conversation',
  ticket: 'Open ticket',
  session: 'Open session',
  research: 'Open run',
  doc: 'Open doc',
}

export const openLabel = (card: Pick<InboxCard, 'source' | 'taskId'>): string => OPEN_LABEL[card.taskId ? 'ticket' : card.source]

export const isMachineText = (label: string): boolean => /\b(fail(ure|ed)?|error|log|trace|output|command)\b/i.test(label)

const KIND_WORD: Record<InboxSource, string | null> = {
  decision: null,
  message: null,
  ticket: null,
  session: 'Run',
  research: 'Research',
  doc: 'Doc',
}

export interface CardRef {
  kind: string | null
  ref: string | null
}

export const refOf = (card: Pick<InboxCard, 'source' | 'ref'>): CardRef => ({ kind: KIND_WORD[card.source], ref: card.ref })

export const approvalOf = (card: Pick<InboxCard, 'line'>): BriefEntry | null => {
  const line = card.line
  return line && !line.resolved && line.current.sourceType === 'approval' && line.current.sourceId ? line.current : null
}

export type CardAction = 'check' | 'approve' | 'reject' | 'restore' | 'dismiss' | 'ask' | 'open'

/** A card's actions as both surfaces draw them (the side drawer and full screen). `bar` is
 *  the header's row in its one order, one of them `primary`; `menu` sits behind the ⋯ that
 *  ends the row. */
export interface CardActionSet {
  bar: Array<{ id: CardAction; primary: boolean }>
  menu: CardAction[]
}

export function cardActions(card: Pick<InboxCard, 'line' | 'state' | 'need' | 'summary'>, can: { ask: boolean; open: boolean }): CardActionSet {
  const line = card.line
  const ask = !!line && can.ask
  const decide: CardAction[] = !line
    ? []
    : line.resolved
      ? ['restore']
      : approvalOf(card)
        ? ['approve', 'reject']
        : ask && blockerOf(card)
          ? ['ask', 'check']
          : ['check']
  const ids = [...decide, ...(ask && !decide.includes('ask') ? (['ask'] as const) : []), ...(can.open ? (['open'] as const) : [])]
  const lead = line && !line.resolved ? ids[0] : null
  return { bar: ids.map((id) => ({ id, primary: id === lead })), menu: decide.includes('check') ? ['dismiss'] : [] }
}

const NEEDS_BY_URGENCY: readonly InboxGlyph[] = ['failed', 'blocked', 'signoff', 'triage', 'waiting']

export function needsBreakdown(cards: readonly InboxCard[]): Array<{ glyph: InboxGlyph; word: string; count: number }> {
  return NEEDS_BY_URGENCY.flatMap((glyph) => {
    const count = cards.filter((c) => LANE_OF[c.state] === 'needs' && glyphOf(c) === glyph).length
    return count > 0 ? [{ glyph, word: GLYPH_WORD[glyph], count }] : []
  })
}

export function laneTally(lanes: Record<InboxLane, readonly unknown[]>, { withZeros = false } = {}): string | null {
  const parts = [[lanes.running.length, 'running'] as const, [lanes.done.length, 'done today'] as const]
  return (
    parts
      .filter(([n]) => withZeros || n > 0)
      .map(([n, word]) => `${n} ${word}`)
      .join(' · ') || null
  )
}

export const LANE_CAP = 5
export type InboxLayout = 'board' | 'list'

export interface InboxInput {
  now: number
  brief: BriefView | null
  queues: HomeSummary['queues'] | null
  sessions: readonly Conversation[] | null
  runs: readonly ResearchRun[] | null
  artifacts: readonly Artifact[] | null
  /** An agent's label, or null for an id that is not an agent (a person, say). */
  agentName: (id: string) => string | null
  /** A ticket's assignees, or null while its board is unread. */
  assignees: (taskId: string) => readonly InboxPerson[] | null
}

type Adapter = (input: InboxInput) => InboxCard[]

const day = (ms: number): string => new Date(ms).toDateString()
const isToday = (iso: string | null, now: number): boolean => !!iso && day(Date.parse(iso)) === day(now)

function firstPartInSentenceCase(label: string | null): string | null {
  const head = label?.split('·')[0]?.trim()
  return head ? head.charAt(0) + head.slice(1).toLowerCase() : null
}

const NEED_WORDS: ReadonlyArray<[RegExp, InboxNeed]> = [
  [/block/i, 'blocked'],
  [/review|sign-?off|approv/i, 'signoff'],
  [/triage|unassigned/i, 'triage'],
]

const needOf = (...words: Array<string | null | undefined>): InboxNeed | null => {
  const text = words.filter(Boolean).join(' ')
  return NEED_WORDS.find(([re]) => re.test(text))?.[1] ?? null
}

export function unwrapQuestion(title: string): string {
  const quoted = /^[^"“”]*["“](.+)["”][^"“”]*\?$/.exec(title)
  return quoted?.[1]?.trim() || title
}

export function splitReporter(title: string): { title: string; from: string | null } {
  const tagged = /^\[([^\]]{1,40})\]\s+(.+)$/.exec(title)
  return tagged ? { title: tagged[2]!, from: tagged[1]!.trim() } : { title, from: null }
}

const STOCK_ADVICE = new Set([
  'Review the evidence, then approve it or request changes.',
  'Open the task, inspect the failure, and decide whether to retry or reassign it.',
  'Open the task and resolve its dependency or reassign the next step.',
  'Open the task and assign an owner or move it into the right workflow stage.',
])

const lineSummary = (e: BriefEntry): string | null => {
  const body = e.body.trim()
  if (body && !STOCK_ADVICE.has(body)) return body
  return e.evidence[0]?.text.trim() || null
}

const lines = (brief: BriefView | null, section: string): BriefLine[] =>
  brief?.sections.find((s) => s.section === section)?.lines ?? []

const STEP_OF: Record<BriefEntry['kind'], (e: BriefEntry) => string> = {
  lede: (e) => e.title,
  item: () => 'Raised in your brief',
  change: () => 'Changed',
  resolved: (e) => firstPartInSentenceCase(e.statusLabel) ?? 'Done',
  checked: () => 'You checked it off',
  dismissed: () => 'You dismissed it',
  note: (e) => e.title,
}

const historySteps = (line: BriefLine): InboxStep[] =>
  line.history.map((e) => ({ label: STEP_OF[e.kind](e), at: e.createdAt }))

const briefTaskIds = (brief: BriefView | null): Set<string> =>
  new Set(
    lines(brief, 'action')
      .map((l) => l.current)
      .filter((e) => e.sourceType === 'task' && e.sourceId)
      .map((e) => e.sourceId!),
  )

const queueItems = (queues: InboxInput['queues']): Array<WorkItem & { queue: string }> =>
  queues
    ? (['triage', 'review', 'blocked'] as const).flatMap((q) => queues[q].items.map((item) => ({ ...item, queue: q })))
    : []

const decisions: Adapter = ({ brief, queues, assignees }) => {
  const tickets = new Map(queueItems(queues).map((t) => [t.id, t]))
  return lines(brief, 'action').map((line) => {
    const e = line.current
    const taskId = e.sourceType === 'task' ? e.sourceId : null
    const ticket = taskId ? tickets.get(taskId) : undefined
    const state: InboxState = line.resolved ? 'done' : e.statusLabel === 'FAILED' ? 'failed' : 'waiting'
    const need = state === 'waiting' ? needOf(e.badge?.label, e.statusLabel) : null
    return {
      key: `brief:${line.key}`,
      source: 'decision',
      state,
      need,
      label: need ? NEED_WORD[need] : firstPartInSentenceCase(e.statusLabel),
      title: ticket?.title ?? unwrapQuestion(e.title),
      summary: line.resolved ? null : lineSummary(e),
      from: null,
      who: null,
      assignees: taskId ? assignees(taskId) : null,
      where: ticket?.board ?? null,
      ref: ticket?.ticketRef ?? null,
      at: e.createdAt,
      startedAt: line.history[0]?.createdAt ?? null,
      href: e.sourceHref && e.sourceHref !== '/' ? e.sourceHref : null,
      taskId,
      steps: historySteps(line),
      line,
    }
  })
}

const answered: Adapter = ({ brief }) =>
  lines(brief, 'comms')
    .filter((line) => line.resolved)
    .map((line) => ({
      key: `brief:${line.key}`,
      source: 'message',
      state: 'done',
      need: null,
      label: null,
      title: line.current.title,
      summary: null,
      from: null,
      who: null,
      assignees: null,
      where: null,
      ref: null,
      at: line.current.createdAt,
      startedAt: null,
      href: line.current.sourceHref,
      taskId: null,
      steps: historySteps(line),
      line,
    }))

const QUEUE_NEED: Record<string, InboxNeed> = { triage: 'triage', review: 'signoff', blocked: 'blocked' }

const tickets: Adapter = ({ brief, queues, assignees }) => {
  const named = briefTaskIds(brief)
  const seen = new Set<string>()
  return queueItems(queues).flatMap((t): InboxCard[] => {
    if (named.has(t.id) || seen.has(t.id)) return []
    seen.add(t.id)
    const need = QUEUE_NEED[t.queue] ?? null
    return [
      {
        key: `ticket:${t.id}`,
        source: 'ticket',
        state: 'waiting',
        need,
        label: need && NEED_WORD[need],
        title: t.title,
        summary: null,
        from: null,
        who: null,
        assignees: assignees(t.id),
        where: t.board,
        ref: t.ticketRef,
        at: t.updatedAt,
        startedAt: null,
        href: `/boards/${t.boardId}/${t.id}`,
        taskId: t.id,
        steps: [],
      },
    ]
  })
}

const sessions: Adapter = ({ sessions, runs, now, agentName }) => {
  const runThreads = new Set((runs ?? []).map((r) => r.conversationId).filter(Boolean))
  return (sessions ?? []).flatMap((c): InboxCard[] => {
    if (runThreads.has(c.id)) return []
    const state: InboxState | null = c.working ? 'running' : c.failed ? 'failed' : isToday(c.updatedAt, now) ? 'done' : null
    if (!state) return []
    return [
      {
        key: `session:${c.id}`,
        source: 'session',
        state,
        need: null,
        label: null,
        title: c.title || 'Untitled session',
        summary: state === 'failed' ? 'The last turn failed. Open it to run it again.' : null,
        from: null,
        who: agentName(c.agentModel) ?? c.agentModel,
        assignees: null,
        where: c.role === 'collaborator' && c.ownerLabel ? `Shared by ${c.ownerLabel}` : null,
        ref: null,
        at: c.updatedAt,
        startedAt: c.working ? c.updatedAt : null,
        href: `/work/${c.id}`,
        taskId: null,
        steps: [{ label: state === 'running' ? 'Working on a reply' : state === 'failed' ? 'Last turn failed' : 'Last reply', at: c.updatedAt }],
      },
    ]
  })
}

const RUN_STATE: Record<ResearchRun['status'], InboxState> = {
  queued: 'running',
  running: 'running',
  awaiting: 'waiting',
  error: 'failed',
  done: 'done',
}

const research: Adapter = ({ runs, now, agentName }) =>
  (runs ?? []).flatMap((r): InboxCard[] => {
    const state = RUN_STATE[r.status]
    if (state === 'done' && !isToday(r.completedAt, now)) return []
    const steps: InboxStep[] = [{ label: 'Started', at: r.createdAt }]
    if (r.phase && state === 'running') steps.push({ label: firstPartInSentenceCase(r.phase) ?? r.phase, at: null })
    if (r.awaiting) steps.push({ label: r.awaiting.question, at: null })
    if (state === 'failed') steps.push({ label: 'Failed', at: r.completedAt })
    if (state === 'done') steps.push({ label: 'Report ready', at: r.completedAt })
    return [
      {
        key: `run:${r.id}`,
        source: 'research',
        state,
        need: null,
        label: r.status === 'queued' ? 'Queued' : r.status === 'awaiting' ? 'Question' : null,
        title: r.title || r.question,
        summary: r.awaiting?.question ?? r.error ?? (r.title ? r.question : null),
        from: null,
        who: agentName(r.agentModel) ?? r.agentModel,
        assignees: null,
        where: 'Research',
        ref: null,
        at: r.completedAt ?? r.createdAt,
        startedAt: r.createdAt,
        href: `/research/${r.id}`,
        taskId: null,
        steps,
      },
    ]
  })

const docs: Adapter = ({ artifacts, brief, runs, now, agentName }) => {
  const reports = new Set((runs ?? []).map((r) => r.artifactId))
  return (artifacts ?? [])
    .filter((a) => isToday(a.updatedAt, now) && a.kind !== 'run-transcript' && a.id !== brief?.artifactId && !reports.has(a.id))
    .map((a): InboxCard => {
      const created = isToday(a.createdAt, now)
      return {
        key: `doc:${a.id}`,
        source: 'doc',
        state: 'done',
        need: null,
        label: null,
        title: a.title || 'Untitled',
        summary: null,
        from: null,
        who: a.createdBy ? agentName(a.createdBy) : null,
        assignees: null,
        where: 'Files',
        ref: null,
        at: a.updatedAt,
        startedAt: null,
        href: `/artifacts?a=${encodeURIComponent(a.id)}`,
        taskId: null,
        steps: created ? [{ label: 'Created', at: a.createdAt }, { label: 'Updated', at: a.updatedAt }] : [{ label: 'Updated', at: a.updatedAt }],
      }
    })
}

export const ADAPTERS: Record<InboxSource, Adapter> = {
  decision: decisions,
  message: answered,
  ticket: tickets,
  session: sessions,
  research,
  doc: docs,
}

const withReporter = (card: InboxCard): InboxCard => {
  const { title, from } = splitReporter(card.title)
  return from ? { ...card, title, from: card.from ?? from } : card
}

const LEADING_REF = /^([A-Z][A-Z0-9]{1,9}-\d+)\s*[:–—-]\s+(\S.*)$/

export function splitRef(title: string): { title: string; ref: string | null } {
  const led = LEADING_REF.exec(title)
  if (!led) return { title, ref: null }
  const rest = led[2]!
  return { title: /^[a-z]+(?![\w-])/.test(rest) ? rest.charAt(0).toUpperCase() + rest.slice(1) : rest, ref: led[1]! }
}

const withRef = (card: InboxCard): InboxCard => {
  const { title, ref } = splitRef(card.title)
  return ref && (!card.ref || card.ref === ref) ? { ...card, title, ref } : card
}

export function buildInboxCards(input: InboxInput): InboxCard[] {
  return Object.values(ADAPTERS)
    .flatMap((adapt) => adapt(input))
    .map(withReporter)
    .map(withRef)
    .sort((a, b) => Date.parse(b.at) - Date.parse(a.at))
}

export function laneCards(cards: readonly InboxCard[]): Record<InboxLane, InboxCard[]> {
  const lanes: Record<InboxLane, InboxCard[]> = { needs: [], running: [], done: [] }
  for (const card of cards) lanes[LANE_OF[card.state]].push(card)
  return lanes
}

export const shownCards = (cards: readonly InboxCard[], showAll: boolean): readonly InboxCard[] => (showAll ? cards : cards.slice(0, LANE_CAP))

export interface InboxMessage {
  line: BriefLine
  comms: CommsState | null
  from: string
  /** Where they asked, one token and never blank: "#helpdesk", "DM". */
  source: string
  text: string
  at: string
  href: string | null
}

const SENDER: ReadonlyArray<RegExp> = [/^(.+?) is waiting on you$/, /^(.+?) asked in (#\S+)$/, /^(.+?) needs an? .+$/]

export function sender(title: string): { name: string; channel: string | null } {
  for (const re of SENDER) {
    const m = re.exec(title.trim())
    if (m) return { name: m[1]!, channel: m[2] ?? null }
  }
  return { name: title.trim(), channel: null }
}

export function messageSource(sourceType: string | null, channel: string | null): string {
  if (channel) return channel
  if (sourceType === 'channel') return 'DM'
  const kind = sourceType?.split(/[^a-z]+/i).find(Boolean)
  return kind ? kind.charAt(0).toUpperCase() + kind.slice(1).toLowerCase() : 'Message'
}

export function todayMessages(brief: BriefView | null): InboxMessage[] {
  return lines(brief, 'comms')
    .filter((line) => !line.resolved)
    .map((line) => {
      const who = sender(line.current.title)
      return {
        line,
        comms: brief?.comms.find((c) => c.sourceKey === line.key) ?? null,
        from: who.name,
        source: messageSource(line.current.sourceType, who.channel),
        text: line.current.body,
        at: line.current.createdAt,
        href: line.current.sourceHref,
      }
    })
    .sort((a, b) => Date.parse(b.at) - Date.parse(a.at))
}

export const todayNotes = (brief: BriefView | null): BriefLine[] =>
  [...lines(brief, 'schedule'), ...lines(brief, 'highlights')].filter((line) => !line.resolved)

function clockMinutes(label: string): number | null {
  const m = /^(\d{1,2}):(\d{2})\s*([AaPp][Mm])?/.exec(label.trim())
  if (!m) return null
  const h = Number(m[1]) % (m[3] ? 12 : 24)
  return (m[3]?.toLowerCase() === 'pm' ? h + 12 : h) * 60 + Number(m[2])
}

const plural = (n: number, word: string): string => `${n} ${word}${n === 1 ? '' : 's'}`

export function alsoToday(notes: readonly BriefLine[], minutesNow: number): { next: string | null; more: string | null } {
  const meetings = notes.filter((n) => n.section === 'schedule')
  const others = notes.length - meetings.length
  const next = meetings.find((n) => {
    const at = clockMinutes(n.current.statusLabel ?? '')
    return at !== null && at >= minutesNow
  })
  const left = meetings.length - (next ? 1 : 0)
  const more = [left > 0 && `${next ? '+' : ''}${plural(left, 'meeting')}`, others > 0 && plural(others, 'note')].filter(Boolean).join(' · ')
  return { next: next ? `${next.current.statusLabel!.trim()} ${next.current.title}` : null, more: more || null }
}

export const sentences = (text: string): string[] =>
  text
    .trim()
    .split(/(?<=[.!?])\s+(?=["“(]?[A-Z0-9])/)
    .filter(Boolean)

const CLAUSE_END = /\s+\(|[:;](?=\s)|\s*—/g

const count = (text: string, mark: string): number => text.split(mark).length - 1

const balanced = (text: string): boolean =>
  count(text, '**') % 2 === 0 && count(text, '`') % 2 === 0 && count(text, '(') === count(text, ')') && count(text, '[') === count(text, ']')

export function ledeStops(lede: string): string[] {
  const parts = sentences(lede)
  const first = parts[0]
  if (!first) return []
  const whole = parts.map((_, i) => parts.slice(0, parts.length - i).join(' '))
  const clauses = [...first.matchAll(CLAUSE_END)]
    .map((m) => first.slice(0, m.index).trimEnd())
    .filter((head) => head.length > 0 && balanced(head))
    .map((head) => `${head.replace(/[,.!?]+$/, '')}.`)
    .reverse()
  return [...whole, ...clauses]
}

const pad = (n: number): string => String(n).padStart(2, '0')

export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${pad(s % 60)}s`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ${pad(m % 60)}m`
  return `${Math.floor(h / 24)}d ${h % 24}h`
}

export function ago(ms: number): string {
  const m = Math.floor(Math.max(0, ms) / 60_000)
  if (m < 1) return 'just now'
  if (m < 60) return `${m}m ago`
  const h = Math.floor(m / 60)
  return h < 24 ? `${h}h ago` : `${Math.floor(h / 24)}d ago`
}

const wordOf = (card: InboxCard): string => (card.state === 'done' ? STATE_WORD.done : (card.label ?? STATE_WORD[card.state]))

const LANE_SAYS: Partial<Record<InboxLane, string>> = { running: STATE_WORD.running, done: STATE_WORD.done }

export interface CardMeta {
  word: string
  statusPrefix: string | null
  time: string
  timeIsDuration: boolean
  ref: CardRef
  owners: readonly InboxPerson[]
}

export function cardMeta(card: InboxCard, now: number): CardMeta {
  const timeIsDuration = card.state === 'running'
  const word = wordOf(card)
  return {
    word,
    statusPrefix: word === LANE_SAYS[LANE_OF[card.state]] ? null : word,
    time: timeIsDuration ? clock(now - Date.parse(card.startedAt ?? card.at)) : ago(now - Date.parse(card.at)),
    timeIsDuration,
    ref: refOf(card),
    owners: ownersOf(card) ?? [],
  }
}

export const statusText = (meta: Pick<CardMeta, 'word' | 'time' | 'timeIsDuration'>): string => (meta.timeIsDuration ? `${meta.word} · ${meta.time}` : meta.word)

export const TICKET_STATUS: Record<TaskStatus, { glyph: InboxGlyph; word: string }> = {
  inbox: { glyph: 'triage', word: NEED_WORD.triage },
  assigned: { glyph: 'running', word: 'Queued' },
  in_progress: { glyph: 'running', word: STATE_WORD.running },
  blocked: { glyph: 'blocked', word: NEED_WORD.blocked },
  quality_review: { glyph: 'signoff', word: NEED_WORD.signoff },
  done: { glyph: 'done', word: STATE_WORD.done },
  failed: { glyph: 'failed', word: STATE_WORD.failed },
  cancelled: { glyph: 'triage', word: 'Cancelled' },
}

export const ticketStatus = (status: string): { glyph: InboxGlyph; word: string } =>
  TICKET_STATUS[status as TaskStatus] ?? { glyph: 'triage', word: status.charAt(0).toUpperCase() + status.slice(1).replace(/_/g, ' ') }
