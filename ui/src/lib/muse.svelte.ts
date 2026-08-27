// Client for the drafting muse + model preferences.
//
// THE MUSE ANSWERS TWO WAYS, and this file is where that shows.
//
//   `streamMuse` — the six PROSE kinds. Tokens arrive as they are generated and
//   land in the editor. Unchanged.
//
//   `draftCron` / `draftAgent` / `draftTicketPatch` / `draftSkillForm` /
//   `draftTemplateForm` — the five JSON kinds. The first three used to stream
//   too, and this file parsed the result with three greedy `/\{[\s\S]*\}/`
//   regexes that returned `null` on anything a model wrapped in prose (audit
//   1.1). The browser is the worst possible place for that parse: the tokens
//   are already spent, so there is no repair turn; no guardrail can run on a
//   value that never passed through the server; and a `null` is a button that
//   silently does nothing. The parse, the schema, the identifier coercion, the
//   ticket field allowlist and the form-record schemas all live in
//   `server/harness/defs/muse.ts` now. What is left here is a fetch.
import { createQuery } from '@tanstack/svelte-query'
import { errorMessage, getJson, postJson, postStream, putJson } from '@/lib/fetch-json'

export type MuseKind =
  | 'soul'
  | 'personality'
  | 'skill'
  | 'memory'
  | 'cron'
  | 'agent'
  | 'document'
  | 'template'
  | 'ticket'
  | 'skillForm'
  | 'templateForm'

export interface MuseRequest {
  kind: MuseKind
  instruction: string
  current?: string
  context?: string
  chat?: Array<{ role: 'user' | 'assistant'; content: string }>
}

/** Stream a muse draft; onChunk receives text pieces as they arrive.
 *  Returns the full text. Throws with the server's message on failure.
 *
 *  PROSE KINDS ONLY. The five structured kinds answer with JSON and would
 *  arrive here as one lump; use the `draft*` helpers below. */
export async function streamMuse(input: MuseRequest, onChunk: (piece: string) => void, signal?: AbortSignal): Promise<string> {
  const r = await postStream('/api/muse', input, { signal })
  const reader = r.body.getReader()
  const decoder = new TextDecoder()
  let full = ''
  // Raw chunks, not SSE frames — so this loop is its own reader, but the
  // discipline is the same one `sseFrames` enforces: every exit path cancels
  // and releases, or an early return leaks the socket (see sse-parse.ts).
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      const piece = decoder.decode(value, { stream: true })
      full += piece
      onChunk(piece)
    }
    return full
  } finally {
    await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}

/** One structured draft, already validated against its schema on the server.
 *
 *  THROWS on every failure, with a sentence written for the person reading it —
 *  which is why no caller carries its own "could not turn that into a job"
 *  string any more. There is no `null` return: the difference between "the model
 *  produced nothing usable" and "the gateway has no model" is a real difference,
 *  and both of them are things to say out loud rather than states to swallow. */
async function draft<T>(input: MuseRequest, signal?: AbortSignal): Promise<T> {
  // postJson throws the server's sentence on every non-2xx; the guard below is
  // the one failure it cannot see — a 200 whose body never carried a `value`.
  const { value } = await postJson<{ value?: T }>('/api/muse', input, { signal })
  if (value === undefined) throw new Error('The server sent a reply this app could not read.')
  return value
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
  return createQuery(() => ({
    queryKey: ['gateway-models'],
    staleTime: 60_000,
    queryFn: (): Promise<{ models: GatewayModel[]; effective: string | null }> =>
      getJson<{ models: GatewayModel[]; effective: string | null }>('/api/models'),
  }))
}

/** The signed-in user's saved preferences. `preferredModel: null` = the
 *  server's default drafting model; `preferredEffort: null` = no default, so
 *  every effort-enabled surface starts at the model's own default;
 *  `timezone: null` = follow the workspace zone. */
export interface ProfilePrefs {
  preferredModel: string | null
  preferredEffort: string | null
  timezone: string | null
}

/** The profile preferences, including `preferredModel: null` meaning "no
 *  preference set" — a real 200 answer. A failed profile read must not look
 *  like a cleared preference. */
