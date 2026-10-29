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
 *  list). Hermes' todo list would also reset every turn of an ordinary chat:
 *  it lives in the session transcript, which a completions-arm turn does not
 *  carry (talaria-agent-defs' `MANDATORY_DISABLED` says when that changes). The server publishes it as `toolsets.mandatoryDisabled` and callers
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

/** Hermes' reading of a config value as `str(v or "")`. */
function pyText(v: unknown): string {
  if (v === null || v === undefined || v === false || v === 0 || v === '') return ''
  if (v === true) return 'True'
  if (typeof v === 'number' || typeof v === 'string') return String(v)
  if (Array.isArray(v)) return v.length ? JSON.stringify(v) : ''
  if (typeof v === 'object') return Object.keys(v).length ? JSON.stringify(v) : ''
  return String(v)
}

/**
 * The browser a def's own `browser:` block gives it on Hermes v0.22.4
 * (`latest.config.raw.browser`), or null when the block leaves Hermes'
 * default, Browser Use mode (`browser_exec`), in place.
 *
 * - `exec`: `browser.backend: browser-use`, the default spelled out.
 * - `local`: the built-in `browser_*` tools on a browser Hermes calls local,
 *   for which it skips its private-address check (`_is_local_backend`): any
 *   other backend on this machine's Chromium, or Camofox
 *   (`browser.cloud_provider: camofox`).
 * - `cdp` / `hosted`: the built-in tools on a CDP endpoint
 *   (`browser.cdp_url`) or a hosted provider (`browser.cloud_provider`),
 *   which keep that check.
 * - `checked`: the built-in tools on this machine's Chromium while the
 *   agent's terminal runs elsewhere (`terminal.backend` other than local,
 *   which Hermes bridges into `TERMINAL_ENV`), so Hermes keeps the check
 *   for it too. `terminal` names that setting.
 *
 * It mirrors talaria-fleet-render's `hermes_browser`: the config, and the
 * names of the agent's own secrets, which reach Hermes as its env. A
 * `BROWSER_CDP_URL` secret is the same CDP override as `browser.cdp_url`
 * (Hermes reads it first). With no names (a viewer who cannot read the
 * agent's secrets) it reads the config alone; the renderer also reads a
 * `CAMOFOX_URL` secret, which this does not.
 * `via` is the setting that decided it, as the grid's line names it.
 */
export type AuthoredBrowser = { kind: 'exec' | 'local' | 'cdp' | 'hosted' | 'checked'; via: string; terminal?: string }

/** The terminal backend Hermes v0.22.4 reads as elsewhere (`TERMINAL_ENV`
 *  neither `local` nor empty), as the setting that says so, or null for a
 *  local one. The gateway writes `terminal.backend` into that variable as
 *  `str()` of the value whenever the key is present, so a null reads
 *  `None`, a bool `True`/`False`, and only a string can be local; no
 *  `terminal` map, or no `backend` in it, is the default `local`. Mirrors
 *  talaria-fleet-render's `hermes_terminal_local`. */
function terminalElsewhere(raw: unknown): string | null {
  const terminal = (raw as { terminal?: unknown } | null | undefined)?.terminal
  if (!terminal || typeof terminal !== 'object' || Array.isArray(terminal) || !('backend' in terminal)) return null
  const backend = (terminal as Record<string, unknown>).backend
  if (typeof backend === 'string') {
    const name = backend.trim().toLowerCase()
    return name === 'local' || name === '' ? null : `terminal.backend: ${name}`
  }
  const spelled = backend === null ? 'None' : backend === true ? 'True' : backend === false ? 'False' : typeof backend === 'number' ? String(backend) : JSON.stringify(backend)
  return `terminal.backend: ${spelled}`
}

export function authoredBrowser(config: unknown, secretNames: readonly string[] = []): AuthoredBrowser | null {
  const raw = (config as { raw?: unknown } | null | undefined)?.raw
  const browser = (raw as { browser?: unknown } | null | undefined)?.browser
  if (!browser || typeof browser !== 'object' || Array.isArray(browser)) return null
  const cfg = browser as Record<string, unknown>
  // get_browser_backend: YAML 1.1's bare `off` is `false`, a choice; `true` is unset.
  const backend =
    cfg.backend === false ? 'off' : cfg.backend === true ? '' : pyText(cfg.backend).trim().toLowerCase()
  const cdpSecret = secretNames.includes('BROWSER_CDP_URL')
  const cdp = cdpSecret || pyText(cfg.cdp_url).trim() !== ''
  const provider = cfg.cloud_provider
  const selection =
    provider === null || provider === undefined
      ? ''
      : typeof provider === 'boolean'
        ? String(provider)
        : pyText(provider).trim().toLowerCase()
  const camofox = !cdp && selection === 'camofox'
  if (!camofox && (backend === '' || backend === 'browser-use')) {
    return backend ? { kind: 'exec', via: 'browser.backend: browser-use' } : null
  }
  if (cdp) return { kind: 'cdp', via: cdpSecret ? 'a BROWSER_CDP_URL secret' : 'browser.cdp_url' }
  if (camofox) return { kind: 'local', via: 'browser.cloud_provider: camofox' }
  const hosted = pyText(provider).trim().toLowerCase()
  if (hosted && hosted !== 'local' && hosted !== 'camofox') {
    return { kind: 'hosted', via: `browser.cloud_provider: ${hosted}` }
  }
  // `_is_local_backend` asks last where the terminal runs: a shell in a
  // container (or over ssh) keeps the check for this machine's browser.
  const terminal = terminalElsewhere(raw)
  if (terminal) return { kind: 'checked', via: `browser.backend: ${backend}`, terminal }
  return { kind: 'local', via: `browser.backend: ${backend}` }
}

/**
 * What the `browser` row gives this agent, or null when there is nothing to
 * say (the row is off, or there is no browser row at all).
 *
 * The terminal decides whether there is a browser at all. Hermes v0.22.4's
 * own browser skips its private-address check whenever it runs locally, on
 * the grounds that the terminal reaches the same networks anyway. That holds
 * for an agent with a terminal and not for one without, so Talaria installs
 * the browser's engine (agent-browser, driving the image's Chromium) only for
 * an agent with both `browser` and `terminal` on (talaria-fleet-render's
 * `fleet_browser_engine`). With the terminal closed, the box stays ticked and
 * the agent has no browser, and this line says why; without it, a ticked box
 * reads as a working browser. Opening `terminal` brings Hermes' default
 * `browser_exec`.
 *
 * A browser the def's own config chose (`authored`, from
 * [`authoredBrowser`]) stands with the terminal open. With it closed, a
 * local one has the toolset closed by the renderer
 * (`fleet_browser_local_close`), so an engine an earlier start left on the
 * volume runs nothing; a CDP endpoint, a hosted provider, or a terminal
 * that runs elsewhere keeps Hermes' check and is left alone. The engine is
 * installed from npm onto the
 * agent's volume once per Hermes pin, which is why the line says a start
 * needs the registry.
 */
export function browserNote(rows: readonly ToolsetRow[], authored: AuthoredBrowser | null = null): string | null {
  const browser = rows.find((r) => r.name === 'browser')
  if (!browser?.on) return null
  const terminal = rows.find((r) => r.name === 'terminal')?.on ?? false
  if (!terminal) {
    if (authored?.kind === 'cdp') {
      const where = authored.via === 'browser.cdp_url' ? 'this agent’s config sets' : 'this agent’s secrets supply'
      return `Browser: the built-in browser tools on the CDP endpoint ${where} (${authored.via}). Hermes keeps its private-address check for a remote browser, so it does not need the terminal.`
    }
    if (authored?.kind === 'hosted') {
      return `Browser: the built-in browser tools on the hosted browser this agent’s config sets (${authored.via}), which keeps Hermes’ private-address check. They need the browser engine, which Talaria installs only while the terminal is open.`
    }
    if (authored?.kind === 'checked') {
      return `Browser: the built-in browser tools this agent’s config sets (${authored.via}), which keep Hermes’ private-address check because its terminal runs elsewhere (${authored.terminal}). They need the browser engine, which Talaria installs only while the terminal is open.`
    }
    const why =
      'On this image, Hermes’ own browser skips its private-address check, on the grounds that the terminal already reaches this machine, other agents and your LAN. That holds only with a terminal, so Talaria gives an agent the browser only with the terminal open.'
    return authored?.kind === 'local'
      ? `Browser: none while the terminal is closed. ${why} This agent’s config picks a browser Hermes runs locally (${authored.via}), so Talaria closes the browser toolset for it until the terminal opens.`
      : `Browser: none while the terminal is closed. ${why}`
  }
  const engine =
    ' Its engine is installed from npm onto the agent’s volume before the agent starts; with no route to the registry, the agent starts without browser tools.'
  if (authored?.kind === 'exec') {
    return `Browser: Hermes’ browser_exec, which comes with the terminal (this agent’s config sets ${authored.via}).${engine}`
  }
  if (authored) {
    return `Browser: the built-in browser tools (browser_navigate and the rest), as this agent’s config sets ${authored.via}.${engine}`
  }
  return `Browser: Hermes’ browser_exec, which comes with the terminal.${engine}`
}
