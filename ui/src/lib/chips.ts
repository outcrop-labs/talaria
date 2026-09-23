import { postJson } from '@/lib/fetch-json'

// Chat chips — the one contract for a platform link, an exposed tool, a
// protected-action approval, and the tools that approval unlocked.
//
// Recognition is path-shaped, not host-shaped: an agent pastes either
// `/boards/…` or `https://<instance>/boards/…` and both are the same chip.
// Anything that does not match stays a normal link. Titles that the server
// already knows ride on the chip; titles for prose links are resolved in a
// batch so a chip never has to show the raw URL to earn its label.

export type ChipEntity = 'board' | 'ticket' | 'kb' | 'document' | 'channel' | 'plan' | 'research'

export type ChipKind = 'link' | 'tool' | 'approval' | 'unlock'

export interface PlatformLink {
  /** App path, origin stripped, no trailing slash. */
  href: string
  entity: ChipEntity
  id: string
  boardId?: string
}

export interface ChipField {
  name: string
  label: string
  type: 'string' | 'text'
  required?: boolean
}

export interface ChatChip {
  id: string
  kind: ChipKind
  href?: string
  entity?: ChipEntity
  title?: string
  tool?: string
  label?: string
  description?: string
  inputs?: Record<string, unknown>
  fields?: ChipField[]
  actionId?: string
  actionKind?: string
  summary?: string
  status?: 'pending' | 'approved' | 'denied'
  tools?: string[]
  approvalId?: string
}

const ID = '[A-Za-z0-9_-]{1,80}'

/** Kind label when a title has not resolved. Never the raw URL. */
export function entityLabel(entity: ChipEntity): string {
  switch (entity) {
    case 'board':
      return 'Board'
    case 'ticket':
      return 'Ticket'
    case 'kb':
      return 'Knowledge'
    case 'document':
      return 'Document'
    case 'channel':
      return 'Channel'
    case 'plan':
      return 'Plan'
    case 'research':
      return 'Research'
  }
}

/** Tools a protected-action approval unlocks. Kept in lockstep with
 *  `tools_unlocked` in api/crates/talaria-chips. */
export function toolsUnlockedBy(kind: string): string[] {
  switch (kind) {
    case 'gmail_send':
      return ['draft_email']
    case 'calendar_create':
      return ['draft_calendar_event']
    case 'ticket_move':
      return ['triage_ticket']
    case 'board_access':
      return ['join_board']
    default:
      return []
  }
}

/** The inputs a known platform tool asks for. Unknown tools still open —
 *  they show whatever inputs the chip already carried. */
export const TOOL_FIELDS: Record<string, { label: string; description: string; fields: ChipField[] }> = {
  create_ticket: {
    label: 'Create ticket',
    description: 'File a ticket on a board. It lands in the inbox for a person to assign.',
    fields: [
      { name: 'boardId', label: 'Board', type: 'string', required: true },
      { name: 'title', label: 'Title', type: 'string', required: true },
      { name: 'description', label: 'Description', type: 'text' },
    ],
  },
  triage_ticket: {
    label: 'Move ticket',
    description: 'Move a ticket forward, or change its priority, labels, or description.',
    fields: [
      { name: 'taskId', label: 'Ticket', type: 'string', required: true },
      { name: 'status', label: 'Status', type: 'string' },
      { name: 'priority', label: 'Priority', type: 'string' },
    ],
  },
  comment: {
    label: 'Comment',
    description: 'Leave a comment on a ticket.',
    fields: [
      { name: 'taskId', label: 'Ticket', type: 'string', required: true },
      { name: 'body', label: 'Comment', type: 'text', required: true },
    ],
  },
  draft_email: {
    label: 'Draft email',
    description: 'Draft an email. Sending waits for approval unless this conversation already unlocked it.',
    fields: [
      { name: 'to', label: 'To', type: 'string', required: true },
      { name: 'subject', label: 'Subject', type: 'string' },
      { name: 'body', label: 'Body', type: 'text' },
    ],
  },
  draft_calendar_event: {
    label: 'Draft event',
    description: 'Draft a calendar event. Creating it waits for approval unless this conversation already unlocked it.',
    fields: [
      { name: 'summary', label: 'Title', type: 'string', required: true },
      { name: 'start', label: 'Start', type: 'string', required: true },
      { name: 'end', label: 'End', type: 'string', required: true },
    ],
  },
  create_document: {
    label: 'Create document',
    description: 'Create a document in Files.',
    fields: [
      { name: 'title', label: 'Title', type: 'string', required: true },
      { name: 'content', label: 'Body', type: 'text' },
    ],
  },
  create_kb_doc: {
    label: 'Create knowledge doc',
    description: 'Create a knowledge base document.',
    fields: [
      { name: 'spaceId', label: 'Space', type: 'string', required: true },
      { name: 'title', label: 'Title', type: 'string', required: true },
      { name: 'body', label: 'Body', type: 'text' },
    ],
  },
  post_to_channel: {
    label: 'Post to channel',
    description: 'Post a message into a channel.',
    fields: [
      { name: 'channelId', label: 'Channel', type: 'string', required: true },
      { name: 'content', label: 'Message', type: 'text', required: true },
    ],
  },
  search_knowledge: {
    label: 'Search knowledge',
    description: 'Search the knowledge base.',
    fields: [{ name: 'query', label: 'Query', type: 'string', required: true }],
  },
  join_board: {
    label: 'Join board',
    description: 'Join a board the owner can already read.',
    fields: [{ name: 'boardId', label: 'Board', type: 'string', required: true }],
  },
}

