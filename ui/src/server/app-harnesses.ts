// THE APP LAYER OF THE ACTIVITY-HARNESS REGISTRY: the harnesses an enabled
// app ships in `apps/<slug>/harnesses/*.ts`, loaded, checked and handed to the
// api's fitness plane as DATA.
//
// WHY IT LIVES HERE AND NOT IN RUST. An app harness is customer code —
// `render` builds the messages, `output.clean`/`verify` judge the reply,
// `evals[].check` grades it — and app modules are never port surface
// (docs/RUST-MIGRATION.md rule 10). So the api cannot read one; it asks this
// host, over loopback with the internal credential, for two things:
//
//   GET  /api/app-host/harnesses                          what each app ships
//   POST /api/app-host/<slug>/harnesses/<id>/cases/<name>  replay one fixture
//
// and registers the first beside its builtins (talaria-app-harnesses), so the
// model-fitness matrix gets a column per app harness and the sweep scores its
// fixtures through the second.
//
// RESTORED FROM THE TS REGISTRY d79b7290 DELETED (`harness/registry.ts`, its
// app layer): same discovery shape — one harness per FILE, enabled apps only,
// sorted so every process merges the same way — and the same rule that A
// BROKEN APP IS A SKIPPED HARNESS, never an outage and never a disabled app.
// What changed is the merge: an app may no longer REPLACE a builtin by
// reusing its id. The builtins are Rust's now, and a third-party definition
// standing in for the judge's or the titler's column would score the
// platform's own slot with somebody else's fixtures.
//
// THE CHECKS ARE THE ONES RUST HOLDS ITS OWN HARNESSES TO
// (api/crates/talaria-harness-defs/src/registry.rs, its test module), applied
// at load instead of in a test because an app is not built with this repo:
// guard rule ids that exist, a floor that is a subset of `requires` and empty
// unless it refuses, the JSON floor derived exactly as `defineHarness` derives
// it, fixture names that are distinct, a widening with a reason, a
// temperature in range, and an id that is unique and not a builtin's. A
// harness that fails one is skipped, logged, and reported on the app's build
// status (Manage → Apps); the app keeps running.
import { RULES } from './guardrails'
import type { Capability } from './harness/capability'
import { defineHarness, isGap, NO_TOOLS, type EvalBand, type EvalContext, type HarnessDefinition } from './harness/define'
import { MAX_LOOP_TURNS, loopTurnsOf, runHarness, type HarnessDeps, type HarnessResult, type HarnessRunRow, type LoopCall } from './harness/run'
import { defaultTransport, type Transport } from './harness/transport'
import { enabledApps } from './apps'
import { withApp } from './app-scope'
import { loadAppHarnesses, type AppHarnessFile } from './app-load'
import { readCurrent, writeCurrent } from './app-build/status'
import { appHarnessSlot } from './app-harness-models'

/** The ledger's chars/4 fallback (talaria-gateway `estimate_tokens`), for a
 *  replay whose reply carried no usage — so the fitness page and the invoice
 *  estimate one call the same way. */
const estimateTokens = (chars: number): number => Math.ceil(chars / 4)

/** The ids the api's builtin registry ships (`EXPECTED_IDS` in
 *  api/crates/talaria-harness-defs/src/registry.rs). An app harness may not
 *  take one: the builtins are not replaceable from an app. `app-harnesses.test.ts`
 *  reads the Rust list and fails when this one drifts from it. */
export const BUILTIN_HARNESS_IDS: readonly string[] = [
  'titler',
  'summarizer',
  'librarian',
  'blurb-writer',
  'distiller',
  'concluder',
  'muse:cron',
  'muse:agent',
  'muse:ticket',
  'muse:skill-form',
  'muse:template-form',
  'muse:draft',
  'judge',
  'ticket-relevance',
  'inbox-brief',
  'inbox-command',
  'inbox-reply',
  'briefer:daily-open',
  'briefer:daily-delta',
  'briefer:reply',
  'work-session',
  'hermes:knowledge',
  'hermes:documents',
  'hermes:governance',
  'hermes:work',
  'hermes:google',
  'hermes:research',
  'hermes:comms',
  'hermes:authority',
  'secrets:handles',
  'workbench:light',
  'workbench:standard',
  'workbench:heavy',
  'channel-plan',
  'plan-doc',
  'outreach:check-in',
  'research-scope',
  'research-queries',
  'research-search',
  'research-synthesis',
]

