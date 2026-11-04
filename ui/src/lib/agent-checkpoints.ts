export type CheckpointStatus = 'running' | 'done' | 'failed' | 'stopped'

export type ToolKind = 'turn' | 'search' | 'read' | 'edit' | 'shell' | 'test' | 'browser' | 'git' | 'other'

export interface LedgerRow {
  seq: number
  tool: string | null
  title: string | null
  status: string | null
  createdAt: string
}

export interface Checkpoint {
  id: string
  title: string
  tool: ToolKind
  parent?: string
  status: CheckpointStatus
  at: number
  attempt?: number
  fork?: boolean
}

export const ROW = 30
export const LANE = 14
export const PAD = 10
export const RAIL = 22
export const MAX_ROWS = 14

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

export function fromLedger(rows: LedgerRow[], runState?: string): Checkpoint[] {
  const start = rows[0] ? Date.parse(rows[0].createdAt) : 0
  return rows.map((row, i) => {
    const previous = rows[i - 1]
    const own: CheckpointStatus = row.status === 'failed' ? 'failed' : 'done'
    return {
      id: String(row.seq),
      title: rowTitle(row),
      tool: toolKind(row.tool),
      parent: previous && String(previous.seq),
      status: i === rows.length - 1 ? headStatus(runState, own) : own,
      at: Date.parse(row.createdAt) - start,
    }
  })
}

export interface Placed {
  cp: Checkpoint
  row: number
  lane: number
  from?: number
  fromLane?: number
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

export function layout(checkpoints: Checkpoint[]): Placed[] {
  const byId = new Map<string, Placed>()
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
      from: parent?.row,
      fromLane: parent?.lane,
      attempt: !continues && row > 0 ? cp.attempt : undefined,
    }
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
export const y = (row: number) => row * ROW + ROW / 2

export function path(p: Placed & { from: number; fromLane: number }, pitch = LANE) {
  const x0 = x(p.fromLane, pitch)
  const y0 = y(p.from)
  const x1 = x(p.lane, pitch)
  const y1 = y(p.row)
  if (x0 === x1) return `M${x0} ${y0}V${y1}`
  const bend = y0 + ROW * 0.85
  return `M${x0} ${y0}C${x0} ${y0 + ROW * 0.5} ${x1} ${bend - ROW * 0.45} ${x1} ${bend}V${y1}`
}

const pad2 = (n: number) => String(n).padStart(2, '0')

export function clock(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000))
  if (s >= 3600) return `${Math.floor(s / 3600)}:${pad2(Math.floor((s % 3600) / 60))}:${pad2(s % 60)}`
  return `${Math.floor(s / 60)}:${pad2(s % 60)}`
}

export const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`