export function toolFields(name: string): ChipField[] {
  return TOOL_FIELDS[name]?.fields ?? []
}

export function toolLabel(name: string): string {
  return TOOL_FIELDS[name]?.label ?? name.replaceAll('_', ' ')
}

export function toolDescription(name: string): string {
  return TOOL_FIELDS[name]?.description ?? 'Platform tool'
}

function isId(s: string | undefined): s is string {
  return !!s && new RegExp(`^${ID}$`).test(s)
}

/** Turn a pasted URL or app path into a platform link, or null when it is
 *  not one of ours. Query-only selections (`?doc=`, `?a=`, `?c=`, `?p=`,
 *  `?r=`) count; a bare index path does not. */
export function classifyPlatformHref(raw: string): PlatformLink | null {
  const trimmed = raw.trim()
  if (!trimmed || trimmed.startsWith('mention:') || /^javascript:/i.test(trimmed)) return null
  let path = trimmed
  if (/^https?:\/\//i.test(trimmed)) {
    try {
      const u = new URL(trimmed)
      path = u.pathname + u.search
    } catch {
      return null
    }
  }
  if (!path.startsWith('/')) return null
  const q = path.indexOf('?')
  const pathname = (q === -1 ? path : path.slice(0, q)).replace(/\/+$/, '') || '/'
  const params = new URLSearchParams(q === -1 ? '' : path.slice(q + 1))
  const seg = pathname.split('/').filter(Boolean)

  // /boards/:boardId/:taskId is three segments. Two is the board itself.
  if (seg[0] === 'boards' && seg.length === 3 && isId(seg[1]) && isId(seg[2])) {
    return { href: `/boards/${seg[1]}/${seg[2]}`, entity: 'ticket', id: seg[2], boardId: seg[1] }
  }
  if (seg[0] === 'boards' && seg.length === 2 && isId(seg[1])) {
    return { href: `/boards/${seg[1]}`, entity: 'board', id: seg[1] }
  }
  if (seg[0] === 'knowledge') {
    const doc = params.get('doc') || params.get('d')
    if (doc && isId(doc)) return { href: `/knowledge?doc=${encodeURIComponent(doc)}`, entity: 'kb', id: doc }
    if (seg.length === 3 && isId(seg[1]) && isId(seg[2])) {
      return { href: `/knowledge/${seg[1]}/${seg[2]}`, entity: 'kb', id: seg[2] }
    }
  }
  if (seg[0] === 'artifacts') {
    const id = params.get('a')
    if (id && isId(id)) return { href: `/artifacts?a=${encodeURIComponent(id)}`, entity: 'document', id }
  }
  if (seg[0] === 'comms' && seg[1] === 'channel' && seg.length === 3 && isId(seg[2])) {
    return { href: `/comms/channel/${seg[2]}`, entity: 'channel', id: seg[2] }
  }
  if (seg[0] === 'comms' && seg.length === 1) {
    const id = params.get('c')
    if (id && isId(id)) return { href: `/comms/channel/${id}`, entity: 'channel', id }
  }
  if (seg[0] === 'plan') {
    const id = seg.length === 2 && isId(seg[1]) ? seg[1] : params.get('p')
    if (id && isId(id)) return { href: `/plan/${id}`, entity: 'plan', id }
  }
  if (seg[0] === 'research') {
    const id = seg.length === 2 && isId(seg[1]) ? seg[1] : params.get('r')
    if (id && isId(id)) return { href: `/research/${id}`, entity: 'research', id }
  }
  return null
}