/** Every capability a harness may name — the `Capability` union, spelled as a
 *  value so a definition this build did not compile can be checked against
 *  it. The record form makes a capability added to the union a type error
 *  here until it is listed. */
const CAPABILITY_SET: Record<Capability, true> = {
  json: true,
  'json-strict': true,
  tools: true,
  'tool-select': true,
  search: true,
  vision: true,
  'long-context': true,
  code: true,
  'instruction-following': true,
}
const CAPABILITIES = new Set<string>(Object.keys(CAPABILITY_SET))

/** A harness id: what `harness_runs.harness` records, a URL segment of the
 *  replay route and the key of a fitness column. Builtins spell theirs
 *  `family:job`; no spaces, no slashes. */
const ID_RE = /^[A-Za-z0-9][A-Za-z0-9:._-]{0,119}$/
const MAX_CASE_NAME = 200
/** Repair turns a json harness may ask for. The runner loops that many times
 *  on a malformed reply and the api reads the count as a u32, so it is a
 *  whole number — NaN and Infinity reached the wire as null — and a small
 *  one: every turn is spend, in a fitness sweep times every fixture. The
 *  builtins ask for 0 to 2. */
const MAX_REPAIR = 5

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v)
const isFilled = (v: unknown): v is string => typeof v === 'string' && v.trim() !== ''
const isStringList = (v: unknown): v is string[] => Array.isArray(v) && v.every((s) => typeof s === 'string')
const errText = (e: unknown): string => (e instanceof Error ? e.message : String(e))

/** A definition that passed every check, with the JSON floor derived. I and O
 *  are `unknown` because that is what a registry can honestly claim about a
 *  module it did not compile; the pair is real inside the app's own module. */
export type AppHarnessDef = HarnessDefinition<unknown, unknown>

export type HarnessCheck = { ok: true; def: AppHarnessDef } | { ok: false; id: string | null; problems: string[] }

/** THE STRUCTURAL HALF — the fields the platform reads without the app in the
 *  room, from the deleted registry's `activityHarnessOf`. Structural because
 *  `defineHarness` is an identity function plus one derived floor: there is no
 *  brand to look for. Each problem is one sentence an admin can act on. */
function structuralProblems(value: Record<string, unknown>): string[] {
  const out: string[] = []
  if (!isFilled(value.id)) out.push('it has no id')
  else if (!ID_RE.test(value.id)) out.push(`its id "${value.id}" may use only letters, digits and : . _ - (at most 120)`)
  if (!isFilled(value.label)) out.push('it has no label')
  if (!isFilled(value.job)) out.push('it has no job line for the admin panel')
  if (typeof value.render !== 'function') out.push('it has no render function')
  if (!isStringList(value.requires)) out.push('requires is not a list of capabilities')
  const model = value.model
  if (!isObject(model)) out.push('it has no model spec')
  else {
    // wireOf copies these onto the listing; a chain that is not a list threw
    // there and took every app's harnesses out of the fitness plane.
    if (model.pin != null && typeof model.pin !== 'string') out.push('its model pin is not a platform slot name')
    if (model.role != null && typeof model.role !== 'string') out.push('its model role is not a role name')
    if (model.chain != null && !isStringList(model.chain)) out.push('its model chain is not a list')
  }
  const floor = value.floor
  if (!isObject(floor) || !isStringList(floor.capabilities) || typeof floor.refuseBelow !== 'boolean' || typeof floor.note !== 'string') {
    out.push('its floor is not { capabilities, refuseBelow, note }')
  } else if (floor.suppliable !== undefined && !isStringList(floor.suppliable)) {
    out.push('floor.suppliable is not a list of capabilities')
  }
  const output = value.output
  if (!isObject(output)) out.push('it has no output contract')
  else if (output.kind === 'json') {
    // The runner hands the parsed value to `schema.safeParse` and nothing
    // else, so that method IS the schema as far as this check is concerned.
    if (!isObject(output.schema) || typeof (output.schema as { safeParse?: unknown }).safeParse !== 'function') {
      out.push('its json output has no schema to parse with')
    }
    const repair = output.repair
    if (repair !== undefined && !(typeof repair === 'number' && Number.isInteger(repair) && repair >= 0 && repair <= MAX_REPAIR)) {
      out.push(`its repair ${String(repair)} is not a whole number of turns from 0 to ${MAX_REPAIR}`)
    }
  } else if (output.kind !== 'text') {
    out.push(`its output kind ${JSON.stringify(output.kind)} is neither text nor json`)
  }
  const onFailure = value.onFailure
  if (!(onFailure === 'null' || onFailure === 'throw' || (isObject(onFailure) && ('fallback' in onFailure || onFailure.escalate === true)))) {
    out.push("its onFailure is not 'null', 'throw', { fallback } or { escalate: true }")
  }
  if (value.widen !== undefined && (!isObject(value.widen) || !isStringList(value.widen.requires))) out.push('it widens on nothing')
  if (value.loop !== undefined) out.push(...loopProblems(value.loop, value.toolDefs, value.tools))
  if (value.evals !== undefined) {
    // Validated as a WHOLE: a fixture with no check would take a column in the
    // org's fitness matrix and fill it with "check is not a function", which
    // reads to an admin as "this model failed" rather than "this app is broken".
    if (!Array.isArray(value.evals)) out.push('evals is not a list')
    else if (!value.evals.every((c) => isObject(c) && isFilled(c.name) && typeof c.check === 'function' && 'input' in c)) {
      out.push('a fixture has no name, no input or no check')
    }
  }
  return out
}

