// Model capability records — what a given model can actually DO.
//
// Talaria has never had this type, and two live bugs share that as their root
// cause:
//
//   - `model-roles.ts` `resolveRoleModel` validates that an assignment still
//     ROUTES on the gateway, and nothing else. So an admin can point
//     `research-recon` at a model with no web search; `research.ts`
//     `searchModelFor` hands it straight to the search stages, and the run
//     produces a confident, uncited, hallucinated brief. Nothing errors, and
//     the auto-chain's careful `sonar*` preference is bypassed entirely.
//   - `llm-gateway.ts` learns unsupported parameters from upstream 400s and
//     pre-strips them on every later call. `rejectedParam` matches a bare
//     lowercase identifier, so `response_format` is strippable: a model that
//     rejects JSON mode gets the constraint silently removed, the call
//     SUCCEEDS returning free prose, and the caller feeds prose to a JSON
//     parser. There is no TTL either, so a provider that later fixes support
//     is never re-tried — a one-way ratchet on capability.
//
// This module is the missing type, and it is DATA ONLY. It probes nothing and
// calls no model. Three writers fill it: the probe suite writes `probe` facts,
// the gateway writes `learned` facts from what an upstream 400 told it, and an
// admin (or a model catalog) writes `declared`.
//
// The cardinal rule lives in `missingCapabilities`: UNKNOWN IS NOT FALSE.
// Talaria has to keep working on a model nobody has benchmarked yet.

import { getSetting, setSetting } from '../audit'

export type Capability =
  | 'json' // honors response_format json_object
  | 'json-strict' // produces schema-conformant nested JSON reliably
  | 'tools' // OpenAI-style tool calling at all
  | 'tool-select' // picks the RIGHT tool from several
  | 'search' // has live web search
  | 'vision'
  | 'long-context'
  | 'code'
  | 'instruction-following' // honors "reply with exactly X"

/** WHERE A FACT CAME FROM, and the four are RANKED — see `SOURCE_RANK`.
 *
 *  'probe'     the platform ran the model and watched. The strongest evidence
 *              there is, and the only kind that may contradict a catalog.
 *  'declared'  a HUMAN said so, in Admin. A person overriding the platform is
 *              exercising judgement the platform does not have, so it outranks
 *              everything except a measurement.
 *  'catalog'   the PROVIDER said so, in its published model list. Free, wide,
 *              and only ever positive (see `capabilitiesFromCatalog`) — a
 *              catalog that omits a field has not denied anything.
 *  'learned'   inferred from an upstream 400. One data point about one
 *              parameter on one endpoint, which is why it is the weakest and the
 *              only one that expires. */
export type CapabilitySource = 'probe' | 'declared' | 'catalog' | 'learned'

export interface CapabilityFact {
  value: boolean
  source: CapabilitySource
  at: string // ISO
  /** One human sentence: what was observed. Shown in the admin UI. */
  detail?: string
  /** 0..1 for probe facts (pass rate over N trials). Absent for learned/declared. */
  score?: number
}

/** Keyed 'endpoint:model' — the same key `llm-gateway.ts` uses for its learned
 *  parameter sets, and for the same reason: capability is a property of the
 *  ENDPOINT serving the model, not of the model name. One id behind two
 *  providers (a quantized local build vs. the vendor's own API) genuinely
 *  differs in what it can hold, and a fact learned from one must never be
 *  credited to the other. */
export type CapabilityKey = string
export const capabilityKey = (endpoint: string, upstreamModel: string): CapabilityKey => `${endpoint}:${upstreamModel}`

/** Learned and catalog facts expire; probe and declared facts do not. A provider that fixes
 *  JSON mode must be able to be re-discovered without an admin action — that is
 *  the release valve on the gateway's one-way ratchet. Probe facts are a
 *  deliberate measurement and expire only when someone re-measures; declared
 *  facts are a human's word and expire only when the human changes it. */
export const LEARNED_TTL_MS = 30 * 24 * 60 * 60 * 1000

const KEY = 'model_capabilities'

