export type CheckpointStatus = 'running' | 'done' | 'failed' | 'stopped'

export type ToolKind = 'turn' | 'search' | 'read' | 'edit' | 'shell' | 'test' | 'browser' | 'git' | 'other'

export interface LedgerRow {
  seq: number
  kind: 'turn' | 'step'
  tool: string | null
  title: string | null
  status: string | null
  restorable: boolean
  unrestorableReason: string | null
  createdAt: string
}

export interface RunAttempt {
  runId: string
  attemptNo: number
  state?: string
  rewoundFromRunId: string | null
  rewoundFromSeq: number | null
  outside?: OutsideSummary
  rows: LedgerRow[]
}

/** One `run-rewind` row. It exists before its attempt does, and a failed one
 *  never gets an attempt at all. */
export interface RewindRow {
  id: string
  state: string
  stage: string | null
  sourceRunId: string | null
  seq: number | null
  attemptRunId: string | null
  error: string | null
}

export type RewindMark = { kind: 'pending'; stage: string | null } | { kind: 'failed'; reason: string }

export type RowAnswer =
  | { kind: 'rewind'; runId: string; seq: number }
  | { kind: 'reason'; reason: string }

export interface Checkpoint {
  id: string
  runId?: string
  title: string
  tool: ToolKind
  parent?: string
  status: CheckpointStatus
  at: number
  attempt?: number
  fork?: boolean
  answer?: RowAnswer
  rewind?: RewindMark
}

export const ROW = 30
export const TOUCH_ROW = 44
export const LANE = 14
export const PAD = 10
export const RAIL = 22
export const MAX_ROWS = 16

const TOOL_WORDS = new Map<string, ToolKind>([
  ['search', 'search'],
  ['grep', 'search'],
  ['find', 'search'],
  ['glob', 'search'],
  ['read', 'read'],
  ['view', 'read'],
  ['write', 'edit'],
  ['edit', 'edit'],
  ['patch', 'edit'],
  ['terminal', 'shell'],
  ['shell', 'shell'],
  ['bash', 'shell'],
  ['exec', 'shell'],
  ['test', 'test'],
  ['tests', 'test'],
  ['browser', 'browser'],
  ['web', 'browser'],
  ['git', 'git'],
  ['commit', 'git'],
  ['push', 'git'],
  ['pr', 'git'],
])

const withoutMcpPrefix = (name: string) => name.split('__').at(-1)!
const toolWords = (name: string) => withoutMcpPrefix(name).split(/[^a-z0-9]+/i)

export function toolKind(name: string | null): ToolKind {
  if (name === null) return 'turn'
  for (const word of toolWords(name)) {
    const kind = TOOL_WORDS.get(word.toLowerCase())
    if (kind) return kind
  }
  return 'other'
}

const sentence = (words: string) => words.charAt(0).toUpperCase() + words.slice(1)

function rowTitle(row: LedgerRow) {
  if (row.title) return sentence(row.title)
  if (row.tool) return sentence(toolWords(row.tool).join(' '))
  return `Checkpoint ${row.seq}`
}

export const runIsLive = (state: string | undefined) =>
  state !== undefined && !['done', 'error', 'cancelled'].includes(state)

function headStatus(runState: string | undefined, own: CheckpointStatus): CheckpointStatus {
  if (runState === 'error') return 'failed'
  if (runState === 'cancelled') return 'stopped'
  return runIsLive(runState) ? 'running' : own
}

const rowId = (runId: string, seq: number) => `${runId}:${seq}`

const REASONS: [RegExp, string][] = [
  [/turn boundary/, 'A turn boundary has no snapshot. Rewind to a step instead.'],
  [/^compacted/, 'The agent compacted its conversation after this step, so the exact conversation here is gone.'],
  [/snapshot budget/, 'The files here were over the snapshot budget, so no snapshot was kept.'],
  [/checkout was removed/, 'The job was torn down, and its files went with it.'],
  [/no workbench job/, 'Talaria knew of no job for this step, so there are no files to restore.'],
  [/no git checkout/, 'The job had no git checkout at this step, so there are no files to restore.'],
  [/^snapshot failed/, 'The snapshot failed at this step, so there are no files to restore.'],
]

export function plainReason(reason: string | null) {
  if (!reason) return 'This step kept no snapshot.'
  return REASONS.find(([pattern]) => pattern.test(reason))?.[1] ?? sentence(reason)
}

function rowAnswer(runId: string, row: LedgerRow): RowAnswer {
  return row.kind === 'step' && row.restorable
    ? { kind: 'rewind', runId, seq: row.seq }
    : { kind: 'reason', reason: plainReason(row.unrestorableReason) }
}

