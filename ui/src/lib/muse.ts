// Client for the drafting muse + model preferences.
import { useQuery } from '@tanstack/react-query'
import { getJson } from '@/lib/fetch-json'

export type MuseKind = 'soul' | 'personality' | 'skill' | 'memory' | 'cron' | 'agent' | 'document' | 'template' | 'ticket'

export interface MuseRequest {
  kind: MuseKind
  instruction: string
  current?: string
  context?: string
  chat?: Array<{ role: 'user' | 'assistant'; content: string }>
}

/** Stream a muse draft; onChunk receives text pieces as they arrive.
 *  Returns the full text. Throws with the server's message on failure. */
export async function streamMuse(input: MuseRequest, onChunk: (piece: string) => void, signal?: AbortSignal): Promise<string> {
  const r = await fetch('/api/muse', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(input),
    signal,
  })
  if (!r.ok || !r.body) {
    const j = (await r.json().catch(() => null)) as { error?: string } | null
    throw new Error(j?.error ?? `muse failed (${r.status})`)
  }
  const reader = r.body.getReader()
  const decoder = new TextDecoder()
  let full = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    const piece = decoder.decode(value, { stream: true })
    full += piece
    onChunk(piece)
  }
  return full
}

export interface GatewayModel {
  id: string
  endpoints: string[]
  /** True for "<endpoint>/<model>" pins; false for bare model ids (which may
   *  themselves contain "/", e.g. OpenRouter names). */
  qualified: boolean
  /** Pretty display name from the public catalog, when known. */
  label?: string
  /** One line on what the model is good at, when known. */
  blurb?: string
}

/** The model catalog + the model the caller's muse resolves to right now. */
export function useModels() {
  return useQuery({
    queryKey: ['gateway-models'],
    staleTime: 60_000,
    queryFn: (): Promise<{ models: GatewayModel[]; effective: string | null }> =>
      getJson<{ models: GatewayModel[]; effective: string | null }>('/api/models'),
  })
}

export function usePreferredModel() {
  return useQuery({
    queryKey: ['profile-prefs'],
    // `preferredModel: null` means "no preference set" — a real 200 answer.
    // A failed profile read must not look like a cleared preference.
    queryFn: (): Promise<{ preferredModel: string | null }> =>
      getJson<{ preferredModel: string | null }>('/api/profile'),
  })
}

export async function savePreferredModel(model: string | null): Promise<{ error?: string }> {
  const r = await fetch('/api/profile', {
    method: 'PUT',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ preferredModel: model }),
  })
  if (!r.ok) return ((await r.json().catch(() => ({}))) as { error?: string }) ?? { error: `save failed (${r.status})` }
  return {}
}

/** Parse the muse's cron JSON (tolerates stray fences/prose around it). */
export function parseCronDraft(text: string): { name: string; schedule: string; prompt: string } | null {
  const m = /\{[\s\S]*\}/.exec(text)
  if (!m) return null
  try {
    const j = JSON.parse(m[0]) as { name?: string; schedule?: string; prompt?: string }
    if (!j.name || !j.schedule || !j.prompt) return null
    return { name: String(j.name), schedule: String(j.schedule), prompt: String(j.prompt) }
  } catch {
    return null
  }
}

export interface AgentDraft {
  name: string
  handle: string
  department: string
  role: string
  soul: string
  skills: Array<{ name: string; content: string }>
}

/** Parse + sanitize a drafted agent design: handles/departments are coerced
 *  into their identifier alphabets, skills into kebab names; soul required. */
export function parseAgentDraft(text: string): AgentDraft | null {
  const m = /\{[\s\S]*\}/.exec(text)
  if (!m) return null
  try {
    const j = JSON.parse(m[0]) as Partial<AgentDraft> & { skills?: Array<{ name?: string; content?: string }> }
    if (!j.name || !j.soul) return null
    const ident = (v: string, allowDash: boolean) =>
      v
        .toLowerCase()
        .replace(allowDash ? /[^a-z0-9-]/g : /[^a-z0-9]/g, '')
        .replace(/^[^a-z]+/, '')
        .slice(0, 30)
    const handle = ident(String(j.handle ?? j.name), false)
    const department = ident(String(j.department ?? handle), true) || handle
    if (handle.length < 2) return null
    return {
      name: String(j.name).slice(0, 60),
      handle,
      department,
      role: String(j.role ?? '').slice(0, 80),
      soul: String(j.soul),
      skills: (j.skills ?? [])
        .filter((s) => s?.name && s?.content)
        .slice(0, 5)
        .map((s) => ({ name: ident(String(s.name), true).replace(/^-+|-+$/g, ''), content: String(s.content) }))
        .filter((s) => s.name.length >= 2),
    }
  } catch {
    return null
  }
}

/** Muse ticket mode returns a JSON patch — tolerant extraction of the first
 *  {...} block (models sometimes wrap in fences despite instructions). */
export interface TicketMusePatch {
  title?: string
  description?: string
  priority?: string
  effort?: string | null
  estimatedHours?: number | null
  dueDate?: string | null
  startDate?: string | null
  color?: string | null
  tags?: string[]
  status?: string
  error?: string
}

export function parseTicketPatch(text: string): TicketMusePatch | null {
  const m = /\{[\s\S]*\}/.exec(text)
  if (!m) return null
  try {
    const j = JSON.parse(m[0]) as Record<string, unknown>
    const out: TicketMusePatch = {}
    if (typeof j.error === 'string') return { error: j.error }
    if (typeof j.title === 'string') out.title = j.title
    if (typeof j.description === 'string') out.description = j.description
    if (typeof j.priority === 'string') out.priority = j.priority
    if (typeof j.effort === 'string' || j.effort === null) out.effort = j.effort as string | null
    if (typeof j.estimatedHours === 'number' || j.estimatedHours === null) out.estimatedHours = j.estimatedHours as number | null
    if (typeof j.dueDate === 'string' || j.dueDate === null) out.dueDate = j.dueDate as string | null
    if (typeof j.startDate === 'string' || j.startDate === null) out.startDate = j.startDate as string | null
    if (typeof j.color === 'string' || j.color === null) out.color = j.color as string | null
    if (Array.isArray(j.tags)) out.tags = j.tags.filter((t): t is string => typeof t === 'string')
    if (typeof j.status === 'string') out.status = j.status
    return Object.keys(out).length ? out : null
  } catch {
    return null
  }
}