/** A tool name a provider accepts as a function name. */
const TOOL_NAME_RE = /^[A-Za-z0-9_-]{1,64}$/
/** `org: '<server>/<tool>'`. */
const ORG_TOOL_RE = /^[^/\s]+\/[^/\s]+$/
const MAX_LOOP_TOOLS = 32

/** A TOOL LOOP THE RUNNER CAN HOLD: a list of tools, each a definition the
 *  model can be offered and exactly one place its call goes, and a turn bound
 *  inside the runner's ceiling. Checked as a whole at load, because a loop
 *  that is wrong at call time is a model turn spent on a tool that cannot run. */
function loopProblems(loop: unknown, toolDefs: unknown, tools: unknown): string[] {
  if (!isObject(loop)) return ['its loop is not { tools, maxTurns? }']
  const out: string[] = []
  if (tools === 'own') out.push("it declares both a loop and tools: 'own' — a turn has one loop")
  const maxTurns = loop.maxTurns
  if (maxTurns !== undefined && !(typeof maxTurns === 'number' && Number.isInteger(maxTurns) && maxTurns >= 1 && maxTurns <= MAX_LOOP_TURNS)) {
    out.push(`its loop's maxTurns ${String(maxTurns)} is not a whole number of turns from 1 to ${MAX_LOOP_TURNS}`)
  }
  if (!Array.isArray(loop.tools) || loop.tools.length === 0) return [...out, 'its loop offers no tools']
  if (loop.tools.length > MAX_LOOP_TOOLS) out.push(`its loop offers ${loop.tools.length} tools; at most ${MAX_LOOP_TOOLS}`)
  const offered = new Set(Array.isArray(toolDefs) ? toolDefs.flatMap((d) => (isObject(d) && typeof d.name === 'string' ? [d.name] : [])) : [])
  const seen = new Set<string>()
  for (const [i, t] of loop.tools.entries()) {
    if (!isObject(t)) {
      out.push(`loop tool ${i} is not a tool`)
      continue
    }
    const name = typeof t.name === 'string' ? t.name : null
    const label = name ? `loop tool "${name}"` : `loop tool ${i}`
    if (!name || !TOOL_NAME_RE.test(name)) out.push(`${label} needs a name of letters, digits, _ and - (at most 64)`)
    else if (seen.has(name)) out.push(`two loop tools are named "${name}"`)
    else if (offered.has(name)) out.push(`${label} is also offered in toolDefs`)
    if (name) seen.add(name)
    if (!isFilled(t.description)) out.push(`${label} has no description for the model`)
    if (!isObject(t.parameters)) out.push(`${label} has no parameters schema`)
    const roads = [typeof t.handler === 'function', t.mcp !== undefined, t.org !== undefined].filter(Boolean).length
    if (roads !== 1) out.push(`${label} must name exactly one of handler, mcp or org`)
    if (t.mcp !== undefined && !isFilled(t.mcp)) out.push(`${label}'s mcp is not one of the app's tool names`)
    if (t.org !== undefined && !(typeof t.org === 'string' && ORG_TOOL_RE.test(t.org))) out.push(`${label}'s org is not '<server>/<tool>'`)
    if (t.handler !== undefined && typeof t.handler !== 'function') out.push(`${label}'s handler is not a function`)
    if (t.simulate !== undefined && typeof t.simulate !== 'function') out.push(`${label}'s simulate is not a function`)
  }
  return out
}

