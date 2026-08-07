// THE ACTIVITY-HARNESS REGISTRY: one place that can name every harness in the
// product, so that nothing has to go looking for them file by file again.
//
// WHY THIS FILE EXISTS
//   `PLATFORM_AGENTS` (server/platform-agents.ts) is the METADATA half of this
//   registry — id, label, job, whether an admin may assign a model — and
//   `harness/defs/*.ts` is the EXECUTABLE half. Until this file the two were
//   joined only by a string an author remembered to spell the same way twice.
//   Two things are about to iterate the executable half and neither can grep:
//   the in-UI model-fitness suite replays every declared eval fixture against a
//   candidate model, and the admin panel shows each harness's floor, its
//   widening and which model is carrying it.
//
//   `registry.test.ts` holds the two halves together: it fails if a harness id
//   drifts from its platform-agent id, if a guard rule id is misspelled (which
//   silently disables EVERY rule for that harness — see `narrowGuardConfig`),
//   or if a floor declares capabilities it never refuses on.
//
// THE THREE LAYERS, same shape and same precedence as
// `workbench-harnesses.ts` `listHarnessDefs`: builtin < app-shipped <
// admin-custom, merged by id. An app that ships a harness with a builtin's id
// replaces it; nothing merges field by field, because a half-overridden prompt
// with somebody else's schema under it is not a harness anybody can reason
// about.
//
// THE PLATFORM_AGENTS CROSS-CHECK, and the two places the lists do NOT line up.
// Both are real and neither is forced:
//
//   'briefer' is a platform agent with NO harness pinning it. It is
//   `assignable: false` on purpose — the briefer is the owner's own personal
//   assistant, and its persona and privacy are the feature — so the inbox
//   harnesses take their model as an explicit `RunContext.model` from the
//   caller rather than resolving a pin. Giving them `pin: 'briefer'` would
//   invent an assignment slot the product deliberately does not offer.
//
//   'briefer' also now HAS harnesses ('briefer:brief', 'briefer:chat'); they
//   simply declare no pin, for the reason above. That is a change from the
//   sentence this header carried through phase 2, when the briefer had none.
//
//   THE CALLER-PINNED HARNESSES have no platform-agent entry, for the same
//   reason from the other side: there is nothing for an admin to assign,
//   because the model is decided by the SUBJECT of the call, not by a slot.
//   The Inbox trio and the briefer run on the owner's own assistant;
//   'work-session' runs on the agent assigned to the ticket; 'channel-plan' and
//   'plan-doc' run on the channel's or the plan's own agent (including a TIER of
//   it, picked in the Plan modal); 'outreach:check-in' runs on the agent doing
//   the reaching out. Every one of them takes its model as an explicit
//   `RunContext.model`. They still belong in this registry, because the fitness
//   suite has to be able to score them — "can this model map an instruction onto
//   an action", "can this model work a ticket" and "can this model rewrite a plan
//   document without gutting it" are exactly the questions an admin picking a
//   model for an agent needs answered.
//
//   The judge is the third exception and the interesting one: it HAS a platform
//   agent, but its model lives in `judge_config` (see the header of
//   defs/judge.ts) so its definition declares no pin. `platformAgentOf` below
//   is where that exception is written down once.
//
// A HARNESS THAT IS NOT IN `BUILTINS` IS INVISIBLE, and invisible in the two
// ways that matter most: the fitness suite cannot replay its eval fixtures, so
// every assertion its author wrote is dead code, and the admin panel cannot show
// its floor. Phase 3 landed nine definitions with 32 fixtures between them and
// registered none of them, because `registry.ts` was read-only to every agent
// that wrote one. Registering is the last step of a port, not a follow-up.
import type { Capability } from './capability'
import type { HarnessDefinition, RoleFloor } from './define'
import type { ModelSpec } from './model'
import type { PlatformAgentId } from '../platform-agents'

import { blurbWriterHarness } from './defs/blurb-writer'
import { briefingChatHarness, briefingHarness } from './defs/briefer'
import { channelPlanHarness } from './defs/channel-plan'
import { concluderHarness } from './defs/concluder'
import { distillerHarness } from './defs/distiller'
import { inboxBriefHarness, inboxCommandHarness, inboxReplyHarness } from './defs/inbox-focus'
import { judgeHarness } from './defs/judge'
import { librarianHarness } from './defs/librarian'
import { museAgentHarness, museCronHarness, museDraftHarness, museTicketHarness } from './defs/muse'
import { outreachCheckInHarness } from './defs/outreach'
import { planDocHarness } from './defs/plan-doc'
import { researchQueriesHarness, researchSearchHarness, researchSynthesisHarness } from './defs/research'
import { summarizerHarness } from './defs/summarizer'
import { titlerHarness } from './defs/titler'
import { workSessionHarness } from './defs/work-session'

