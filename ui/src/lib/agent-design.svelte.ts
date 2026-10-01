// TALA-11 part 2: the in-flight agent design, as module state.
//
// The design used to live inside CreateAgentModal's component state, which
// meant closing the modal killed it — a whole-agent generation runs tens of
// seconds to minutes, and the one thing the person could not do while it ran
// was look away. The modal REMOUNTS per open (Agents.svelte renders it under
// `{#if creating}`), so per-open component state is exactly the wrong home;
// the orchestration moves here, next to toast.svelte.ts in the "module-level
// reactive store" idiom.
//
// ONE DESIGN AT A TIME. A second startDesign() while one is in flight does
// NOT silently replace it — it returns the existing id so the caller can
// decide (today every caller just re-enters the same run). An idle finished
// design IS replaced: a fresh describe is a fresh intent.
//
// The honesty rules are unchanged from the modal's inline era: no
// percentage, nothing that sweeps toward an end; the elapsed count (shown
// once it is genuinely taking a while) is the one number the surface can
// actually measure.
import { draftAgent, type AgentDraft } from '@/lib/muse.svelte'
import { appliedFields, summarizeSoul, type AppliedField, type SoulSummary } from '@/lib/agent-onboard-refine'
import { pushToast } from '@/lib/toast.svelte'

export type AgentDesignStatus = 'generating' | 'ready' | 'error'

/** The refine receipt (TALA-4): what the accepted refine changed. */
export interface DesignChange {
  fields: AppliedField[]
  soul: SoulSummary
}

export interface AgentDesign {
  id: number
  /** The INITIAL instruction — the describe text. Stays visible while
   *  generating and on error, so a reopen shows what was asked for. */
  purpose: string
  /** The LATEST instruction — the last refine text, when refining. */
  instruction: string
  refining: boolean
  status: AgentDesignStatus
  error: string
  genSeconds: number
  chat: Array<{ role: 'user' | 'assistant'; content: string }>
  // The filled draft fields (empty until the first answer lands).
  name: string
  handle: string
  department: string
  role: string
  soul: string
  /** RichEditor reseed counter — bumps whenever muse redrafts the soul. */
  soulRev: number
  skills: AgentDraft['skills']
  lastChange: DesignChange | null
}

// Module state, like toast.svelte.ts: `$state` at module scope IS the store.
const store = $state<{ design: AgentDesign | null }>({ design: null })
let nextId = 1
let tick: ReturnType<typeof setInterval> | null = null

/** Read API: the current design, or null when nothing is in flight or done. */
export function currentDesign(): AgentDesign | null {
  return store.design
}

/** True when a design is generating right now (initial or refine). */
export function designGenerating(): boolean {
  return store.design?.status === 'generating'
}

// The toast ("the design landed") must fire only when the modal is NOT open
// — the person standing in the review step already knows. The store cannot
// know that; the modal reports it, exactly like a caller reporting anything
// else the store has no way to see.
let modalOpen = false
export function setModalOpen(open: boolean): void {
  modalOpen = open
}

/** Forget the current design (the caller claims it — the hire went out, or
 *  the person started over). */
export function clearDesign(): void {
  if (tick) {
    clearInterval(tick)
    tick = null
  }
  store.design = null
}