const BARE =
  /(?:https?:\/\/[^\s<>)\]]+)?\/(?:boards\/[A-Za-z0-9_-]+(?:\/[A-Za-z0-9_-]+)?|knowledge(?:\/[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+|\?[^\s<>)\]]*doc=[A-Za-z0-9_%-]+)|artifacts\?[^\s<>)\]]*a=[A-Za-z0-9_%-]+|comms\/channel\/[A-Za-z0-9_-]+|plan\/[A-Za-z0-9_-]+|research\/[A-Za-z0-9_-]+)/gi

/** Bare platform paths and absolute URLs in prose, in source order. A match
 *  that does not classify is dropped — the regex is a candidate finder, not
 *  the recognizer. */
export function findPlatformLinks(text: string): Array<{ start: number; end: number; link: PlatformLink }> {
  const out: Array<{ start: number; end: number; link: PlatformLink }> = []
  for (const m of text.matchAll(BARE)) {
    const start = m.index ?? 0
    const before = start === 0 ? '' : text[start - 1]
    if (before && !/[\s([>]/.test(before)) continue
    const raw = m[0]
    const link = classifyPlatformHref(raw)
    if (!link) continue
    out.push({ start, end: start + raw.length, link })
  }
  return out
}

/** Link chips already painted from the prose stay out of the row, so a URL
 *  the agent wrote does not appear twice. Tool, approval, and unlock chips
 *  always stay — they are not in the prose. */
export function chipsBesideProse(chips: ChatChip[] | undefined, content: string): ChatChip[] {
  if (!chips?.length) return []
  return chips.filter((c) => c.kind !== 'link' || !c.href || !content.includes(c.href))
}

const titleCache = new Map<string, string>()

/** Batch-resolve titles for prose chips. A miss caches the empty string so a
 *  forbidden or unknown id does not refetch on every render; the chip then
 *  shows the entity label, not the URL and not someone else's title. */
export async function resolveChipTitles(hrefs: string[]): Promise<Record<string, string>> {
  const missing = [...new Set(hrefs.filter((h) => h && !titleCache.has(h)))]
  if (missing.length) {
    try {
      const body = await postJson<{ chips?: Array<{ href: string; title?: string }> }>('/api/chat/chips/resolve', {
        hrefs: missing,
      })
      for (const row of body.chips ?? []) titleCache.set(row.href, row.title ?? '')
    } catch {
      /* offline or forbidden — labels stand in */
    }
    for (const h of missing) if (!titleCache.has(h)) titleCache.set(h, '')
  }
  return Object.fromEntries(hrefs.map((h) => [h, titleCache.get(h) ?? '']))
}

export function chipTitle(chip: { entity?: ChipEntity; title?: string }): string {
  const title = chip.title?.trim()
  if (title) return title
  return chip.entity ? entityLabel(chip.entity) : 'Link'
}

/** The sentence a Run click sends into the thread. The agent already holds
 *  the tool; the user is handing it the inputs. */
export function invokeText(tool: string, inputs: Record<string, string>): string {
  const filled = Object.fromEntries(Object.entries(inputs).filter(([, v]) => v.trim()))
  return `Run ${tool} with these inputs:\n\`\`\`json\n${JSON.stringify(filled, null, 2)}\n\`\`\``
}

export async function decideChip(
  actionId: string,
  decision: 'approve' | 'deny',
): Promise<{ ok: boolean; error?: string; tools?: string[] }> {
  try {
    const body = await postJson<{ tools?: string[]; status?: string }>(
      `/api/chat/chips/approvals/${encodeURIComponent(actionId)}`,
      { decision: decision === 'deny' ? 'reject' : 'approve' },
    )
    return { ok: true, tools: body.tools }
  } catch (e) {
    const message = e instanceof Error ? e.message : ''
    return { ok: false, error: message || 'That did not go through.' }
  }
}