/** THE PLATFORM'S RULES, over a definition that is structurally whole and has
 *  its JSON floor derived — the assertions registry.rs makes of every builtin. */
function platformProblems(def: AppHarnessDef): string[] {
  const out: string[] = []
  if (!isFilled(def.floor.note)) out.push('its floor has no note for the admin choosing a model')
  for (const cap of [...def.requires, ...def.floor.capabilities, ...(def.floor.suppliable ?? []), ...(def.widen?.requires ?? [])]) {
    if (!CAPABILITIES.has(cap)) out.push(`"${cap}" is not a capability Talaria measures`)
  }
  // A typo in a rule id does not disable one rule — `narrowGuardConfig` turns
  // a rule on only when the harness names it, so it can disable all of them.
  const known = new Set(RULES.map((r) => r.id))
  const guard = def.guard
  if (guard !== undefined && !isObject(guard)) out.push('its guard is not { rules?, redact? }')
  else if (guard?.rules !== undefined) {
    if (!isStringList(guard.rules)) out.push('guard.rules is not a list of rule ids')
    else for (const rule of guard.rules) if (!known.has(rule)) out.push(`guard rule "${rule}" does not exist`)
  }
  // The floor is the non-negotiable SUBSET of `requires`, and a floor that
  // never refuses is a comment that reads like a requirement.
  for (const cap of def.floor.capabilities) {
    if (!def.requires.includes(cap)) {
      out.push(
        cap === 'json' && def.output.kind === 'json'
          ? 'a json harness refuses on "json" (the derived floor) without requiring it — add "json" to requires'
          : `the floor refuses on "${cap}" without requiring it`,
      )
    }
  }
  if (!def.floor.refuseBelow && def.floor.capabilities.length > 0) out.push('the floor lists capabilities but never refuses on them')
  if (def.widen !== undefined) {
    if (!isObject(def.widen) || !isStringList(def.widen.requires) || def.widen.requires.length === 0) out.push('it widens on nothing')
    else if (!isFilled(def.widen.note)) out.push('its widening has no note')
  }
  if (def.temperature !== undefined && !(typeof def.temperature === 'number' && def.temperature >= 0 && def.temperature <= 1)) {
    out.push(`it asks for temperature ${String(def.temperature)}, outside 0–1`)
  }
  if (def.tools !== undefined && def.tools !== 'none' && def.tools !== 'own') out.push(`its tool policy ${JSON.stringify(def.tools)} is neither 'none' nor 'own'`)
  const names = (def.evals ?? []).map((e) => e.name)
  for (const n of names) if (n.length > MAX_CASE_NAME) out.push(`fixture "${n.slice(0, 40)}…" has a name over ${MAX_CASE_NAME} characters`)
  const dup = names.find((n, i) => names.indexOf(n) !== i)
  if (dup !== undefined) out.push(`two fixtures are named "${dup}" — the sweep records and resumes by name`)
  for (const e of def.evals ?? []) {
    if (e.band !== undefined && e.band !== 'easy' && e.band !== 'standard' && e.band !== 'hard') out.push(`fixture "${e.name}" has band ${JSON.stringify(e.band)}`)
  }
  return out
}

/** Check one module's default export. Pure: no ids from other apps, no
 *  builtins — the listing below adds those. Never throws: a definition shaped
 *  so oddly that checking it throws is a problem on that one harness. */
export function checkAppHarness(value: unknown): HarnessCheck {
  if (!isObject(value)) return { ok: false, id: null, problems: ['it does not default-export a harness (defineHarness)'] }
  const id = isFilled(value.id) ? value.id : null
  try {
    const structural = structuralProblems(value)
    if (structural.length > 0) return { ok: false, id, problems: structural }
    // THE JSON FLOOR, DERIVED — the same function the SDK's defineHarness is, so
    // a definition written without it is held to the floor it would have had.
    const def = defineHarness(value as unknown as AppHarnessDef)
    const problems = platformProblems(def)
    return problems.length > 0 ? { ok: false, id, problems } : { ok: true, def }
  } catch (e) {
    return { ok: false, id, problems: [`it could not be checked: ${errText(e)}`] }
  }
}

// ── The listing ─────────────────────────────────────────────────────────────