/** One design turn: the shared body of startDesign and refineDesign. */
async function run(design: AgentDesign, instruction: string, refining: boolean): Promise<void> {
  // A design is a whole-agent generation — tens of seconds normally, minutes
  // on a slow provider day. The elapsed count (shown once it's genuinely
  // taking a while) is what separates "working, slowly" from "wedged" — the
  // difference between waiting it out and canceling a turn that was about to
  // land.
  design.genSeconds = 0
  design.status = 'generating'
  design.refining = refining
  design.error = ''
  design.instruction = instruction
  if (tick) clearInterval(tick)
  tick = setInterval(() => (design.genSeconds += 1), 1000)
  try {
    // The turn the model sees on the next refine is the VALIDATED draft, not
    // whatever text it happened to emit — so a reply that needed a repair
    // turn does not teach the model its own broken shape on the way round
    // again.
    const draft = await draftAgent({
      instruction,
      ...(refining
        ? {
            current: JSON.stringify({
              name: design.name,
              handle: design.handle,
              department: design.department,
              role: design.role,
              soul: design.soul,
              skills: design.skills,
            }),
          }
        : {}),
      chat: design.chat,
    })
    design.chat = [
      ...design.chat.slice(-8),
      { role: 'user', content: instruction },
      { role: 'assistant', content: JSON.stringify(draft) },
    ]
    if (refining) {
      // The receipt compares against what the viewer was looking at — the
      // form's live values, not the last chat turn (hand edits between
      // refines are changes too, and the model saw them via `current`).
      design.lastChange = {
        fields: appliedFields(
          { role: design.role, soul: design.soul, skills: design.skills },
          { role: draft.role, soul: draft.soul, skills: draft.skills },
          [
            { label: 'Role', key: 'role' },
            { label: 'Soul', key: 'soul' },
            { label: 'Starter skills', key: 'skills' },
          ],
        ),
        soul: summarizeSoul(design.soul, draft.soul),
      }
    }
    design.name = draft.name
    design.handle = draft.handle
    design.department = draft.department
    design.role = draft.role
    design.soul = draft.soul
    design.soulRev += 1 // reseed the editor with the new draft
    design.skills = draft.skills
    design.status = 'ready'
    if (!modalOpen) {
      pushToast({ title: 'The agent design is ready', body: draft.name, href: '/agents', tone: 'success' })
    }
  } catch (e) {
    design.error = (e as Error).message
    design.status = 'error'
  } finally {
    if (tick) {
      clearInterval(tick)
      tick = null
    }
    design.refining = false
  }
}

/**
 * Kick off a new design from a describe. If one is already in flight the call
 * does NOT replace it — the caller gets the existing design's id back so it
 * can decide what to do (re-entering it is the same as following it). An
 * idle design (ready/error) IS replaced: a fresh describe is a fresh intent.
 */
export function startDesign(purpose: string): number {
  const existing = store.design
  if (existing && existing.status === 'generating') return existing.id
  const design: AgentDesign = {
    id: nextId++,
    purpose: purpose.trim(),
    instruction: purpose.trim(),
    refining: false,
    status: 'generating',
    error: '',
    genSeconds: 0,
    chat: [],
    name: '',
    handle: '',
    department: '',
    role: '',
    soul: '',
    soulRev: 0,
    skills: [],
    lastChange: null,
  }
  store.design = design
  void run(design, purpose.trim(), false)
  return design.id
}

/** Seed an idle, ready design from caller-owned fields — the role-template
 *  path on the describe step. No muse run: the record exists so the review
 *  step's store bindings and the refine's `current` read it, and everything
 *  stays editable from there. Replaces any idle design; an in-flight run is
 *  left alone (the role picker is hidden while one runs). */
export function adoptDesign(fields: {
  name: string
  handle: string
  department: string
  role: string
  soul: string
  skills?: AgentDraft['skills']
}): void {
  const existing = store.design
  if (existing && existing.status === 'generating') return
  store.design = {
    id: nextId++,
    purpose: '',
    instruction: '',
    refining: false,
    status: 'ready',
    error: '',
    genSeconds: 0,
    chat: [],
    name: fields.name,
    handle: fields.handle,
    department: fields.department,
    role: fields.role,
    soul: fields.soul,
    soulRev: 1,
    skills: fields.skills ?? [],
    lastChange: null,
  }
}

/** Refine the current design. No-op when there is nothing to refine or a
 *  refine is already running (the RefineBar disables itself while busy). */
export function refineDesign(instruction: string): void {
  const design = store.design
  if (!design || design.status === 'generating' || !instruction.trim()) return
  void run(design, instruction.trim(), true)
}
