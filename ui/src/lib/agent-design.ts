// The agent design — New agent's "describe it" — as the SERVER holds it.
//
// The design used to be orchestrated here, as module state: this file
// awaited the muse call, kept the chat, ticked the elapsed count and decided
// when the design was ready. A reload, a navigation or a closed tab threw
// all of it away, and nothing on the server knew it had existed. The owner's
// rule since 2026-11-04: no run is controlled by browser state.
//
// So the run is the server's (talaria-agent-design): the record lives in
// Redis, the muse call is an `agent-design` run, and `/api/fleet/agent-design`
// is how the browser reads and controls it by id. What is left here is the
// read, four verbs, and pure helpers over the record. The modal keeps only
// view state: which step it shows, the form as it is being typed, a 1 s
// clock for the elapsed count. Opening New agent after a reload, in a second
// tab or on another device resumes exactly where the design stands.
import { createQuery, type QueryClient } from '@tanstack/svelte-query'
import { delJsonGone, getJson, HttpError, patchJson, postJson } from '@/lib/fetch-json'
import { appliedFields, summarizeSoul, type AppliedField, type SoulSummary } from '@/lib/agent-onboard-refine'
import type { AgentDraft } from '@/lib/muse.svelte'

export type AgentDesignStatus = 'generating' | 'ready' | 'error'

/** The fields the review step edits and the hire takes. */
export interface DraftFields {
  name: string
  handle: string
  department: string
  role: string
  soul: string
  skills: AgentDraft['skills']
}

/** The server's record (talaria-agent-design `AgentDesign`), camelCase. */
export interface AgentDesign {
  id: string
  runId: string | null
  status: AgentDesignStatus
  refining: boolean
  /** The first describe: what the splash and the roster row name. */
  purpose: string
  instruction: string
  /** How many chat turns the server holds (the chat itself stays there). */
  chatLength: number
  /** Null until a describe lands. */
  draft: DraftFields | null
  /** The draft a refine started from, for the "Refine applied" receipt. */
  previous: DraftFields | null
  error: string | null
  /** Epoch ms, the SERVER's clock. */
  startedAt: number | null
  finishedAt: number | null
  updatedAt: number
  /** The hire this design is spent on, while it runs. The design is gone
   *  for the modal meanwhile; it comes back (with the hire's sentence in
   *  `error`) if the hire fails. */
  hireRunId: string | null
}

/** One read: the record, and how far the server's clock is from this one. */
export interface DesignRead {
  design: AgentDesign | null
  /** serverNow − Date.now() at the read. */
  skewMs: number
}

export const DESIGN_KEY = ['agent-design'] as const
const URL = '/api/fleet/agent-design'

export const EMPTY_FIELDS: DraftFields = { name: '', handle: '', department: '', role: '', soul: '', skills: [] }

// How many open modals in this tab are looking at the design. View state,
// and the one thing the server cannot see: a read made while one is open
// says `watching`, so a landing in the next few seconds skips the bell.
let watchers = 0

/** The open modal calls this while it is mounted; the return releases it. */
export function watchDesign(): () => void {
  watchers += 1
  return () => {
    watchers = Math.max(0, watchers - 1)
  }
}

export async function fetchAgentDesign(watching: boolean): Promise<DesignRead> {
  const r = await getJson<{ design: AgentDesign | null; now: number }>(watching ? `${URL}?watching=1` : URL)
  return { design: r.design, skewMs: r.now - Date.now() }
}

/** How often to read again: quickly while a design generates and someone
 *  is watching, slowly while it generates in the background (the run's
 *  event on the person's stream usually gets there first), and not at all
 *  once it has settled. */
export function pollEvery(design: AgentDesign | null | undefined, watching: boolean): number | false {
  if (design?.status === 'generating') return watching ? 2_000 : 5_000
  // A hire is spending it: New agent waits, and opens again when the read
  // settles it (consumed, or handed back).
  if (design?.hireRunId) return watching ? 3_000 : 10_000
  return false
}