export interface AppHarness {
  slug: string
  /** The app's display name, for the column an admin reads. */
  appName: string
  /** The file under `harnesses/` it came from. */
  file: string
  def: AppHarnessDef
}

export interface AppHarnessProblem {
  app: string
  file: string
  id: string | null
  reason: string
}

export interface AppHarnessListing {
  harnesses: AppHarness[]
  problems: AppHarnessProblem[]
}

/** The edges the listing reads, injected — the module loader and the enabled
 *  set are what a test cannot have. */
export interface AppHarnessLayer {
  /** Enabled apps, slug and display name. */
  enabled: () => Promise<Array<{ slug: string; name: string }>>
  files: (slug: string) => Promise<AppHarnessFile[]>
  warn: (message: string) => void
}

/** `sink`, called once per distinct message for the life of the process.
 *  The api re-lists every few seconds while the fitness matrix polls, and the
 *  replay route lists once per fixture, so one broken harness logged on every
 *  listing buried the log. A skip's message names the app, the file and the
 *  problem, so a changed problem is logged again. Bounded: past `cap`
 *  distinct messages the memory starts over rather than growing. */
export function onceEach(sink: (message: string) => void, cap = 1000): (message: string) => void {
  const seen = new Set<string>()
  return (message) => {
    if (seen.has(message)) return
    if (seen.size >= cap) seen.clear()
    seen.add(message)
    sink(message)
  }
}

const REAL_LAYER: AppHarnessLayer = {
  enabled: async () => (await enabledApps()).map((a) => ({ slug: a.slug, name: a.name })),
  files: loadAppHarnesses,
  warn: onceEach((message) => console.warn(`[app-harness] ${message}`)),
}

/** EVERY ENABLED APP'S HARNESSES, checked. Apps in slug order, files in name
 *  order, so two processes always agree which of two apps claiming one id
 *  keeps it (the first). A file the app build could not compile, a file that
 *  throws on import, a default export that is not a harness, a definition
 *  that fails a check, an id a builtin or an earlier app holds — each is a
 *  problem on that one file, logged, and the rest of the app's harnesses
 *  still register. Every harness listed here goes on the wire (`wireOf`)
 *  without throwing. */
export async function listAppHarnesses(layer: Partial<AppHarnessLayer> = {}): Promise<AppHarnessListing> {
  const edges: AppHarnessLayer = { ...REAL_LAYER, ...layer }
  const apps = [...(await edges.enabled())].sort((a, b) => (a.slug < b.slug ? -1 : a.slug > b.slug ? 1 : 0))
  const builtins = new Set(BUILTIN_HARNESS_IDS)
  const owner = new Map<string, string>()
  const harnesses: AppHarness[] = []
  const problems: AppHarnessProblem[] = []
  const skip = (app: string, file: string, id: string | null, reason: string) => {
    problems.push({ app, file, id, reason })
    edges.warn(`app "${app}": harnesses/${file} skipped — ${reason}`)
  }
  for (const app of apps) {
    let files: AppHarnessFile[]
    try {
      files = await edges.files(app.slug)
    } catch (e) {
      skip(app.slug, '*', null, `the harness modules could not be listed: ${errText(e)}`)
      continue
    }
    for (const { file, load, compileError } of files) {
      if (compileError !== undefined) {
        skip(app.slug, file, null, `it failed to compile: ${compileError}`)
        continue
      }
      let mod: unknown
      try {
        mod = await load()
      } catch (e) {
        skip(app.slug, file, null, `it failed to import: ${errText(e)}`)
        continue
      }
      const checked = checkAppHarness(isObject(mod) ? mod.default : undefined)
      if (!checked.ok) {
        skip(app.slug, file, checked.id, checked.problems.join('; '))
        continue
      }
      const id = checked.def.id
      if (builtins.has(id)) {
        skip(app.slug, file, id, `"${id}" is a builtin harness's id, and an app cannot replace one`)
        continue
      }
      const holder = owner.get(id)
      if (holder !== undefined) {
        skip(app.slug, file, id, holder === app.slug ? `another file in this app already registers "${id}"` : `app "${holder}" already registers "${id}"`)
        continue
      }
      const harness: AppHarness = { slug: app.slug, appName: app.name, file, def: checked.def }
      // The checks cover every field wireOf reads; this is the backstop that
      // keeps one definition they missed from failing the whole listing route.
      try {
        wireOf(harness)
      } catch (e) {
        skip(app.slug, file, id, `it could not be listed: ${errText(e)}`)
        continue
      }
      owner.set(id, app.slug)
      harnesses.push(harness)
    }
  }
  return { harnesses, problems }
}