const FAILURES: [RegExp, string][] = [[/container .* is not running/, 'The agent is not running, so it could not rewind. Start it and try again.']]

export const plainFailure = (error: string | null) =>
  error === null
    ? 'The rewind stopped before it finished.'
    : (FAILURES.find(([pattern]) => pattern.test(error))?.[1] ?? sentence(error))

const REWIND_LANDED = 'done'
const REWIND_ENDED = [REWIND_LANDED, 'error', 'cancelled']

const rewindIsLive = (row: RewindRow) => !REWIND_ENDED.includes(row.state)

function rewindMark(row: RewindRow): RewindMark | undefined {
  if (rewindIsLive(row)) return { kind: 'pending', stage: row.stage }
  if (row.state === REWIND_LANDED) return undefined
  return { kind: 'failed', reason: plainFailure(row.error) }
}

/** Newest first, so the first row for a step is the one that speaks for it: a
 *  retry that landed clears the failure before it. */
function rewindMarks(rewinds: RewindRow[]) {
  const marks = new Map<string, RewindMark | undefined>()
  for (const row of rewinds) {
    if (row.sourceRunId === null || row.seq === null) continue
    const id = rowId(row.sourceRunId, row.seq)
    if (!marks.has(id)) marks.set(id, rewindMark(row))
  }
  return marks
}

export const byAttemptNo = (attempts: RunAttempt[]) => attempts.toSorted((a, b) => a.attemptNo - b.attemptNo)

const folded = (rows: LedgerRow[]) =>
  rows.filter((row, i) => !(row.kind === 'turn' && row.status === 'send' && rows[i - 1]?.status === 'record'))

function attemptCheckpoints(
  attempt: RunAttempt,
  numbered: boolean,
  marks: Map<string, RewindMark | undefined>,
): Checkpoint[] {
  const { runId, state } = attempt
  const rows = folded(attempt.rows)
  const start = rows[0] ? Date.parse(rows[0].createdAt) : 0
  const source =
    attempt.rewoundFromRunId !== null && attempt.rewoundFromSeq !== null
      ? rowId(attempt.rewoundFromRunId, attempt.rewoundFromSeq)
      : undefined
  return rows.map((row, i) => {
    const previous = rows[i - 1]
    const own: CheckpointStatus = row.status === 'failed' ? 'failed' : 'done'
    const id = rowId(runId, row.seq)
    return {
      id,
      runId,
      title: rowTitle(row),
      tool: toolKind(row.tool),
      parent: previous ? rowId(runId, previous.seq) : source,
      status: i === rows.length - 1 ? headStatus(state, own) : own,
      at: Date.parse(row.createdAt) - start,
      attempt: numbered ? attempt.attemptNo : undefined,
      fork: !previous && source !== undefined,
      answer: rowAnswer(runId, row),
      rewind: marks.get(id),
    }
  })
}

/** The whole lineage as one list. Attempts follow the server's numbers, and a
 *  rewound attempt hangs off the row its `rewoundFrom` columns name. */
export function fromLineage(attempts: RunAttempt[], rewinds: RewindRow[]): Checkpoint[] {
  const marks = rewindMarks(rewinds)
  const numbered = attempts.length > 1 || attempts.some((a) => a.attemptNo > 1)
  return byAttemptNo(attempts).flatMap((attempt) => attemptCheckpoints(attempt, numbered, marks))
}

export function lineageRewinding(attempts: RunAttempt[], rewinds: RewindRow[]) {
  const runs = new Set(attempts.map((a) => a.runId))
  return rewinds.some((row) => rewindIsLive(row) && row.sourceRunId !== null && runs.has(row.sourceRunId))
}

export const STAGE_WORDS: Record<string, string> = {
  stop: 'Stopping the run',
  quiet: 'Stopping the run',
  fork: 'Copying the conversation',
  cut: 'Copying the conversation',
  restore: 'Restoring the files',
  verify: 'Checking the restore',
  start: 'Starting the attempt',
}

export const rewindStageWords = (stage: string | null) => (stage && STAGE_WORDS[stage]) || 'Checking the step'

export interface OutsideSummary {
  tokens: number
  pushes: { branch: string; at: string }[]
  pullRequests: string[]
}

export interface TicketMove {
  from: string
  to: string
}

function outsideLines(outside: OutsideSummary | undefined) {
  if (!outside) return []
  const branches = [...new Set(outside.pushes.map((p) => p.branch))]
  return [
    `${outside.tokens.toLocaleString('en-US')} ${outside.tokens === 1 ? 'token' : 'tokens'} spent`,
    ...branches.map((branch) => `Pushes to ${branch}`),
    ...outside.pullRequests.map((url) => `Pull request ${url}`),
  ]
}

