// Slot-level reasoning-effort preferences — the admin's "how hard should this
// CLASS of work think" dial.
//
// WHY THIS EXISTS. Effort could be picked per message (the composer) and per
// agent target (the agent editor's pick beside the model), but the two places
// an admin configures WORKLOAD were model-select-only: a Model role
// ("utility", "code-heavy", …) and a platform agent (Muse, the Distiller, …).
// "Low effort for the easy chores, high for the hard ones" had no home — every
// blurb, every distill ran at the model's default, and the only lever was to
// move the work to a smaller model.
//
// THE SHAPE. One settings row, keys are the SLOT the turn ran under:
//   'role:<ModelRole>'    when the model chain's role step (or the utility
//                         step) produced the model
//   'agent:<PlatformAgentId>'
//                         when the pin step produced it
// The value is the effort string. Slots with no preference are absent —
// absent means "the model's own default", which is also what the empty string
// means everywhere an effort is offered, so there is exactly one spelling of
// "no preference" on either side of the wire.
//
// PRECEDENCE, one rule, stated once: the nearer the ask, the stronger it is.
//   conversation pick > agent-configured target effort > slot preference here
// The runner implements it in one place (harness/run.ts); nothing else may
// read this module at turn time.
//
// VALIDATION IS AT RUN TIME, deliberately. The preference is stored as the
// admin set it and held against the model's LIVE published levels when a turn
// is about to carry it: a level the model stopped publishing is dropped, not
// sent, exactly like a stale agent-configured effort. The write paths also
// validate (a typo in a settings row should bounce), but the run-time check is
// the one correctness depends on — assignments and catalogs move under a
// stored preference, and the turn must degrade rather than 400.
import { getSetting, setSetting } from './audit'
import { effortsForModel } from './model-efforts'

const KEY = 'effort_prefs'

export const roleSlot = (role: string): string => `role:${role}`
export const agentSlot = (id: string): string => `agent:${id}`

export async function getEffortPrefs(): Promise<Record<string, string>> {
  const stored = await getSetting<unknown>(KEY, {})
  if (!stored || typeof stored !== 'object' || Array.isArray(stored)) return {}
  const out: Record<string, string> = {}
  for (const [slot, effort] of Object.entries(stored as Record<string, unknown>)) {
    if (typeof effort === 'string' && effort.trim() && slot.length <= 64) out[slot] = effort.trim()
  }
  return out
}

export async function setEffortPref(slot: string, effort: string | null): Promise<void> {
  const cur = await getEffortPrefs()
  if (effort && effort.trim()) cur[slot] = effort.trim()
  else delete cur[slot]
  await setSetting(KEY, cur)
}

/** The stored preference for a slot, held against the model's live published
 *  levels. Null when there is no preference, or when the preferred level is
 *  not one the model currently publishes — a stale preference is no
 *  preference. Never throws: this exists to improve a turn, not to gate one. */
export async function slotEffortForModel(slot: string, model: string): Promise<string | null> {
  const stored = (await getEffortPrefs())[slot]
  if (!stored) return null
  const levels = await effortsForModel(model).catch((): string[] => [])
  return levels.includes(stored) ? stored : null
}