// ── The build status report ─────────────────────────────────────────────────

/** What one app's harnesses came to, written on its build status so Manage →
 *  Apps can say which were skipped and why. */
export interface AppHarnessReport {
  registered: string[]
  problems: string[]
}

/** `slug`'s line of a listing, as the report spells it. */
const reportOf = (listing: AppHarnessListing, slug: string): AppHarnessReport => ({
  registered: listing.harnesses.filter((h) => h.slug === slug).map((h) => h.def.id),
  problems: listing.problems.filter((p) => p.app === slug).map((p) => `harnesses/${p.file}: ${p.reason}`),
})

/** THE PROBE'S HALF: check `slug`'s harnesses as if it were enabled beside the
 *  apps that already are, and keep its own line of the result. Run by the load
 *  probe (app-health.ts) on every build and enable, so the report is there
 *  before the app is switched on. Never throws, never fails the probe. */
export async function checkAppHarnessesOf(slug: string, layer: Partial<AppHarnessLayer> = {}): Promise<AppHarnessReport> {
  const edges: AppHarnessLayer = { ...REAL_LAYER, ...layer }
  // Most apps ship no harness: nothing to check, and no settings read to ask
  // which other apps are on.
  const own = await edges.files(slug).catch(() => null)
  if (own !== null && own.length === 0) return { registered: [], problems: [] }
  const listing = await listAppHarnesses({
    ...edges,
    enabled: async () => {
      const on = await edges.enabled().catch(() => [])
      return on.some((a) => a.slug === slug) ? on : [...on, { slug, name: slug }]
    },
  })
  return reportOf(listing, slug)
}

/** Put the report on the app's current build pointer. Nothing to write
 *  without one (the probe always runs after a build wrote it), and nothing
 *  written when the pointer already says it. */
export function recordAppHarnessReport(slug: string, report: AppHarnessReport): void {
  const cur = readCurrent(slug)
  if (!cur) return
  const empty = report.registered.length === 0 && report.problems.length === 0
  if (JSON.stringify(cur.harnesses ?? null) === JSON.stringify(empty ? null : report)) return
  const next = { ...cur }
  if (empty) delete next.harnesses
  else next.harnesses = { registered: [...report.registered], problems: [...report.problems] }
  writeCurrent(slug, next)
}

/** EVERY ENABLED APP'S REPORT, rewritten from one listing. A report the load
 *  probe wrote is true of the enabled set it saw: when an app that sorts
 *  first is enabled later and claims the same id, it takes the id, and the
 *  app that held it still read "N harnesses" with nothing skipped. So this
 *  runs whenever the set changes — after a live load probe, when a failed
 *  app is switched off (app-health.ts), and when the api enables, disables
 *  or uninstalls one (`POST /api/app-host/harnesses`). A disabled app keeps
 *  its last report; Manage → Apps says its harnesses are not registered
 *  while it is off. Returns the reports written, by slug. */
export async function refreshAppHarnessReports(
  layer: Partial<AppHarnessLayer> = {},
  record: (slug: string, report: AppHarnessReport) => void = recordAppHarnessReport,
): Promise<Map<string, AppHarnessReport>> {
  const edges: AppHarnessLayer = { ...REAL_LAYER, ...layer }
  const apps = await edges.enabled()
  const listing = await listAppHarnesses({ ...edges, enabled: async () => apps })
  const reports = new Map<string, AppHarnessReport>()
  for (const { slug } of apps) {
    const report = reportOf(listing, slug)
    reports.set(slug, report)
    record(slug, report)
  }
  return reports
}

// ── The wire ────────────────────────────────────────────────────────────────

/** One app harness as the api reads it — data only, never code. Rust's
 *  `talaria_app_harnesses::AppHarnessWire` is the other end. */