/** The confirm step, in the order a person has to weigh it. */
export function rewindConfirmation({
  running,
  outside,
  ticketMove,
}: {
  running: boolean
  outside?: OutsideSummary
  ticketMove: TicketMove | null
}) {
  return [
    'A new attempt starts from this step, with the conversation and the files as they were here. The attempt it leaves stays visible.',
    ...(running ? ['This run is working, so it will stop first.'] : []),
    ['These stay changed:', ...outsideLines(outside).map((line) => `• ${line}`), '• Anything else the agent did outside this run, such as comments or messages'].join('\n'),
    'Agent memory is not restored. What the agent saved since this step stays.',
    'The coding harness session starts fresh.',
    ...(ticketMove ? [`The ticket moves from ${ticketMove.from} back to ${ticketMove.to}.`] : []),
  ].join('\n\n')
}

export interface Placed {
  cp: Checkpoint
  row: number
  lane: number
  top: number
  mid: number
  from?: number
  fromLane?: number
  fromMid?: number
  attempt?: number
}

function parentsFirst(checkpoints: Checkpoint[]) {
  const ids = new Set(checkpoints.map((cp) => cp.id))
  const placed = new Set<string>()
  const waiting = new Map<string, Checkpoint[]>()
  const ordered: Checkpoint[] = []
  const place = (cp: Checkpoint) => {
    ordered.push(cp)
    placed.add(cp.id)
    waiting.get(cp.id)?.forEach(place)
  }
  for (const cp of checkpoints) {
    if (cp.parent !== undefined && ids.has(cp.parent) && !placed.has(cp.parent)) {
      waiting.set(cp.parent, [...(waiting.get(cp.parent) ?? []), cp])
    } else {
      place(cp)
    }
  }
  return ordered
}

export function layout(
  checkpoints: Checkpoint[],
  { row: rowHeight = ROW, notes = {} }: { row?: number; notes?: Record<string, number> } = {},
): Placed[] {
  const byId = new Map<string, Placed>()
  let top = 0
  const lastOn: number[] = []
  const taken = new Set<string>()
  const free = (from: number) => {
    let lane = 0
    while ((lastOn[lane] ?? -1) >= from) lane++
    return lane
  }
  return parentsFirst(checkpoints).map((cp, row) => {
    const parent = cp.parent === undefined ? undefined : byId.get(cp.parent)
    const continues = parent && !cp.fork && !taken.has(parent.cp.id) && lastOn[parent.lane] === parent.row
    const lane = continues ? parent.lane : free(parent ? parent.row : row)
    const placed: Placed = {
      cp,
      row,
      lane,
      top,
      mid: top + rowHeight / 2,
      from: parent?.row,
      fromLane: parent?.lane,
      fromMid: parent?.mid,
      attempt: !continues ? cp.attempt : undefined,
    }
    top += rowHeight + (notes[cp.id] ?? 0)
    if (parent) taken.add(parent.cp.id)
    lastOn[lane] = row
    byId.set(cp.id, placed)
    return placed
  })
}

export function summarize(placed: Placed[]) {
  return {
    checkpoints: placed.length,
    attempts: Math.max(1, new Set(placed.flatMap((p) => p.cp.attempt ?? [])).size),
    lanes: Math.max(1, ...placed.map((p) => p.lane + 1)),
  }
}

export function livePath(placed: Placed[]) {
  const rows = new Set<number>()
  for (let p = placed.at(-1); p; p = p.from === undefined ? undefined : placed[p.from]) rows.add(p.row)
  return rows
}

export const x = (lane: number, pitch = LANE) => RAIL + lane * pitch

export function path(p: Placed & { fromLane: number; fromMid: number }, pitch = LANE, rowHeight = ROW) {
  const x0 = x(p.fromLane, pitch)
  const y0 = p.fromMid
  const x1 = x(p.lane, pitch)
  const y1 = p.mid
  if (x0 === x1) return `M${x0} ${y0}V${y1}`
  const bend = y0 + rowHeight * 0.85
  return `M${x0} ${y0}C${x0} ${y0 + rowHeight * 0.5} ${x1} ${bend - rowHeight * 0.45} ${x1} ${bend}V${y1}`
}

const pad2 = (n: number) => String(n).padStart(2, '0')

export function clock(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000))
  if (s >= 3600) return `${Math.floor(s / 3600)}:${pad2(Math.floor((s % 3600) / 60))}:${pad2(s % 60)}`
  return `${Math.floor(s / 60)}:${pad2(s % 60)}`
}

export const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`