export type HarnessSource = 'builtin' | `app:${string}` | 'custom'

/** One harness as the registry hands it out.
 *
 *  The metadata is FLAT and type-erased, because every consumer of it (the
 *  admin panel, the fitness matrix header, an audit script) wants to read the
 *  same fields off a heterogeneous list. The definition itself cannot be
 *  erased the same way — `render` takes an I and `evals[].check` takes an O, so
 *  a `HarnessDefinition<unknown, unknown>` would accept no real harness — which
 *  is what `use` is for: it applies a generic function to the definition with
 *  its I and O still paired, which is the only way to run a harness against its
 *  own fixtures without an `any` in the middle. */
export interface RegisteredHarness {
  id: string
  label: string
  job: string
  source: HarnessSource
  requires: Capability[]
  floor: RoleFloor
  model: ModelSpec
  outputKind: 'text' | 'json'
  widen: { requires: Capability[]; note: string } | null
  guard: { rules?: string[]; redact?: boolean } | null
  temperature: number | null
  /** Fixture names only — the inputs are typed and stay behind `use`. */
  evalNames: string[]
  /** Apply `fn` to the definition with its input and output types intact.
   *
   *      harness.use((def) => runHarness(def, def.evals![0]!.input, ctx))
   *
   *  The closure remembers the concrete types; the list does not have to. */
  use: <R>(fn: <I, O>(def: HarnessDefinition<I, O>) => R) => R
}

function register<I, O>(def: HarnessDefinition<I, O>, source: HarnessSource): RegisteredHarness {
  return {
    id: def.id,
    label: def.label,
    job: def.job,
    source,
    requires: def.requires,
    floor: def.floor,
    model: def.model,
    outputKind: def.output.kind,
    widen: def.widen ?? null,
    guard: def.guard ?? null,
    temperature: def.temperature ?? null,
    evalNames: (def.evals ?? []).map((e) => e.name),
    use: (fn) => fn(def),
  }
}

/** The harnesses Talaria ships. Order is the order the admin panel shows them
 *  in, in two blocks: first the ones an admin ASSIGNS a model to, then the ones
 *  whose model is decided by the subject of the call. An admin reading the panel
 *  top to bottom therefore reads "here is what you control" before "here is what
 *  your agents are doing with the models you already gave them". */
const BUILTINS: RegisteredHarness[] = [
  // ── Assigned in Admin ──────────────────────────────────────────────────────
  register(titlerHarness, 'builtin'),
  register(summarizerHarness, 'builtin'),
  register(librarianHarness, 'builtin'),
  register(blurbWriterHarness, 'builtin'),
  register(distillerHarness, 'builtin'),
  register(concluderHarness, 'builtin'),
  register(museCronHarness, 'builtin'),
  register(museAgentHarness, 'builtin'),
  register(museTicketHarness, 'builtin'),
  register(museDraftHarness, 'builtin'),
  register(judgeHarness, 'builtin'),
  // ── The model comes from the subject of the call ───────────────────────────
  // The owner's own personal assistant: the Inbox trio and the two briefer
  // surfaces.
  register(inboxBriefHarness, 'builtin'),
  register(inboxCommandHarness, 'builtin'),
  register(inboxReplyHarness, 'builtin'),
  register(briefingHarness, 'builtin'),
  register(briefingChatHarness, 'builtin'),
  // The agent assigned to the ticket, the channel or the plan — including a
  // TIER of it, which the Plan modal lets a user pick per draft.
  register(workSessionHarness, 'builtin'),
  register(channelPlanHarness, 'builtin'),
  register(planDocHarness, 'builtin'),
  register(outreachCheckInHarness, 'builtin'),
  // The researching agent, and one of the TWO harnesses in the tree that REFUSE
  // below their floor (research-search, on 'search'; the judge is the other, on
  // json/json-strict/instruction-following) — a model with no web search does
  // not error, it answers from memory and the brief comes out confident and
  // uncited.
  register(researchQueriesHarness, 'builtin'),
  register(researchSearchHarness, 'builtin'),
  register(researchSynthesisHarness, 'builtin'),
]

// ── The app-shipped layer ────────────────────────────────────────────────────

/** Build-time discovery, the SAME shape `workbench-harnesses.ts` uses for
 *  `apps/*\/harness.ts` and `apps.ts` uses for `apps/*\/talaria.json`: a glob of
 *  lazy loaders, keyed by path. One harness per FILE (`harnesses/*.ts`, plural
 *  directory) rather than one per app, because an app with three model calls to
 *  make has three contracts to declare and cramming them into one module's
 *  default export would need an array-or-object convention nothing else here
 *  has. */