// Written as exhaustive records rather than arrays so that adding a member to
// either union fails the build HERE, at the validator, instead of silently
// producing a capability the reader throws away as unrecognized.
const ALL_CAPABILITIES: Record<Capability, true> = {
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
const ALL_SOURCES: Record<CapabilitySource, true> = { probe: true, declared: true, catalog: true, learned: true }

/** PRECEDENCE, and the reason it had to exist the day the catalog became a
 *  writer. Every write used to be last-write-wins, which was correct while the
 *  writers were an admin, a probe run and a 400 — all three episodic and
 *  deliberate. A catalog refresh is neither: it runs on a daily cadence across
 *  every model an endpoint serves, so under last-write-wins it would overwrite
 *  a probe result with a provider's marketing copy every single day, and an
 *  admin who probed a model and found its tool calling broken would watch that
 *  finding evaporate overnight.
 *
 *  Equal rank still means last write wins — re-probing a model must be able to
 *  correct the previous probe. */
const SOURCE_RANK: Record<CapabilitySource, number> = { probe: 3, declared: 2, catalog: 1, learned: 0 }

/** May `next` replace `prev`? Yes when nothing is there, when it is at least as
 *  authoritative, or when what is there has expired. */
export const outranks = (next: CapabilitySource, prev: CapabilitySource | undefined): boolean =>
  prev === undefined || SOURCE_RANK[next] >= SOURCE_RANK[prev]

/** Every capability, derived from the exhaustive record above rather than
 *  re-listed. Exported because three other places used to hand-list the same
 *  nine strings — the probe registry, the probe census test and the admin
 *  page's tag order — and adding a tenth capability would have silently given
 *  it no probe and no tag. Now those lists are checked against this one. */
export const CAPABILITIES: readonly Capability[] = Object.keys(ALL_CAPABILITIES) as Capability[]

const CAPABILITY_IDS: ReadonlySet<string> = new Set(Object.keys(ALL_CAPABILITIES))
const SOURCE_IDS: ReadonlySet<string> = new Set(Object.keys(ALL_SOURCES))

const isCapability = (v: string): v is Capability => CAPABILITY_IDS.has(v)
const isSource = (v: unknown): v is CapabilitySource => typeof v === 'string' && SOURCE_IDS.has(v)

/** An unparseable `at` counts as expired rather than eternal. Dropping a
 *  learned fact costs one re-discovery; keeping a corrupt one forever is
 *  exactly the ratchet the TTL exists to release. */
const isExpired = (fact: CapabilityFact, now: number): boolean => {
  // A CATALOG FACT EXPIRES LIKE A LEARNED ONE, and for the same reason rather
  // than out of symmetry: both are re-derivable without asking anyone. If an
  // endpoint is removed, or a provider stops advertising a parameter, the claim
  // must decay on its own rather than outlive the thing that made it. The
  // refresh cadence is well inside the TTL, so a live endpoint never lapses.
  if (fact.source !== 'learned' && fact.source !== 'catalog') return false
  const at = Date.parse(fact.at)
  return !Number.isFinite(at) || now - at > LEARNED_TTL_MS
}

/** Parse one stored entry defensively. `app_settings` is JSON that outlives the
 *  code that wrote it — a hand-edited row, or a shape from a previous version,
 *  must not crash a harness run. Anything that isn't a well-formed, unexpired
 *  fact is simply not a fact, which lands the caller on the unknown path. */
function readFact(raw: unknown, now: number): CapabilityFact | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const f = raw as Record<string, unknown>
  if (typeof f.value !== 'boolean' || !isSource(f.source) || typeof f.at !== 'string') return null
  const fact: CapabilityFact = { value: f.value, source: f.source, at: f.at }
  if (typeof f.detail === 'string') fact.detail = f.detail
  if (typeof f.score === 'number' && Number.isFinite(f.score)) fact.score = f.score
  return isExpired(fact, now) ? null : fact
}

const readAll = (): Promise<Record<string, unknown>> => getSetting<Record<string, unknown>>(KEY, {})

const entryOf = (all: Record<string, unknown>, key: CapabilityKey): Record<string, unknown> => {
  const raw = all[key]
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
}

// Every write here is a read-modify-write of ONE app_settings row, and a probe
// run records nine capabilities for a model back to back. Fire those
// concurrently and the last write wins outright — eight facts vanish with no
// error anywhere. Serializing in process closes that window. The remaining
// cross-process race is accepted: probe runs are admin-triggered and singular,
// and the same read-modify-write shape is already how model roles and platform
// agent assignments are stored.
let writes: Promise<unknown> = Promise.resolve()
function serialize<T>(op: () => Promise<T>): Promise<T> {
  const next = writes.then(op, op)
  // A rejected write must not poison every write that follows it.
  writes = next.catch(() => undefined)
  return next
}

/** Everything currently known about this endpoint:model. Expired learned facts
 *  are filtered out here, so no caller ever has to think about the TTL. */
export async function getCapabilities(key: CapabilityKey): Promise<Partial<Record<Capability, CapabilityFact>>> {
  const now = Date.now()
  const out: Partial<Record<Capability, CapabilityFact>> = {}
  for (const [cap, raw] of Object.entries(entryOf(await readAll(), key))) {
    if (!isCapability(cap)) continue
    const fact = readFact(raw, now)
    if (fact) out[cap] = fact
  }
  return out
}