export function useProfilePrefs() {
  return createQuery(() => ({
    queryKey: ['profile-prefs'],
    queryFn: (): Promise<ProfilePrefs> => getJson<ProfilePrefs>('/api/profile'),
  }))
}

// All three profile writes answer `{ ok: true }` and every caller reads an
// in-band `.error` field instead of catching, so they resolve-only through the
// door: rejections fold into the same `{ error }` shape.
const savePref = (body: Record<string, unknown>): Promise<{ error?: string }> =>
  putJson<{ ok: true }>('/api/profile', body)
    .then(() => ({}))
    .catch((e: unknown) => ({ error: errorMessage(e) }))

export const savePreferredModel = (model: string | null): Promise<{ error?: string }> => savePref({ preferredModel: model })

/** Save the platform-default reasoning effort. `null` clears it — every model
 *  runs at its own default again. The preference is a bare level string; each
 *  surface applies it only where the model's metadata publishes the level. */
export const savePreferredEffort = (effort: string | null): Promise<{ error?: string }> => savePref({ preferredEffort: effort })

/** Save the person's IANA zone — the zone their brief opens in and their
 *  digest arrives in. `null` clears it back to "follow the workspace zone".
 *  Used by the Settings picker and by TimezoneAdopt's silent first adoption. */
export const saveTimezone = (timezone: string | null): Promise<{ error?: string }> => savePref({ timezone })

// ── The five structured drafts ───────────────────────────────────────────────
//
// The interfaces below MIRROR the zod schemas in `server/harness/defs/muse.ts`,
// the same way `GatewayModel` above mirrors the gateway's own type. The server
// is the authority: it validates, it coerces the identifiers, it drops the
// fields outside the allowlist. Nothing on this side re-checks any of that —
// re-checking would be a second, drifting copy of a contract, which is the
// arrangement this whole port exists to end.

export interface CronDraft {
  name: string
  schedule: string
  prompt: string
}

export interface AgentDraft {
  name: string
  handle: string
  department: string
  role: string
  soul: string
  skills: Array<{ name: string; content: string }>
}

/** A previewable patch on one ticket, or `{ error }` when the instruction asked
 *  for something outside the fields the Muse may change (an assignee, a board
 *  move, a question). The two never arrive together. */
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

export const draftCron = (input: Omit<MuseRequest, 'kind'>, signal?: AbortSignal): Promise<CronDraft> =>
  draft<CronDraft>({ ...input, kind: 'cron' }, signal)

export const draftAgent = (input: Omit<MuseRequest, 'kind'>, signal?: AbortSignal): Promise<AgentDraft> =>
  draft<AgentDraft>({ ...input, kind: 'agent' }, signal)

export const draftTicketPatch = (input: Omit<MuseRequest, 'kind'>, signal?: AbortSignal): Promise<TicketMusePatch> =>
  draft<TicketMusePatch>({ ...input, kind: 'ticket' }, signal)

/** The complete record of ONE skill — the two fields of the skill view: its
 *  name and its full SKILL.md — or `{ error }` when the instruction asked for
 *  something the form cannot do (delete, move, create a second skill). The
 *  two never arrive together; the server coerces the name to the write path's
 *  alphabet before it reaches the caller. */
export interface SkillForm {
  name?: string
  content?: string
  error?: string
}

/** The complete record of ONE template — the three fields of the Templates
 *  view: its name, its prompt-only guidance and its skeleton body — or
 *  `{ error }` on refusal. The two never arrive together. */
export interface TemplateForm {
  name?: string
  guidance?: string
  body?: string
  error?: string
}

export const draftSkillForm = (input: Omit<MuseRequest, 'kind'>, signal?: AbortSignal): Promise<SkillForm> =>
  draft<SkillForm>({ ...input, kind: 'skillForm' }, signal)

export const draftTemplateForm = (input: Omit<MuseRequest, 'kind'>, signal?: AbortSignal): Promise<TemplateForm> =>
  draft<TemplateForm>({ ...input, kind: 'templateForm' }, signal)