const APP_HARNESS_MODS = import.meta.glob('../../../../apps/*/harnesses/*.ts') as Record<string, () => Promise<unknown>>

const appSlugOf = (path: string): string => /apps\/([^/]+)\//.exec(path)?.[1] ?? path

/** The edges the app layer reads, injected.
 *
 *  `import.meta.glob` is a BUILD-TIME constant and `enabled` reaches the
 *  settings row through a database, so a loader reachable only through those two
 *  is a loader nothing can exercise until the day an app ships one — which is
 *  precisely when a wrong loader is most expensive. `registry.apps.test.ts`
 *  drives this with a fixture app instead. */
export interface AppHarnessLayer {
  /** Path -> loader. Exactly what `import.meta.glob` returns. */
  modules: Record<string, () => Promise<unknown>>
  /** Slugs of the apps an admin has switched on. */
  enabled: () => Promise<Set<string>>
  /** Where a rejected definition goes. Not a throw: see `listActivityHarnesses`. */
  warn: (message: string) => void
}

const REAL_LAYER: AppHarnessLayer = {
  modules: APP_HARNESS_MODS,
  // Imported lazily, exactly as `workbench-harnesses.ts` does it — this module
  // is read by the fitness suite and the admin panel, and a static import of
  // the app registry would drag the settings store into every one of them.
  enabled: async () => {
    const { enabledApps } = await import('../apps')
    return new Set((await enabledApps()).map((a) => a.slug))
  },
  warn: (message) => console.warn(`[harness] ${message}`),
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v)
const isFilled = (v: unknown): v is string => typeof v === 'string' && v.trim() !== ''

/** THE MINIMAL ACCEPTANCE CHECK for a definition this build did not write —
 *  `workbench-harnesses.ts`'s `def.invoke && def.guide`, in the shape an
 *  ACTIVITY harness has.
 *
 *  It is structural because `defineHarness` is an identity function: there is no
 *  brand to look for, and there should not be — an app author must be able to
 *  build a definition without importing anything that boots Talaria.
 *
 *  WHAT IT IS PROTECTING. Every field checked here is one the platform reads
 *  WITHOUT the app in the room: the admin panel prints `label`, `job` and
 *  `floor.note`; `runHarness` calls `render` and consults `floor` and `model`;
 *  the fitness sweep replays `evals` and scores `output`. A definition missing
 *  any of them does not fail in the app's own feature, it fails in a platform
 *  surface listing all 23 harnesses — so it never enters the list.
 *
 *  `evals` is validated as a WHOLE and rejects the definition rather than being
 *  dropped: a harness whose fixtures are malformed would take a column in the
 *  org's fitness matrix and fill it with `check is not a function`, which reads
 *  to an admin as "this model failed" rather than "this app is broken". */
function activityHarnessOf(value: unknown): HarnessDefinition<unknown, unknown> | null {
  if (!isObject(value)) return null
  if (!isFilled(value.id) || !isFilled(value.label) || !isFilled(value.job)) return null
  if (typeof value.render !== 'function') return null
  if (!Array.isArray(value.requires) || !value.requires.every((c) => typeof c === 'string')) return null
  if (!isObject(value.model)) return null

  const floor = value.floor
  if (!isObject(floor) || !Array.isArray(floor.capabilities) || typeof floor.refuseBelow !== 'boolean' || typeof floor.note !== 'string') return null

  const output = value.output
  if (!isObject(output)) return null
  if (output.kind === 'json') {
    // The runner hands the parsed value to `schema.safeParse` (json.ts) and
    // nothing else, so that method IS the schema as far as this registry is
    // concerned.
    if (!isObject(output.schema) || typeof (output.schema as { safeParse?: unknown }).safeParse !== 'function') return null
  } else if (output.kind !== 'text') {
    return null
  }

  const onFailure = value.onFailure
  if (!(onFailure === 'null' || onFailure === 'throw' || (isObject(onFailure) && ('fallback' in onFailure || onFailure.escalate === true)))) return null

  if (value.evals !== undefined) {
    if (!Array.isArray(value.evals)) return null
    if (!value.evals.every((c) => isObject(c) && isFilled(c.name) && typeof c.check === 'function' && 'input' in c)) return null
  }

  // The one assertion in the file, and the honest end of a structural check on a
  // module this build did not compile against these types. `unknown` for both I
  // and O is what the registry can actually claim: the pair is real and paired
  // inside the app's own module, and `use` hands it back still paired to anyone
  // who has a generic function to apply.
  return value as unknown as HarnessDefinition<unknown, unknown>
}

