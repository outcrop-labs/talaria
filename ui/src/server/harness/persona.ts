// Capability keys for a FLEET PERSONA.
//
// WHY THIS FILE EXISTS — the bug it closes
//   `runHarness` derives capability keys from `routingFor(model)`, which answers
//   with the GATEWAY endpoints that serve a catalog model id. A fleet persona
//   ("penny-assistant", "dex-developer") is not a catalog model: `routingFor`
//   returns `endpoints: []`, so the key list was empty, so step 3's
//   `keys.length > 0` guard never passed and `widened` was ALWAYS FALSE on every
//   persona run.
//
//   That made widening structurally dead on the path that matters most. The
//   marquee widening case in the whole project — the Inbox command harness
//   handing a capable model the item's full action list instead of a
//   regex-chosen single action (audit 1.8) — runs on the owner's PERSONAL
//   ASSISTANT, which is a persona. "Excel with larger models" could not fire
//   there, and nothing anywhere said so.
//
//   A persona is not an unknown quantity, though. It is BACKED by a real
//   endpoint + upstream model, recorded in its agent version's config, and that
//   is exactly the mapping `db/pg.ts` walks to backfill the usage ledger's
//   `endpoint` / `llm_model` columns (`config->'main'->>'model'`, plus the
//   `aliases` array for tiers). A persona whose backing model has been probed
//   inherits that probe. That is all this module does.
//
// WHAT IT DELIBERATELY DOES NOT DO
//   It never guesses. Every ambiguity resolves to NO KEYS, which lands the
//   caller on the unknown path — run anyway, do not widen. Unknown is safe in
//   both directions by construction (capability.ts's cardinal rule cuts one way,
//   the widening gate cuts the other); a WRONG fact is safe in neither, because
//   it can both refuse a model that works and widen a model that cannot hold the
//   wider prompt.
import { db } from '../db/pg'
import { capabilityKey, type CapabilityKey } from './capability'
import type { ModelTarget } from '../agent-defs'

/** One enabled agent's current version, as read from the database. `config` is
 *  `unknown` on purpose: it is a jsonb column that outlives the code that wrote
 *  it (a hand-edited row, an import from a previous version), and a malformed
 *  one must land a harness on the unknown path rather than crash a run. Same
 *  posture as `capability.ts`'s `readFact`. */
export interface PersonaRow {
  /** `agent_defs.model` — the routable base id, e.g. "dex-developer". */
  agent: string
  config: unknown
}

const asRecord = (raw: unknown): Record<string, unknown> | null =>
  raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : null

/** A stored target is only a target when it names BOTH halves of a capability
 *  key. A half-written one (endpoint set, model blank) would produce the key
 *  "spark:" — which is a key some other half-written row could also produce, so
 *  the two would silently pool each other's facts. */
function readTarget(raw: unknown): ModelTarget | null {
  const t = asRecord(raw)
  if (!t) return null
  const endpoint = typeof t.endpoint === 'string' ? t.endpoint.trim() : ''
  const model = typeof t.model === 'string' ? t.model.trim() : ''
  if (!endpoint || !model) return null
  return { endpoint, model }
}

function readTargetList(raw: unknown): ModelTarget[] {
  if (!Array.isArray(raw)) return []
  const out: ModelTarget[] = []
  for (const entry of raw) {
    const t = readTarget(entry)
    if (t) out.push(t)
  }
  return out
}

/** Everything a call on this persona-and-tier could actually land on: the tier's
 *  own target first, then the agent's fallback providers.
 *
 *  THE FALLBACKS BELONG IN THE POOL. Hermes moves to them when the primary
 *  errors, so "which model answers this call" is genuinely not knowable in
 *  advance — the same situation as a gateway model served by several endpoints,
 *  and `run.ts` already answers both capability questions UNANIMOUSLY over such
 *  a pool. Including them only ever makes the runner more cautious: one
 *  unprobed fallback is enough to withhold a widening, and enough to withhold a
 *  refusal. Both are the safe direction. */
function poolFor(primary: ModelTarget, fallbacks: ModelTarget[]): ModelTarget[] {
  return [primary, ...fallbacks]
}

/** Every routable persona id → the targets a call on it could land on.
 *
 *  Two passes, and the order is load-bearing. Base ids are claimed first so that
 *  an agent literally named "dex-developer-opus" (slug "dex", department
 *  "developer-opus") wins over the reading of that same string as the "opus"
 *  tier of "dex-developer". Both are real routable ids; the agent's own id is
 *  the one whose config we are certain describes it. */