/** Record one fact, leaving every other capability on this key — and every
 *  other key — alone. Last write wins per capability, which is what makes a
 *  probe result able to correct a fact the gateway learned from a single 400. */
export async function recordCapability(key: CapabilityKey, cap: Capability, fact: CapabilityFact): Promise<void> {
  await serialize(async () => {
    const all = await readAll()
    const now = Date.now()
    const entry = { ...entryOf(all, key) }
    // Opportunistic cleanup while we hold the row: drop expired and malformed
    // facts, but ONLY under capability ids this build recognizes. During a
    // rolling deploy the other process may be a newer build that knows
    // capabilities we don't, and rewriting the row must not delete its work.
    for (const [k, v] of Object.entries(entry)) {
      if (isCapability(k) && readFact(v, now) === null) delete entry[k]
    }
    entry[cap] = fact
    all[key] = entry
    await setSetting(KEY, all)
  })
}

/** MANY FACTS, MANY KEYS, ONE WRITE — and it respects `SOURCE_RANK`.
 *
 *  Two things forced this, and both come from the catalog becoming a writer:
 *
 *    VOLUME. `recordCapability` is a read-modify-write of one `app_settings`
 *    row, serialized. A catalog refresh derives up to four facts for each of
 *    four hundred models on each of several endpoints; done one fact at a time
 *    that is thousands of sequential rewrites of a row that grows with every
 *    one of them. Here it is one read and one write for the whole refresh.
 *
 *    PRECEDENCE. See `SOURCE_RANK`. A weaker source never displaces a stronger
 *    one, so a nightly catalog sweep cannot erase a probe result — and the
 *    return value says how many facts were actually taken, so a refresh can
 *    report "wrote 12, deferred to existing evidence on 3" rather than claiming
 *    credit for writes it did not make.
 *
 *  Facts under capability ids THIS BUILD DOES NOT RECOGNIZE are preserved
 *  untouched, exactly as `recordCapability` preserves them, because during a
 *  rolling deploy the other process may be a newer build. */
export async function mergeCapabilities(key: CapabilityKey, facts: Partial<Record<Capability, CapabilityFact>>): Promise<number>
export async function mergeCapabilities(batch: Array<{ key: CapabilityKey; facts: Partial<Record<Capability, CapabilityFact>> }>): Promise<number>
export async function mergeCapabilities(
  a: CapabilityKey | Array<{ key: CapabilityKey; facts: Partial<Record<Capability, CapabilityFact>> }>,
  b?: Partial<Record<Capability, CapabilityFact>>,
): Promise<number> {
  const batch = typeof a === 'string' ? [{ key: a, facts: b ?? {} }] : a
  if (batch.length === 0) return 0
  return serialize(async () => {
    const all = await readAll()
    const now = Date.now()
    let written = 0
    for (const { key, facts } of batch) {
      const entry = { ...entryOf(all, key) }
      for (const [cap, fact] of Object.entries(facts)) {
        if (!isCapability(cap) || !fact) continue
        // The incumbent is read through `readFact`, so an EXPIRED fact is not an
        // incumbent at all and anything may replace it.
        const prev = readFact(entry[cap], now)
        if (!outranks(fact.source, prev?.source)) continue
        entry[cap] = fact
        written++
      }
      all[key] = entry
    }
    await setSetting(KEY, all)
    return written
  })
}

/** The admin's "forget what you learned about this model" button (audit 1.2).
 *  It clears probe and declared facts too, by design: this is the full reset
 *  you reach for when a model id has been re-pointed at different weights and
 *  every recorded fact is now about something else. */
export async function forgetCapabilities(key: CapabilityKey): Promise<void> {
  await serialize(async () => {
    const all = await readAll()
    if (!(key in all)) return // nothing recorded — don't churn the row
    delete all[key]
    await setSetting(KEY, all)
  })
}

/** Does this model satisfy every capability a harness requires?
 *
 *  UNKNOWN IS NOT FALSE. An untested model must be allowed to run — Talaria
 *  cannot refuse to work until an admin gets around to running a benchmark, and
 *  on a fresh self-host nothing has been probed at all. Only a fact that
 *  positively says "no" counts as missing.
 *
 *  Returns the capabilities known to be MISSING; empty means go. Callers decide
 *  what to do with the unknowns — the role-assignment UI warns, `runHarness`
 *  takes the repair path, and neither of them blocks. */
export async function missingCapabilities(key: CapabilityKey, required: Capability[]): Promise<Capability[]> {
  const facts = await getCapabilities(key)
  return required.filter((cap) => facts[cap]?.value === false)
}