/** APP-SHIPPED HARNESSES — `apps/<slug>/harnesses/*.ts`, enabled apps only.
 *
 *  A BROKEN APP IS A SKIPPED APP, never an outage. An app whose module throws on
 *  import (a bad top-level `await`, a missing dependency after a half-finished
 *  install) and an app whose default export is not a harness are both a logged
 *  skip, because the caller of this function is the fitness matrix or the admin
 *  panel enumerating all 23 platform harnesses — and one third-party app must
 *  not be able to empty that page.
 *
 *  Paths are walked in sorted order so that two apps declaring the SAME id merge
 *  the same way on every process. Within the layer the later path wins, which is
 *  arbitrary but stable; across layers the precedence is the documented one. */
async function appHarnesses(layer: AppHarnessLayer): Promise<RegisteredHarness[]> {
  const paths = Object.keys(layer.modules).sort()
  // No app ships one on most installs, and the enablement read costs a settings
  // row. Nothing to load means nothing to ask about.
  if (paths.length === 0) return []
  const enabled = await layer.enabled()
  const out: RegisteredHarness[] = []
  for (const path of paths) {
    const app = appSlugOf(path)
    if (!enabled.has(app)) continue
    const load = layer.modules[path]
    if (!load) continue
    let mod: unknown = null
    try {
      mod = await load()
    } catch (err) {
      layer.warn(`app "${app}": ${path} failed to import, skipping — ${err instanceof Error ? err.message : String(err)}`)
      continue
    }
    const def = activityHarnessOf(isObject(mod) ? mod.default : null)
    if (!def) {
      layer.warn(`app "${app}": ${path} does not default-export an activity harness (defineHarness), skipping`)
      continue
    }
    out.push(register(def, `app:${app}`))
  }
  return out
}

/** ADMIN-CUSTOM HARNESSES — the layer that is deliberately empty, and the
 *  reason is worth stating rather than leaving as a TODO.
 *
 *  A workbench harness is DECLARATIVE (a command line, an env map, a guide
 *  string), so `workbench_harness_defs` can hold one as a JSON row and
 *  `listHarnessDefs` can trust it. An activity harness is CODE: `render`
 *  builds messages, `output.clean` parses a reply, `evals[].check` asserts on a
 *  value. Talaria does not run code out of a database row and this file is not
 *  the place to start.
 *
 *  So the custom layer, when it exists, is an app-shipped definition or a
 *  reviewed bundle — not a form in Admin. What an admin CAN customize today is
 *  the model each harness runs on, which lives in `platform_agent_models` and
 *  is applied by `harness/model.ts`, one layer down from here. */
async function customHarnesses(): Promise<RegisteredHarness[]> {
  return []
}

/** The merged registry — builtin < app-shipped < admin-custom, by id.
 *
 *  THE OUTER `catch` IS THE LAST GUARD, under the per-app one: a definition that
 *  fails to load is skipped inside `appHarnesses`, and if the layer ITSELF fails
 *  (the settings read for enablement, say) the platform's own harnesses are
 *  still returned. The fitness matrix showing 23 harnesses and a logged warning
 *  is a working product; the same page showing an error because one app is
 *  broken is not. */
export async function listActivityHarnesses(layer: Partial<AppHarnessLayer> = {}): Promise<RegisteredHarness[]> {
  const byId = new Map<string, RegisteredHarness>()
  for (const h of BUILTINS) byId.set(h.id, h)
  for (const h of await appHarnesses({ ...REAL_LAYER, ...layer }).catch(() => [])) byId.set(h.id, h)
  for (const h of await customHarnesses().catch(() => [])) byId.set(h.id, h)
  return [...byId.values()]
}

/** The builtin layer alone, synchronously. For callers that must not await —
 *  and for `registry.test.ts`, which is asserting about what Talaria SHIPS and
 *  would say nothing useful about an install's app list. */
export const builtinActivityHarnesses = (): RegisteredHarness[] => [...BUILTINS]

/** Which platform agent's model assignment drives this harness, if any.
 *
 *  `model.pin` for the assigned block above. The judge is the exception, written
 *  down here rather than rediscovered: it is a platform agent, but its model
 *  lives in `judge_config` so that the Guard panel and the Platform panel cannot
 *  disagree about which model is judging, and its definition therefore declares
 *  no pin.
 *
 *  Null means nothing in Admin assigns this harness a model, which is now the
 *  larger half of the registry: every harness whose model comes from the subject
 *  of the call. `registry.test.ts` locks the exact list so that a harness which
 *  SHOULD have had a pin cannot join it by omission. */
export function platformAgentOf(harness: RegisteredHarness): PlatformAgentId | null {
  if (harness.id === 'judge') return 'judge'
  return harness.model.pin ?? null
}