/** The person's design. One cache entry for every reader — the roster's
 *  row, the New agent mark and the modal — refetched by the `run` events
 *  on the person's stream (user-events.svelte.ts) and by the poll above. */
export function useAgentDesign(enabled: () => boolean = () => true) {
  return createQuery(() => ({
    queryKey: DESIGN_KEY,
    enabled: enabled(),
    queryFn: () => fetchAgentDesign(watchers > 0),
    refetchInterval: (q: { state: { data?: DesignRead } }) => pollEvery(q.state.data?.design, watchers > 0),
    refetchOnWindowFocus: true,
  }))
}

/** Put a verb's answer in the cache, keeping the clock skew last read. */
function settle(qc: QueryClient, design: AgentDesign | null): AgentDesign | null {
  const prev = qc.getQueryData<DesignRead>(DESIGN_KEY)
  qc.setQueryData<DesignRead>(DESIGN_KEY, { design, skewMs: prev?.skewMs ?? 0 })
  return design
}

/** A describe. While one is generating the server answers that one. */
export async function startDesign(qc: QueryClient, purpose: string): Promise<AgentDesign> {
  const r = await postJson<{ design: AgentDesign }>(URL, { instruction: purpose.trim() })
  return settle(qc, r.design) as AgentDesign
}

/** A refine of the form as the person sees it, hand edits included. Names
 *  the design it revises: a stale tab gets a 409 (`isStale`). */
export async function refineDesign(qc: QueryClient, id: string, instruction: string, current: DraftFields): Promise<AgentDesign> {
  const r = await postJson<{ design: AgentDesign }>(URL, { instruction: instruction.trim(), refine: true, id, current })
  return settle(qc, r.design) as AgentDesign
}

/** The person's own draft: review-step edits, a role template, a manual
 *  start. Names the design it edits (none only for a manual start), so a
 *  tab that missed a discard elsewhere gets a 409, never a write over the
 *  newer design. `init` carries `keepalive` for the save on the way out. */
export async function saveDesign(qc: QueryClient, fields: DraftFields, id?: string, init?: RequestInit): Promise<AgentDesign> {
  const r = await patchJson<{ design: AgentDesign }>(URL, id ? { ...fields, id } : fields, init)
  return settle(qc, r.design) as AgentDesign
}

/** The server refused a write because the design it named is not the one
 *  held (another tab or device replaced it), or a run or hire holds it. */
export function isConflict(e: unknown): boolean {
  return e instanceof HttpError && e.status === 409
}

/** Discard the design (and cancel its run). Gone already is done. */
export async function discardDesign(qc: QueryClient, id?: string): Promise<void> {
  await delJsonGone(URL, id ? { id } : {})
  settle(qc, null)
}

// ── Pure helpers over the record ─────────────────────────────────────────────

/** The design the modal, the roster row and the + mark offer: the record,
 *  unless a hire is spending it right now (the hiring strip tells that
 *  story; the record returns here if the hire fails). */
export function pendingDesign(design: AgentDesign | null | undefined): AgentDesign | null {
  return design && !design.hireRunId ? design : null
}

/** The design a hire is spending right now, if any. While there is one, New
 *  agent waits: a describe or a manual start would have to replace the
 *  design the hire hands back if it fails, so the server refuses both (409)
 *  and the modal says why instead of offering them. */
export function heldDesign(design: AgentDesign | null | undefined): AgentDesign | null {
  return design?.hireRunId ? design : null
}

// ── The edit that could not leave with the page ──────────────────────────────
//
// A save on `pagehide` rides `keepalive`, and the browser caps a keepalive
// body at 64 KiB while a soul alone may be 200k characters. Such an edit is
// kept here, in this browser, and offered back the next time New agent opens
// on the same design: per-viewer view state, the one thing localStorage is
// for, never the design itself. A reload that outruns its own keepalive save
// is caught the same way.
const UNSAVED_KEY = 'talaria.agent-design.unsaved'
const UNSAVED_TTL_MS = 24 * 3600 * 1000
/** Under the browser's keepalive cap, with room for the headers. */
export const KEEPALIVE_MAX_BYTES = 60_000