export interface AppHarnessWire {
  id: string
  label: string
  job: string
  source: `app:${string}`
  app: string
  appName: string
  file: string
  requires: string[]
  floor: { capabilities: string[]; refuseBelow: boolean; note: string }
  outputKind: 'text' | 'json'
  /** Repair turns a malformed reply may get: the json default of one, or
   *  what the harness declared. Always 0 for text. */
  repair: number
  verifies: boolean
  tools: 'none' | 'own'
  guard: { rules: string[] | null; redact: boolean } | null
  widen: { requires: string[]; note: string } | null
  temperature: number | null
  /** `slot` is the harness's own model setting, `app:<slug>/<id>` — what an
   *  admin assigns in Admin → Models → Fitness, resolved before `role`. Set
   *  by the host from the slug and the id; the app's own value is never read. */
  model: { pin: string | null; role: string | null; chain: string[] | null; slot: string }
  /** The runner-held tool loop, as data: the turn bound and, per tool, where
   *  its call goes and whether it can answer a dry run (`simulate`). The api
   *  dry-runs an app harness only when every tool can. */
  loop: { maxTurns: number; tools: Array<{ name: string; via: 'handler' | 'mcp' | 'org'; simulate: boolean }> } | null
  evals: Array<{ name: string; band: EvalBand }>
}

export function wireOf(h: AppHarness): AppHarnessWire {
  const d = h.def
  return {
    id: d.id,
    label: d.label,
    job: d.job,
    source: `app:${h.slug}`,
    app: h.slug,
    appName: h.appName,
    file: h.file,
    requires: [...d.requires],
    floor: { capabilities: [...d.floor.capabilities], refuseBelow: d.floor.refuseBelow, note: d.floor.note },
    outputKind: d.output.kind,
    repair: d.output.kind === 'json' ? Math.max(0, Math.floor(d.output.repair ?? 1)) : 0,
    verifies: d.output.verify !== undefined,
    tools: d.tools ?? 'none',
    guard: d.guard ? { rules: d.guard.rules ? [...d.guard.rules] : null, redact: d.guard.redact === true } : null,
    widen: d.widen ? { requires: [...d.widen.requires], note: d.widen.note } : null,
    temperature: d.temperature ?? null,
    model: { pin: d.model.pin ?? null, role: d.model.role ?? null, chain: d.model.chain ? [...d.model.chain] : null, slot: appHarnessSlot(h.slug, d.id) },
    loop: d.loop
      ? {
          maxTurns: loopTurnsOf(d.loop.maxTurns),
          tools: d.loop.tools.map((t) => ({
            name: t.name,
            via: typeof t.handler === 'function' ? ('handler' as const) : t.mcp !== undefined ? ('mcp' as const) : ('org' as const),
            simulate: typeof t.simulate === 'function',
          })),
        }
      : null,
    evals: (d.evals ?? []).map((e) => ({ name: e.name, band: e.band ?? 'standard' })),
  }
}

// ── Replaying one fixture ───────────────────────────────────────────────────

/** One fixture, replayed once against a pinned model — what the api's sweep
 *  turns into an `EvalCaseScore`. The run is the real runner with two edges
 *  swapped: `recordRun` and `recordFindings` capture instead of filing, the
 *  same rule the Rust sweep keeps (a benchmark must not write into the
 *  `harness_runs` and `guard_findings` it is compared against). Token spend is
 *  real spend: the relay books it as the app's, and the counts below are the
 *  provider's own, off the reply. */
export interface CaseReplay {
  harness: string
  case: string
  band: EvalBand
  /** The model asked, which the runner pins — never resolved. */
  model: string
  value: unknown
  /** The fixture's own verdict: pass, fail, or `gap` (the fixture could not
   *  fairly ask). Null when there was no value to grade: the contract failed,
   *  or the floor refused. */
  verdict: 'pass' | 'fail' | 'gap' | null
  /** The fixture's sentence for a fail or a gap. */
  reason: string | null
  schemaValid: boolean
  repairs: number
  answered: boolean
  /** The capability floor declined to ask: a skip, never a failure. */
  refused: boolean
  error: string | null
  latencyMs: number
  /** Guard findings this run is evidence for (`harness_runs.findings`). */
  findings: number
  raw: string | null
  prompt: string
  promptTokens: number
  completionTokens: number
  /** The token counts are a chars/4 estimate: the reply carried no usage —
   *  a provider that sends none, or an agent turn whose stream reported none.
   *  A gateway reply's own counts reach here through the relay. */
  estimated: boolean
  /** A loop harness's dispatched calls — every tool answered by its
   *  `simulate` — in order; empty for a single-shot one. */
  calls: LoopCall[]
  /** The loop hit its turn bound with the model still calling tools. */
  exhausted: boolean
}

export interface ReplayOptions {
  signal?: AbortSignal
  /** The test seam: a scripted transport, a stub model resolution. */
  deps?: Partial<HarnessDeps>
}

