// The Hermes-toolset grid's logic, away from its markup so it can be tested
// (ToolsetsControl.svelte is the view).
//
// THE ONE THING TO HOLD ON TO: the stored list is the toolsets an agent does
// NOT get (`AgentDef.disabledToolsets`), and the checkbox is the toolset
// being ON. Every function here converts between those two directions, and
// the sign error that reads "checked" as "disabled" is the whole reason the
// conversion lives in one place.

/** The pair Talaria takes off every agent's table whatever a def says: its
 *  own todo and clarify tools are the ones a person actually sees, so Hermes'
 *  are closed fleet-wide (the renderer merges them ahead of the agent's own
 *  list). The server publishes it as `toolsets.mandatoryDisabled` and callers
 *  pass that through — this is only the default, so a three-argument
 *  `toggleToolset` still cannot author one of them into a stored list. Spelled
 *  in the server constant's order (talaria-agent-defs' `MANDATORY_DISABLED`)
 *  so the two read as the same list; everything here treats it as a set. */
export const MANDATORY_DISABLED: readonly string[] = ['todo', 'clarify']

export interface ToolsetRow {
  /** The governed toolset name, spelled as the server spells it. */
  name: string
  /** ON for this agent — i.e. NOT in its disabled list. */
  on: boolean
  /** The floor: shown off, and not the operator's to change. */
  forced: boolean
  /** Forced by the ORG floor — an admin closed it for every agent — rather
   *  than by Talaria's own pair. The lock is the same; the reason shown is
   *  not. */
  orgClosed: boolean
}

/**
 * One row per governed toolset, in the order the server gave them (which is
 * alphabetical), deduped.
 *
 * A forced name reads off whatever the stored list says: a legacy def that
 * still carries `todo` and one that never did must render identically,
 * because the rendered config is identical. `orgDisabled` is the part of the
 * floor an admin set (the server's `toolsets.orgDisabled`); a name in it is
 * forced whether or not `mandatoryDisabled` repeats it.
 */
export function toolsetRows(
  governed: readonly string[],
  mandatoryDisabled: readonly string[],
  disabled: readonly string[],
  orgDisabled: readonly string[] = [],
): ToolsetRow[] {
  const org = new Set(orgDisabled.map((n) => n.trim()))
  const forced = new Set([...mandatoryDisabled.map((n) => n.trim()), ...org])
  const off = new Set(disabled.map((n) => n.trim()))
  const seen = new Set<string>()
  const rows: ToolsetRow[] = []
  for (const raw of governed) {
    const name = raw.trim()
    if (!name || seen.has(name)) continue
    seen.add(name)
    const isForced = forced.has(name)
    rows.push({ name, on: !isForced && !off.has(name), forced: isForced, orgClosed: org.has(name) })
  }
  return rows
}

/**
 * The part of the published floor that is never STORED on a def: Talaria's
 * own pair, i.e. `mandatoryDisabled` minus the org part.
 *
 * This is what `normalizeToolsets` and `toggleToolset` strip. An org-closed
 * name must NOT be stripped: a manager who closed `terminal` on one agent
 * before an admin closed it fleet-wide still means it, and the day the admin
 * lifts the floor that agent must come back exactly as its managers left it —
 * not reopened by an unrelated Apply that rewrote the list without it.
 * With no vocabulary (an older api) the pair is the floor.
 */
export function builtinFloor(
  toolsets: { mandatoryDisabled: readonly string[]; orgDisabled?: readonly string[] } | undefined,
): readonly string[] {
  if (!toolsets) return MANDATORY_DISABLED
  const org = new Set((toolsets.orgDisabled ?? []).map((n) => n.trim()))
  return toolsets.mandatoryDisabled.filter((n) => !org.has(n.trim()))
}

/**
 * The storable spelling of a disabled list: trimmed, deduped, sorted, and
 * without the floor.
 *
 * SORTED matters for more than tidiness. Two operators who reached the same
 * set by different routes have to produce the same list, or the PATCH writes
 * a different array, the column differs, and the agent rolls for a change
 * that is not one. The floor comes out for the same reason: `todo` is a
 * governed name, so a def CAN legitimately carry it, and a list that does
 * means exactly what the same list without it means — the renderer merges
 * those two names on top of whatever is stored.
 */
export function normalizeToolsets(
  disabled: readonly string[],
  forced: readonly string[] = MANDATORY_DISABLED,
): string[] {
  const next = new Set(disabled.map((n) => n.trim()).filter(Boolean))
  for (const f of forced) next.delete(f.trim())
  return [...next].sort()
}

/**
 * Flip one toolset and hand back the WHOLE disabled list to store.
 *
 * `on` is the TOOLSET's state, so the list moves the other way.
 */
export function toggleToolset(
  disabled: readonly string[],
  name: string,
  on: boolean,
  forced: readonly string[] = MANDATORY_DISABLED,
): string[] {
  const target = name.trim()
  const without = disabled.filter((n) => n.trim() !== target)
  return normalizeToolsets(on ? without : [...without, target], forced)
}

/** Whether two disabled lists mean the same thing — a SET comparison, because
 *  the server's own normalisation keeps insertion order while `toggleToolset`
 *  sorts. Order drift must not light up an Apply button that would roll a
 *  container for nothing. */
export function sameToolsets(a: readonly string[], b: readonly string[]): boolean {
  const left = new Set(a.map((n) => n.trim()).filter(Boolean))
  const right = new Set(b.map((n) => n.trim()).filter(Boolean))
  if (left.size !== right.size) return false
  for (const n of left) if (!right.has(n)) return false
  return true
}