export interface UnsavedEdit {
  id: string | null
  form: DraftFields
  at: number
}

export function stashUnsaved(edit: UnsavedEdit): void {
  try {
    localStorage.setItem(UNSAVED_KEY, JSON.stringify(edit))
  } catch {
    /* storage unavailable: nothing more this browser can do */
  }
}

/** The stashed edit for this design, if one is fresh and differs from what
 *  the server holds; it is removed either way once read. */
export function takeUnsaved(id: string | null, server: DraftFields | null, now: number = Date.now()): DraftFields | null {
  let raw: string | null = null
  try {
    raw = localStorage.getItem(UNSAVED_KEY)
    localStorage.removeItem(UNSAVED_KEY)
  } catch {
    return null
  }
  if (!raw) return null
  try {
    const edit = JSON.parse(raw) as UnsavedEdit
    if (edit.id !== id || now - edit.at > UNSAVED_TTL_MS) return null
    if (server && JSON.stringify(server) === JSON.stringify(edit.form)) return null
    return edit.form
  } catch {
    return null
  }
}

/** What a pagehide save can send: the whole edit when it fits the
 *  keepalive cap, else the small fields over the server's soul and skills
 *  (the rest waits in `stashUnsaved`). `whole` says which it is. */
export function keepaliveBody(form: DraftFields, server: DraftFields | null): { fields: DraftFields; whole: boolean } {
  const size = new TextEncoder().encode(JSON.stringify(form)).length
  if (size <= KEEPALIVE_MAX_BYTES) return { fields: form, whole: true }
  return {
    fields: { ...form, soul: server?.soul ?? '', skills: server?.skills ?? [] },
    whole: false,
  }
}

/** Where New agent opens. A design that landed, or a refine that failed
 *  over a draft, opens on review; a describe in flight or a describe that
 *  failed opens on describe, where the splash or the sentence is. */
export function openingStep(design: AgentDesign | null | undefined, preselect: boolean): 'describe' | 'review' {
  if (preselect) return 'review'
  if (!design) return 'describe'
  if (design.status === 'ready') return 'review'
  if (design.refining || (design.status === 'error' && design.draft)) return 'review'
  return 'describe'
}

/** Seconds since the server started this turn, on the server's clock — the
 *  same count in every tab and after a reload. */
export function elapsedSeconds(design: AgentDesign | null | undefined, skewMs: number, clock: number = Date.now()): number {
  if (!design?.startedAt) return 0
  return Math.max(0, Math.floor((clock + skewMs - design.startedAt) / 1000))
}

/** What identifies one run's landing: the form reseeds from the server's
 *  draft when this changes, and only then — a hand edit's save must not
 *  overwrite what is being typed. */
export function landingKey(design: AgentDesign | null | undefined): string {
  return design ? `${design.id}:${design.runId ?? '-'}:${design.finishedAt ?? '-'}` : ''
}

export interface DesignChange {
  fields: AppliedField[]
  soul: SoulSummary
}

const RECEIPT_ORDER = [
  { label: 'Role', key: 'role' },
  { label: 'Soul', key: 'soul' },
  { label: 'Starter skills', key: 'skills' },
]

/** The "Refine applied" receipt: what the landed refine changed against
 *  the form it started from. Null for anything but a landed refine. */
export function refineReceipt(design: AgentDesign | null | undefined): DesignChange | null {
  const before = design?.previous
  const after = design?.draft
  if (!design || design.status !== 'ready' || !before || !after) return null
  return {
    fields: appliedFields(
      { role: before.role, soul: before.soul, skills: before.skills },
      { role: after.role, soul: after.soul, skills: after.skills },
      RECEIPT_ORDER,
    ),
    soul: summarizeSoul(before.soul, after.soul),
  }
}

/** The roster's row and the New agent mark: what to say about a design. */
export function designHeadline(design: AgentDesign): string {
  if (design.status === 'ready') return design.draft?.name || 'the design'
  const first = design.purpose
    .split(/\s+/)
    .slice(0, 7)
    .join(' ')
    .replace(/[.,;:!?]+$/, '')
  return first || 'the new agent'
}