/** Replay the fixture `name` of `h` against `model`. Null when the harness
 *  declares no such fixture. */
export async function replayAppCase(h: AppHarness, name: string, model: string, opts: ReplayOptions = {}): Promise<CaseReplay | null> {
  const fixture = (h.def.evals ?? []).find((e) => e.name === name)
  if (!fixture) return null
  const capture = { row: null as HarnessRunRow | null, prompt: '', promptTokens: 0, completionTokens: 0, estimated: false }
  const base: Transport = opts.deps?.transport ?? defaultTransport
  const transport: Transport = async (req) => {
    capture.prompt = req.messages.map((m) => `${m.role}: ${m.content}`).join('\n\n')
    const reply = await base(req)
    if (reply.usage) {
      capture.promptTokens += reply.usage.promptTokens
      capture.completionTokens += reply.usage.completionTokens
    } else {
      // The ledger's own chars/4 fallback, so the fitness page and the
      // invoice estimate one call the same way.
      capture.promptTokens += estimateTokens(capture.prompt.length)
      capture.completionTokens += estimateTokens(reply.text.length)
      capture.estimated = true
    }
    return reply
  }
  let result: HarnessResult<unknown> | null = null
  let threw: string | null = null
  try {
    // AS THE APP: the replay's calls go to the gateway on the app's key and
    // land in its ledger as `app` spend, under this stamp.
    result = await withApp(h.slug, () =>
      runHarness(h.def, fixture.input, {
        caller: `app:${h.slug}:fitness:${h.def.id}`,
        model,
        // A DRY RUN: a loop tool answers through its `simulate`, never for real.
        // A fitness sweep must not send a candidate's tool calls to the app's
        // handlers, its MCP tools or the org's.
        loop: { app: h.slug, actingUserId: null, simulate: true },
        ...(opts.signal ? { signal: opts.signal } : {}),
        deps: {
          ...opts.deps,
          transport,
          recordRun: async (row) => {
            capture.row = row
          },
          recordFindings: async () => {},
        },
      }),
    )
  } catch (e) {
    // `onFailure: 'throw'` throws by declaration; the row was captured before
    // it did, so the case still reads what happened.
    threw = errText(e)
  }
  const schemaValid = capture.row?.schemaValid ?? result?.schemaValid ?? false
  let verdict: CaseReplay['verdict'] = null
  let reason: string | null = null
  const calls = result?.calls ?? []
  if (result && schemaValid && result.value !== null && result.value !== undefined && !result.refused) {
    try {
      const checked = fixture.check(result.value, calls.length || h.def.loop ? evalContextOf(calls, result.exhausted ?? false) : NO_TOOLS)
      if (checked === null) verdict = 'pass'
      else if (isGap(checked)) {
        verdict = 'gap'
        reason = checked.gap
      } else {
        verdict = 'fail'
        reason = checked
      }
    } catch (e) {
      // A check that throws is the FIXTURE's fault, not the model's: a gap.
      verdict = 'gap'
      reason = `the fixture's check threw: ${errText(e)}`
    }
  }
  return {
    harness: h.def.id,
    case: fixture.name,
    band: fixture.band ?? 'standard',
    model,
    value: result?.value ?? null,
    verdict,
    reason,
    schemaValid,
    repairs: capture.row?.repairs ?? result?.repairs ?? 0,
    answered: result?.answered ?? false,
    refused: result?.refused ?? false,
    error: threw ?? capture.row?.error ?? result?.error ?? null,
    latencyMs: capture.row?.latencyMs ?? result?.latencyMs ?? 0,
    findings: capture.row?.findings ?? 0,
    raw: result?.raw ?? null,
    prompt: capture.prompt,
    promptTokens: capture.promptTokens,
    completionTokens: capture.completionTokens,
    estimated: capture.estimated,
    calls,
    exhausted: result?.exhausted ?? false,
  }
}

/** What a fixture sees of a loop run: the calls, and the order they came in. */
export function evalContextOf(calls: readonly LoopCall[], exhausted: boolean): EvalContext {
  const first = (tool: string) => calls.findIndex((c) => c.tool === tool)
  return {
    calls,
    calledBefore: (a, b) => {
      const ia = first(a)
      const ib = first(b)
      return ia >= 0 && ib >= 0 && ia < ib
    },
    world: null,
    exhausted,
  }
}