export function personaIndex(rows: readonly PersonaRow[]): Map<string, ModelTarget[]> {
  const byId = new Map<string, ModelTarget[]>()

  const parsed = rows.map((row) => {
    const config = asRecord(row.config)
    return {
      agent: row.agent,
      main: readTarget(config?.main),
      aliases: config && Array.isArray(config.aliases) ? config.aliases : [],
      fallbacks: readTargetList(config?.fallbacks),
    }
  })

  for (const row of parsed) {
    // NO MAIN, NO KEYS — not "fall back to the fallbacks". The fallbacks only
    // serve when the primary fails, so a pool that omits the primary is a claim
    // about a model that will usually not be the one answering.
    if (row.main) byId.set(row.agent, poolFor(row.main, row.fallbacks))
  }

  for (const row of parsed) {
    for (const raw of row.aliases) {
      const alias = asRecord(raw)
      const name = typeof alias?.name === 'string' ? alias.name.trim() : ''
      const target = readTarget(alias)
      if (!name || !target) continue
      const id = `${row.agent}-${name}`
      if (byId.has(id)) continue // an agent's own id outranks another's tier
      byId.set(id, poolFor(target, row.fallbacks))
    }
  }

  return byId
}

/** The targets behind one routable persona id, tier included. Empty when the id
 *  is not a persona, when its config is missing or malformed, or when it names a
 *  TIER THIS AGENT DOES NOT HAVE.
 *
 *  That last case is the one worth stating out loud: an unresolvable tier
 *  returns nothing rather than falling back to the agent's main target.
 *  Inheriting the wrong model's capabilities is worse than inheriting none — the
 *  caller of "dex-developer-opus" asked for a different, usually larger model
 *  than `main`, and crediting main's probe to it would widen (or refuse) on a
 *  fact about something else entirely. */
export function personaTargets(model: string, rows: readonly PersonaRow[]): ModelTarget[] {
  return personaIndex(rows).get(model) ?? []
}

/** `personaTargets` as capability keys, deduplicated — a persona whose fallback
 *  points at the same endpoint:model as its main must not ask the same question
 *  twice on the hot path, and must not count twice in a unanimity vote. */
export function personaKeysFrom(model: string, rows: readonly PersonaRow[]): CapabilityKey[] {
  return [...new Set(personaTargets(model, rows).map((t) => capabilityKey(t.endpoint, t.model)))]
}

// ── The cached database read ─────────────────────────────────────────────────

/** Long enough that a busy install reads the table roughly once a minute rather
 *  than once a harness run; short enough that an admin who re-points an agent at
 *  a different model sees capability records follow within a minute. Nothing
 *  here is authoritative — a stale entry costs one run of a narrower prompt. */
const TTL_MS = 60_000
/** A failed read is retried far sooner than a good one is refreshed, but not on
 *  every run: a database that is down would otherwise be asked once per harness
 *  call by a subsystem whose answer is "no keys" either way. */
const RETRY_MS = 5_000

interface Snapshot {
  at: number
  ok: boolean
  byId: Map<string, ModelTarget[]>
}

let snapshot: Snapshot | null = null
let inflight: Promise<Snapshot> | null = null

/** THIS NEVER THROWS AND NEVER REJECTS. Resolving a persona is a lookup that
 *  makes a harness run BETTER; it is not a precondition for running one, and a
 *  database blip must not be able to turn a working harness into a failure. A
 *  failed read is an empty index, which is exactly the state a fresh self-host
 *  is in anyway. */
async function load(): Promise<Snapshot> {
  try {
    const sql = await db()
    const rows = (await sql`
      select d.model as agent, v.config as config
      from agent_defs d
      join agent_versions v on v.agent_id = d.id and v.version = d.current_version
      where d.enabled
    `) as unknown as PersonaRow[]
    return { at: Date.now(), ok: true, byId: personaIndex(rows) }
  } catch {
    return { at: Date.now(), ok: false, byId: new Map() }
  }
}

async function index(): Promise<Map<string, ModelTarget[]>> {
  const now = Date.now()
  if (snapshot && now - snapshot.at < (snapshot.ok ? TTL_MS : RETRY_MS)) return snapshot.byId
  if (!inflight) {
    const pending = load()
    inflight = pending
    void pending.then((s) => {
      snapshot = s
      if (inflight === pending) inflight = null
    })
  }
  return (await inflight).byId
}

/** The runner's default `personaKeys` edge: the capability keys a fleet persona
 *  inherits from the model actually serving it. Empty for anything that is not a
 *  live persona, which leaves the caller on the unknown path. */
export async function personaCapabilityKeys(model: string): Promise<CapabilityKey[]> {
  const byId = await index()
  return [...new Set((byId.get(model) ?? []).map((t) => capabilityKey(t.endpoint, t.model)))]
}

/** Drop the cached index. For tests, and for any caller that has just changed an
 *  agent's config and would rather not wait out the TTL. */
export function clearPersonaCache(): void {
  snapshot = null
  inflight = null
}
