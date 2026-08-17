// RESEARCH, AS A DURABLE RUN — the pipeline that used to die with its process.
//
// WHAT THIS DELETES. `server/research.ts` answered "the app restarted while I
// was researching" with:
//
//     update research_runs set status = 'error',
//       error = 'run went stale (app restarted mid-research?)'
//
// forty-five minutes after the fact, to a person who had asked a question and
// waited. Nothing was lost when that process died — every search that had come
// back was in memory and would have been in the report — but nothing had ever
// been WRITTEN DOWN in a form another process could pick up. `setPhase` wrote a
// sentence ("searching (3): postgres 17 logical replication") and no cursor, so
// there was nothing to resume FROM even if something had wanted to.
//
// This file is that cursor. The run is a sequence of small steps over a
// checkpoint that carries the resume position AND the work so far — the round,
// how many of the round's queries have come back, every finding, and the whole
// citation registry — so a driver on any instance can re-enter the loop at the
// exact query that had not run yet. `runs/reclaim.ts` is what re-enters it, and
// its 30s sweep is why the sweep in research.ts is gone rather than rewritten.
//
// ── THE STEPS, and why they are this small ──────────────────────────────────
//
//   begin       write the domain row if it is missing, resolve the search model
//               and the depth budget. NO billed call.
//   plan        ONE persona call: this round's queries (recon skips it — its
//               single query is the question).
//   search      ONE sonar call: one query, its findings, its sources.
//   synthesize  ONE persona call: the report.
//   artifact    create the report artifact and LINK it to the run, so a
//               re-entry can find the one it already made.
//   save        write the body, the members' editor grants, the citation rows
//               and the domain record's terminal fields. Every one idempotent.
//   publish     index the report, notify the owner, done.
//
// ONE BILLED CALL PER STEP, CHECKPOINTED IMMEDIATELY AFTER, is the whole reason
// the steps are cut here and not somewhere more convenient. The runtime is
// AT-LEAST-ONCE (runs/define.ts states the seven ways), and research is the
// port where risk 1 is expensive rather than theoretical: a search stage that
// ran and did not checkpoint is PAID FOR A SECOND TIME on resume. A step that
// ran two queries would risk two; a step that ran a whole round would risk four
// and a whole expedition sixteen.
//
// WHAT A RE-ENTRY REPEATS, IN THE WORST CASE, per step:
//   begin       nothing — one insert with `on conflict do nothing`.
//   plan        one planner call. Cheap, and the queries are re-planned rather
//               than duplicated: the checkpoint replaces the round's plan.
//   search      ONE sonar call, re-billed, for the one query in flight when the
//               process died. Its sources land in the registry the same way, so
//               the report is unaffected — only the invoice is.
//   synthesize  one synthesis call, re-billed. The artifact is not written yet,
//               so there is nothing to duplicate downstream.
//   artifact    a SECOND artifact only if the process dies between the create
//               and the link, which is one statement apart; the link is what
//               makes the id addressable, and the next entry reuses it. The
//               loser is an empty untitled doc, never given a body.
//   save        nothing that is not keyed: `saveArtifact` overwrites the same
//               id (it does take a version snapshot), the citation rows are
//               `on conflict do nothing`, the domain update is a set of
//               absolute values.
//   publish     the retrieval index is keyed on (sourceType, sourceId) and
//               hashes the content, so re-indexing is a no-op. The
//               NOTIFICATION is not keyed — a process that dies between the
//               bell row and the run's `done` write sends it twice. That is
//               the one repeat this port cannot close from here, and it is why
//               `publish` is the last step and does nothing after it.
//
// THE OTHER SIX HAZARDS, answered:
//   2 (abandoned but still running)  every step calls `stopIfAbandoned` before
//     each outward call, and the persona/search stages are handed `ctx.signal`.
//   3 (notifying from a step)  the run never messages anybody to ask something.
//     The one question it can raise is a `decide`, and the question IS the
//     notification.
//   4 (the re-ask)  the no-sources question's key carries the attempt number,
//     so a genuinely new ask is a new approval key.
//   5 (the answer consumed twice)  the branch that reads `ctx.decision`
//     checkpoints in the same step, or throws terminally.
//   6 (the enqueue)  `startResearch` passes a deterministic `opts.id` — the
//     research record's own uuid names the run, so one research row can only
//     ever have one run.
//   7 (the tail)  see `publish` above.
//
// TESTABILITY IS A DESIGN CONSTRAINT, as everywhere else in server/runs: every
// edge — the models, the artifacts, the domain rows, the index, the bell — is a
// field on `ResearchRunDeps`, so defs/research.test.ts drives whole runs with
// no database, no Redis and no gateway.
import { agentCategoryFolder, attachArtifact, createArtifact, saveArtifact } from '../../artifacts'
import { db } from '../../db/pg'
import { describeAgent } from '../../gateway'
import {
  clampQueries,
  queriesFromLines,
  researchQueriesHarness,
  researchSearchHarness,
  researchSynthesisHarness,
  searchTransport,
  type ResearchDepth,
  type SearchSource,
} from '../../harness/defs/research'
import { runHarness } from '../../harness/run'
import { setEditors } from '../../kb-perms'
import { gatewayModels } from '../../llm-gateway'
import { resolveRoleModel } from '../../model-roles'
import { capabilityKeysFor } from '../../harness/run'
import { reachFor, type Reach } from '../../capability-reach'
import { addNotification } from '../../notifications'
import { indexActivity, indexPersonal } from '../../retrieval/sources'
// Type-only, and therefore erased: `server/approvals.ts` imports runs/define.ts
// at runtime (the census asks a run's own definition who may decide it), so a
// value import back would be a cycle. Same one-way edge, same reason.
import type { Authority } from '../../approvals'
import { defineRun, registerRun, type RunDefinition, type RunStepContext, type StepResult } from '../define'

const LOG = '[runs/research]'

/** The registry key and the `kind` column. Written into every row this
 *  definition has ever produced, so it never changes. */
export const RESEARCH_RUN_KIND = 'research'

// ── The depth budgets, unchanged ─────────────────────────────────────────────
//
// Moved here from server/research.ts rather than imported from it: this module
// is loaded at boot so the reclaim sweep can drive a research run it finds, and
// an import back into the domain module would be a cycle. server/research.ts
// re-exports the two public names (`RESEARCH_MODES`, `searchModelFor`) so no
// caller moves.

/** Depth budget + search model preference per mode. Query counts bound each
 *  round; rounds bound the expedition loop. Overridable for tests via env. */
const MODES: Record<ResearchDepth, { rounds: number; queries: number; search: string[]; blurb: string }> = {
  recon: { rounds: 1, queries: 1, search: ['sonar', 'sonar-pro'], blurb: 'one fast pass — a cited answer in minutes' },
  brief: { rounds: 1, queries: 3, search: ['sonar-pro', 'sonar'], blurb: 'planned angles, one synthesis — a briefing' },
  expedition: { rounds: 3, queries: 4, search: ['sonar-pro', 'sonar-reasoning-pro', 'sonar'], blurb: 'iterative deep dive — a full report' },
}

export function budgetFor(mode: ResearchDepth): { rounds: number; queries: number; search: string[]; blurb: string } {
  const b = MODES[mode]
  const rounds = Number(process.env.TALARIA_RESEARCH_MAX_ROUNDS ?? 0) || b.rounds
  return { ...b, rounds }
}

export const RESEARCH_MODES: Array<{ mode: ResearchDepth; blurb: string }> = (
  Object.entries(MODES) as Array<[ResearchDepth, (typeof MODES)[ResearchDepth]]>
).map(([mode, b]) => ({ mode, blurb: b.blurb }))

// SEARCH PLANNING, PORTED FORWARD FROM server/research.ts.
//
// The version this file was copied from picked a model by matching sonar
// SPELLINGS out of the gateway catalog. Main replaced that with capability
// facts and a second path — `via: 'tool'`, where any routable model drives a
// registered search tool or the platform's SearXNG — which is what makes the
// feature model-agnostic and is the common case on a self-hosted install.
//
// It lives HERE now rather than there because the pipeline does: server/
// research.ts re-exports these names, the same way it already re-exports
// RESEARCH_MODES, so every caller keeps the import it had.
export interface SearchPlan {
  model: string
  /** `native`: the model searches as part of answering. `tool`: our harness
   *  drives a search tool and hands it the results. */
  via: 'native' | 'tool'
  supplier: { server: string; tool: string } | null
}

/** THE REASON A RUN CANNOT START, phrased for the person who can fix it.
 *  Exported so the route and the MCP tool report the same sentence. */
export const NO_SEARCH_REASON =
  'this workspace cannot search yet — either connect a search backend (Settings → Search, e.g. a self-hosted SearXNG) so any model can research through it, or register a model with native web search and assign it to the research role'

export async function planSearch(mode: ResearchDepth, deps?: { models?: () => Promise<Array<{ id: string }>> }): Promise<SearchPlan | null> {
  const pathOf = async (model: string): Promise<SearchPlan | null> => {
    const keys = await capabilityKeysFor(model).catch((): string[] => [])
    const reach = await reachFor(keys, ['search']).catch((): Record<string, Reach> => ({}))
    const r = reach['search']
    if (!r) return null
    if (r.via === 'tool') return { model, via: 'tool', supplier: r.supplier }
    if (r.via === 'native') return { model, via: 'native', supplier: null }
    return null
  }

  // 1. The admin said which one. Their choice, including the choice to use a
  //    model that turns out to need the tool path.
  //
  //    AND UNKNOWN IS NOT MISSING, which is this codebase's cardinal rule about
  //    capabilities and the reason this step falls through to `native` rather
  //    than to the next step. A fresh self-host has probed nothing and its
  //    catalog may say nothing, so `reachFor` answers with silence — and
  //    treating that silence as "this model cannot search" would refuse the
  //    admin's own explicit assignment. The runtime defence is one layer down,
  //    where it belongs: `researchSearchHarness` declares `requires: ['search']`
  //    with `refuseBelow: true`, so a model POSITIVELY KNOWN not to search fails
  //    the stage loudly instead of writing a fluent uncited brief.
  const assigned = await resolveRoleModel(`research-${mode}`)
  if (assigned) return (await pathOf(assigned)) ?? { model: assigned, via: 'native', supplier: null }

  const models = await (deps?.models ?? gatewayModels)().catch((): Array<{ id: string }> => [])
  if (models.length === 0) return null

  // 2. A model that searches on its own, if the org has one. Asked of its
  //    CAPABILITY FACTS, so a new sonar spelling — or any other vendor shipping
  //    native search — is picked up the day it is registered, with nothing here
  //    to edit.
  const plans = await Promise.all(models.map((m) => pathOf(m.id).catch((): SearchPlan | null => null)))
  const native = plans.find((p): p is SearchPlan => p?.via === 'native')
  if (native) return native

  // 3. Anything routable, through our own search. The model only has to be able
  //    to call a tool, which is most of them.
  return plans.find((p): p is SearchPlan => p !== null) ?? null
}

/** BACK-COMPAT for callers that only want the model id. `planSearch` is the one
 *  to reach for — it also says which path the run will take. */
export async function searchModelFor(mode: ResearchDepth): Promise<string | null> {
  return (await planSearch(mode))?.model ?? null
}

/** A deep-research-class search model is an agentic researcher in itself
 *  (each call runs its own multi-search sweep) — shrink OUR loop so effort
 *  doesn't multiply: fewer, bigger stages instead of many small ones.
 *
 *  AUDIT NOTE, left as-is deliberately: this is capability reasoning done by
 *  REGEX ON A MODEL NAME, which is the shape `harness/capability.ts` exists to
 *  replace. It is not converted here because "runs its own multi-search sweep"
 *  is a genuinely new capability, not one of the nine that module declares, and
 *  inventing a capability id that only one regex writes is worse than leaving
 *  the regex where a reader can see it. The right end state is a
 *  `deep-research` capability that a probe or a catalog declares, at which
 *  point this becomes a `widen` on the search harness — a capable model
 *  earning the fewer-bigger-stages loop — rather than a name match. Until then
 *  the regex is honest about being a heuristic. */
const isDeepResearchModel = (model: string) => /deep-research/i.test(model)

export function adaptBudget(budget: ReturnType<typeof budgetFor>, searchModel: string): ReturnType<typeof budgetFor> {
  if (!isDeepResearchModel(searchModel)) return budget
  return { ...budget, rounds: Math.min(budget.rounds, 2), queries: 1 }
}

// ── The citation registry ────────────────────────────────────────────────────

/** One source as the CHECKPOINT carries it. The registry is rebuilt from this
 *  list at the top of every step, which is the difference between a resumable
 *  run and a run that renumbers its citations from 1 the moment it restarts. */
export interface RegistrySource {
  idx: number
  url: string
  title: string | null
  snippet: string | null
}

interface SearchHit {
  content: string
  sources: SearchSource[]
}

export class SourceRegistry {
  private byUrl = new Map<string, { idx: number; title: string | null; snippet: string | null }>()

  /** Rehydrate from a checkpoint. Insertion order IS the numbering, so the
   *  entries go back in ascending `idx` — a registry rebuilt in a different
   *  order would hand out the same numbers to different documents. */
  static from(sources: readonly RegistrySource[]): SourceRegistry {
    const reg = new SourceRegistry()
    for (const s of [...sources].sort((a, b) => a.idx - b.idx)) reg.byUrl.set(s.url, { idx: s.idx, title: s.title, snippet: s.snippet })
    return reg
  }

  add(s: { url: string; title: string | null; snippet: string | null }): number {
    const existing = this.byUrl.get(s.url)
    if (existing) {
      if (!existing.title && s.title) existing.title = s.title
      return existing.idx
    }
    // `size + 1` and not "the highest idx + 1" only because the map is the
    // whole registry: rehydration above preserves both the entries and their
    // numbers, so the two are the same number.
    const idx = this.byUrl.size + 1
    this.byUrl.set(s.url, { idx, title: s.title, snippet: s.snippet })
    return idx
  }

  /** Rewrite one search hit's LOCAL [n] markers onto global numbering. */
  renumber(hit: SearchHit): string {
    const map = new Map<number, number>()
    hit.sources.forEach((s, i) => map.set(i + 1, this.add(s)))
    return hit.content.replace(/\[(\d{1,2})\]/g, (m, n) => {
      const g = map.get(Number(n))
      return g ? `[${g}]` : m
    })
  }

  list(): RegistrySource[] {
    return [...this.byUrl.entries()].map(([url, s]) => ({ idx: s.idx, url, title: s.title, snippet: s.snippet }))
  }

  get size(): number {
    return this.byUrl.size
  }
}

// ── Input and checkpoint ─────────────────────────────────────────────────────

/** Everything the pipeline needs that does not change while it runs. It is on
 *  the RUN's input rather than read back from `research_runs` every step for
 *  one reason that matters after a crash: the run row is written first, so a
 *  process that dies between the two inserts leaves a run that can still
 *  rebuild the domain record from what it was started with. */
export interface ResearchInput {
  question: string
  mode: ResearchDepth
  agentModel: string
  ownerUserId: string | null
  requestedBy: string
  /** THE RUN THIS ONE EXTENDS, for a follow-up asked from a report's own
   *  conversation. It is on the INPUT rather than read at synthesis time
   *  because a resumed run must seed the same registry the first attempt did:
   *  the parent's sources go into the checkpoint at `begin`, so [1]..[n] keep
   *  meaning what the already-written prose says they mean however many times
   *  the driver dies. Reading it later would renumber on the first reclaim. */
  parentRunId?: string | null
}

/** THE RESUME CURSOR, and the work so far.
 *
 *  It carries findings and sources rather than only positions because the
 *  alternative is re-running completed searches to rebuild them, which is
 *  exactly the cost this port exists to avoid — every one of those is a paid
 *  model call. The blob grows with the run (an expedition ends around a dozen
 *  findings and their sources); that is a jsonb column holding a report's worth
 *  of text, which is the same order as the report itself. */
export interface ResearchCheckpoint {
  /** What the NEXT step does. The one field the driver's re-entry branches on. */
  stage: 'plan' | 'search' | 'synthesize' | 'artifact' | 'save' | 'publish'
  /** Resolved once, in `begin`, and then fixed for the life of the run: an
   *  admin re-pointing the `research-*` role mid-expedition must not switch
   *  models between one round and the next. */
  searchModel: string
  /** The adapted depth budget, likewise resolved once. */
  rounds: number
  perRound: number
  /** 1-based. `round > rounds` means the loop is done. */
  round: number
  /** This round's planned queries, in order. */
  plan: string[]
  /** How many of `plan` have COME BACK and are already in `findings`. The
   *  single number that makes "resume mid-round" mean anything. */
  done: number
  /** Across all rounds — the `queries` stat, and the number in the phase line. */
  queriesRun: number
  /** One markdown note per query that has run, in order. */
  findings: string[]
  sources: RegistrySource[]
  /** Any query threw. Goes to the synthesis harness, which grounds against it. */
  searchFailed: boolean
  /** How many times the owner has said "search again" after a round that found
   *  nothing citable. Bounds the re-ask, and varies the approval key. */
  retries: number
  /** The written report, once the persona has produced it. */
  report: { title: string; body: string; cited: number; dropped: number; ungrounded: number } | null
  /** THE IDEMPOTENCY HANDLE for the one effect that cannot be undone. Written
   *  before the body is, so a re-entry writes into the artifact it already
   *  made rather than creating a second report. */
  artifactId: string | null
}

// ── Deps ─────────────────────────────────────────────────────────────────────

export interface ResearchRunDeps {
  searchModelFor: (mode: ResearchDepth) => Promise<string | null>
  /** The persona plans a round. Never throws — a failed plan is one lost round,
   *  and an empty list is the persona saying the question is saturated. */
  planQueries: (args: { runId: string; agentModel: string; question: string; findingsSoFar: string[]; max: number; signal: AbortSignal }) => Promise<string[]>
  /** One query against the search model. THROWS on a dead query, which costs
   *  one angle rather than the run. */
  search: (args: { runId: string; model: string; query: string; signal: AbortSignal }) => Promise<SearchHit>
  /** The persona writes the document. Throws on an unusable reply. */
  synthesize: (args: {
    runId: string
    agentModel: string
    question: string
    mode: ResearchDepth
    sources: Array<{ idx: number; url: string; title: string | null }>
    findings: string[]
    searchFailed: boolean
    signal: AbortSignal
  }) => Promise<{ doc: string; ungrounded: number }>
  agentLabel: (agentModel: string) => string
  /** Write the domain record if it is not there. Idempotent on the id. */
  ensureRow: (runId: string, input: ResearchInput) => Promise<void>
  /** The parent report's numbered sources, for a follow-up. Injected like every
   *  other read so the seeding is testable without a database. */
  sourcesOf: (runId: string) => Promise<RegistrySource[]>
  /** Is the domain record still there? DELETE /api/research/:id removes it, and
   *  that is the product's "stop this" — a run that kept spending after it
   *  would be billing a person for a thing they threw away. */
  rowExists: (runId: string) => Promise<boolean>
  memberIds: (runId: string) => Promise<string[]>
  saveSources: (runId: string, sources: RegistrySource[]) => Promise<void>
  finishRow: (args: { runId: string; artifactId: string; stats: Record<string, number> }) => Promise<void>
  /** Mirror a failure onto the domain record. See `mirrorFailure`. */
  failRow: (runId: string, error: string) => Promise<void>
  /** The artifact already linked to this run, if a previous entry made one.
   *
   *  The same query as `researchArtifactFor` in server/research.ts, and NOT an
   *  import of it: that module imports this one (it is loaded at boot so a
   *  reclaimed research run has code to drive it) and the edge only goes one
   *  way. If a third caller ever wants it, the query moves to artifacts.ts,
   *  which is where "what is linked to this target" belongs. */
  linkedArtifact: (runId: string) => Promise<string | null>
  /** Create the report artifact AND link it to the run, in that order and with
   *  nothing between them: the link is what makes the id findable by the next
   *  entry, so the window in which a crash costs a second artifact is one
   *  statement wide. */
  createReport: (args: { runId: string; title: string; ownerUserId: string | null; requestedBy: string; agentLabel: string }) => Promise<string>
  writeReport: (args: { artifactId: string; body: string; ownerUserId: string | null; agentLabel: string; memberIds: string[] }) => Promise<void>
  index: (args: { runId: string; artifactId: string; title: string; body: string; question: string; mode: ResearchDepth; ownerUserId: string | null }) => Promise<void>
  notify: (args: { ownerUserId: string; runId: string; title: string; agentLabel: string; mode: ResearchDepth; sources: number }) => Promise<void>
}

// ── The real edges ───────────────────────────────────────────────────────────

/** One search query → the search model's cited answer + its source list.
 *
 *  What a THROW means is unchanged: the search step catches per query and
 *  records "(search failed: ...)" against that angle, so one dead query costs
 *  one angle rather than the run, and a run where every query died ends with no
 *  citable sources — which the pipeline now takes to the OWNER rather than
 *  failing on the spot.
 *
 *  The capability floor is the harness's: `requires: ['search']` with
 *  `refuseBelow: true`, so a model an admin assigned to a `research-*` role
 *  that is KNOWN not to search fails loudly here instead of answering fluently
 *  from training data and handing the synthesis stage an uncited brief to write
 *  up (audit 1.6). The sources come off the provider's own
 *  `search_results`/`citations` fields — see `searchTransport`. */
async function searchStage(args: { runId: string; model: string; query: string; signal: AbortSignal }): Promise<SearchHit> {
  const sources: SearchSource[] = []
  const run = await runHarness(
    researchSearchHarness,
    { query: args.query },
    { caller: `research:${args.runId}`, model: args.model, signal: args.signal, deps: { transport: searchTransport(args.runId, sources) } },
  )
  // `researchSearchHarness` declares `onFailure: 'throw'` and `runHarness`
  // honors it on every path that fails to produce a value — the transport and
  // the pre-call ones included — so this is a TYPE narrowing rather than a
  // second copy of the policy.
  if (run.value === null) throw new Error(run.error ?? `search stage produced nothing on "${args.model}"`)
  return { content: run.value, sources }
}

/** This round's search queries, or an empty list when the persona says the
 *  question is saturated. Never throws.
 *
 *  THE LINE-BASED SALVAGE, which used to live inside the extractor. On a small
 *  model a numbered list is likelier than a JSON array, and the old
 *  `parseQueryList` quietly read one — so a model that never once produced the
 *  declared contract was indistinguishable from a model that always did. The
 *  tolerance is kept and the silence is not: the harness records the contract
 *  failure on its `harness_runs` row, and the salvage runs afterwards, here,
 *  where it is visible.
 *
 *  It runs ONLY on a reply the guard found nothing in. These strings are sent
 *  onward to a third-party search model, and guardrails.ts's cardinal invariant
 *  is that flagged content never re-enters a model's context — a rule the
 *  runner honors for its own repair turn and which a salvage path must not
 *  quietly route around. */
async function planStage(args: { runId: string; agentModel: string; question: string; findingsSoFar: string[]; max: number; signal: AbortSignal }): Promise<string[]> {
  const run = await runHarness(
    researchQueriesHarness,
    { question: args.question, max: args.max, findingsSoFar: args.findingsSoFar },
    // Both persona stages of a run carry the same attribution — see the
    // synthesis call for why `source: 'research'` exists at all. An expedition
    // plans up to three times, so leaving this off would make the planning half
    // of a run's cost the half nobody could find.
    { caller: `research:${args.runId}`, model: args.agentModel, signal: args.signal, ledger: { source: 'research', refId: args.runId } },
  )
  if (run.value !== null) return clampQueries(run.value, args.max)
  if (run.raw && run.findings.length === 0) return clampQueries(queriesFromLines(run.raw), args.max)
  return []
}

/** The persona writes the document against the global registry.
 *
 *  THE GROUNDING PASS IS INSIDE THIS CALL. `searchFailed` and the findings are
 *  on the INPUT because `researchSynthesisHarness` declares `ground`: the
 *  search hits are this turn's tool results, so `ungrounded_ref` — the
 *  definitive research failure mode — runs on the report, from the runner, with
 *  one findings row per fabricated link. */
async function synthesisStage(args: Parameters<ResearchRunDeps['synthesize']>[0]): Promise<{ doc: string; ungrounded: number }> {
  const synthesis = await runHarness(
    researchSynthesisHarness,
    { question: args.question, mode: args.mode, sources: args.sources, findings: args.findings, searchFailed: args.searchFailed },
    // `source: 'research'` + the run id is what makes a run's COST answerable.
    // The persona stages used to meter as an ownerless chat turn, so research
    // spend was indistinguishable from every other persona call in
    // `usage_events`. The search stages meter themselves through the gateway.
    { caller: `research:${args.runId}`, model: args.agentModel, signal: args.signal, ledger: { source: 'research', refId: args.runId } },
  )
  // A TYPE NARROWING, not a policy — `researchSynthesisHarness` declares
  // `onFailure: 'throw'` and the runner honors it on every path that fails to
  // produce a value. It used to be a policy, restated by hand, because 'throw'
  // covered a CONTRACT failure and nothing else: a persona gateway answering
  // 502 mid-synthesis arrived as `value: null`, and the `?? ''` this replaced
  // saved an artifact containing only the Sources list, marked the run `done`,
  // indexed the empty report and notified the requester.
  if (synthesis.value === null) throw new Error(synthesis.error ?? 'the report came back empty')
  // Filtered to `ungrounded_ref` because `synthesis.findings` also carries the
  // `secret_leak` / `pii_leak` hits, and this number has one meaning on the
  // research surface: how many links this model asserted that no search result
  // backs.
  return { doc: synthesis.value, ungrounded: synthesis.findings.filter((f) => f.check === 'ungrounded_ref').length }
}

export const REAL_RESEARCH_DEPS: ResearchRunDeps = {
  searchModelFor,
  planQueries: planStage,
  search: searchStage,
  synthesize: synthesisStage,
  agentLabel: (agentModel) => describeAgent(agentModel).label,

  async ensureRow(runId, input) {
    const sql = await db()
    // The run row is written first, so this is the recovery path for a process
    // that died between the two inserts: the run knows what it was started
    // with and can rebuild the record a person reads. `on conflict do nothing`
    // because the ordinary path is `startResearch` having already written it —
    // and because a DELETED run must not be resurrected by a later step, which
    // is why this only ever runs at the very first step.
    await sql`
      insert into research_runs (id, owner_user_id, requested_by, agent_model, mode, question, parent_run_id)
      values (${runId}, ${input.ownerUserId}, ${input.requestedBy}, ${input.agentModel}, ${input.mode}, ${input.question}, ${input.parentRunId ?? null})
      on conflict (id) do nothing
    `
  },

  async sourcesOf(runId) {
    const sql = await db()
    return (await sql`
      select idx, url, title, snippet from research_sources where run_id = ${runId} order by idx asc
    `) as unknown as RegistrySource[]
  },

  async rowExists(runId) {
    const sql = await db()
    const rows = (await sql`select 1 as ok from research_runs where id = ${runId}`) as unknown as Array<{ ok: number }>
    return rows.length > 0
  },

  async memberIds(runId) {
    const sql = await db()
    const rows = (await sql`select user_id as id from research_members where run_id = ${runId}`) as unknown as Array<{ id: string }>
    return rows.map((r) => r.id)
  },

  async saveSources(runId, sources) {
    const sql = await db()
    for (const s of sources) {
      await sql`
        insert into research_sources (run_id, idx, url, title, snippet)
        values (${runId}, ${s.idx}, ${s.url}, ${s.title}, ${s.snippet}) on conflict do nothing
      `
    }
  },

  async finishRow({ runId, artifactId, stats }) {
    const sql = await db()
    // `status` is written here and in `failRow` and NOWHERE else. It is not the
    // authority on whether the run is alive — the `runs` row is, and
    // server/research.ts's reads project it — it is the TERMINAL OUTCOME, kept
    // as a column because /api/research's duplicate-question check reads it in
    // raw SQL. `error = null` because a run that finishes after a failed
    // attempt must not keep the old attempt's sentence.
    await sql`
      update research_runs set status = 'done', phase = null, error = null, artifact_id = ${artifactId},
        stats = ${sql.json(stats)}, updated_at = now(), completed_at = now()
      where id = ${runId}
    `
  },

  async failRow(runId, error) {
    const sql = await db()
    await sql`
      update research_runs set status = 'error', phase = null, error = ${error.slice(0, 2000)}, updated_at = now()
      where id = ${runId} and status <> 'done'
    `
  },

  async linkedArtifact(runId) {
    const sql = await db()
    const rows = (await sql`
      select artifact_id as id from artifact_links where target_type = 'research' and target_id = ${runId} limit 1
    `) as unknown as Array<{ id: string }>
    return rows[0]?.id ?? null
  },

  async createReport({ runId, title, ownerUserId, requestedBy, agentLabel }) {
    const artifact = await createArtifact({
      kind: 'doc',
      title,
      createdBy: requestedBy,
      ownerUserId,
      folderId: await agentCategoryFolder(agentLabel, 'Research', requestedBy),
    })
    // IMMEDIATELY, and with nothing between: the link is the handle a re-entry
    // finds the artifact by, so every statement between the create and it is
    // another statement wide the "two reports" window gets.
    await attachArtifact(artifact.id, { targetType: 'research', targetId: runId }, agentLabel)
    return artifact.id
  },

  async writeReport({ artifactId, body, ownerUserId, agentLabel, memberIds }) {
    // Ownership decides reach: an ORG run (no owner) publishes org-visible; a
    // user's run stays PRIVATE to them, with members granted editor on the doc
    // — sharing the run is the only way anyone else sees it.
    await saveArtifact(artifactId, { body, visibility: ownerUserId ? 'private' : 'org' }, agentLabel)
    if (ownerUserId && memberIds.length)
      await setEditors(
        'artifact',
        artifactId,
        memberIds.map((id) => ({ principalType: 'user' as const, principalId: id, role: 'editor' as const })),
      ).catch(() => {})
  },

  async index({ runId, artifactId, title, body, question, mode, ownerUserId }) {
    // Placement follows ownership: a personal run's report goes to the owner's
    // private brain (them + their assistant); an org-wide run's report goes to
    // the ambient index, marked orgWide so scopes actually match it.
    const doc = {
      sourceType: 'research',
      sourceId: artifactId,
      title,
      text: `${title}\n\n${body}`,
      payload: ownerUserId ? { runId, question, mode } : { runId, question, mode, orgWide: true },
      href: `/research?r=${runId}`,
    }
    if (ownerUserId) await indexPersonal(ownerUserId, doc)
    else await indexActivity(doc)
  },

  async notify({ ownerUserId, runId, title, agentLabel, mode, sources }) {
    await addNotification(ownerUserId, {
      kind: 'research',
      title: `Research ready: ${title.slice(0, 120)}`,
      body: `${agentLabel} finished ${mode === 'recon' ? 'a recon' : mode === 'brief' ? 'a brief' : 'an expedition'} — ${sources} sources`,
      href: `/research?r=${runId}`,
    })
  },
}

// ── The step ─────────────────────────────────────────────────────────────────

type Ctx = RunStepContext<ResearchInput, ResearchCheckpoint>
type Result = StepResult<ResearchCheckpoint>

/** The sentence a run with nothing citable ends on, when the owner says stop or
 *  has already been asked. Unchanged from the version that used to be thrown
 *  the moment the loop ended. */
const NO_SOURCES = 'no sources found — search returned nothing citable'

/** How many times the owner may be asked to search again before the run gives
 *  up on its own. Two asks: the transient outage, and the one after it. */
const MAX_NO_SOURCE_ASKS = 2

const noSourceKey = (attempt: number) => `no-sources:attempt-${attempt}`

/** Risk 2, stated as a function. `maxStepMs` and a lost lease abort by
 *  REJECTING a race — nothing stops a promise that ignores its signal — so a
 *  step can be re-entered on another instance while this one is still in
 *  flight. Called before every outward call so the abandoned copy stops
 *  spending rather than racing its successor to the same checkpoint. */
function stopIfAbandoned(ctx: Ctx): void {
  if (ctx.signal.aborted) throw new Error('the driver abandoned this step — another instance resumes from the last checkpoint')
}

/** How round one starts, in one place because it happens twice: at `begin`, and
 *  again when the owner says "search again" after a round that found nothing.
 *  Recon asks the question itself — one pass, no planner call — and every other
 *  mode plans its angles first. Two spellings of that would mean a retried
 *  recon quietly acquiring a planning stage the mode does not have. */
const firstRound = (input: ResearchInput): { stage: 'plan' | 'search'; plan: string[] } =>
  input.mode === 'recon' ? { stage: 'search', plan: [input.question] } : { stage: 'plan', plan: [] }

const cleanupMarkers = (doc: string, known: Set<number>): string =>
  doc
    .trim()
    .replace(/^```[a-z]*\n?|\n?```$/g, '')
    .replace(/\[(\d{1,2})\]/g, (m, n) => (known.has(Number(n)) ? m : ''))

async function advance(ctx: Ctx, deps: ResearchRunDeps): Promise<Result> {
  const runId = ctx.run.id
  const input = ctx.input
  const cp = ctx.checkpoint

  // ── begin ────────────────────────────────────────────────────────────────
  if (cp === null) {
    await deps.ensureRow(runId, input)
    const searchModel = await deps.searchModelFor(input.mode)
    // The same refusal `startResearch` makes up front, restated here because a
    // reclaimed run is entered without going through it — and a gateway that
    // lost its sonar model between the click and the resume is a real state.
    if (!searchModel) throw new Error('no search-capable model on the gateway — register a Perplexity sonar model on /models first')
    const budget = adaptBudget(budgetFor(input.mode), searchModel)
    // A FOLLOW-UP CONTINUES ITS PARENT'S NUMBERING. Seeded here, into the
    // checkpoint, so [1]..[n] keep meaning what the parent's already-written
    // prose says they mean and anything new starts above the highest — and so
    // that a reclaim rebuilds the same registry rather than restarting at [1]
    // and silently re-aiming every citation a human already read.
    const parentSources = input.parentRunId ? await deps.sourcesOf(input.parentRunId) : []
    return {
      kind: 'next',
      checkpoint: {
        ...firstRound(input),
        searchModel,
        rounds: budget.rounds,
        perRound: budget.queries,
        round: 1,
        done: 0,
        queriesRun: 0,
        findings: [],
        sources: parentSources,
        searchFailed: false,
        retries: 0,
        report: null,
        artifactId: null,
      },
      phase: 'starting',
    }
  }

  // DELETE on /api/research/:id removes the domain record and is the only stop
  // button research has. Checked once per step rather than per call: it costs
  // one indexed read and it is the difference between "I deleted that" and a
  // run that goes on billing searches for a report nobody will ever open.
  if (!(await deps.rowExists(runId))) {
    console.log(`${LOG} ${runId}: the research record is gone (deleted) — stopping at stage "${cp.stage}"`)
    return { kind: 'done', result: { deleted: true } }
  }

  switch (cp.stage) {
    // ── plan: ONE persona call ─────────────────────────────────────────────
    case 'plan': {
      if (cp.round > cp.rounds) return { kind: 'next', checkpoint: { ...cp, stage: 'synthesize' }, phase: 'writing the report' }
      ctx.log(cp.round === 1 ? 'planning search angles' : `round ${cp.round}: chasing gaps`)
      stopIfAbandoned(ctx)
      const plan = await deps.planQueries({
        runId,
        agentModel: input.agentModel,
        question: input.question,
        // Round 1 plans from the question; later rounds plan against what is
        // already known, which is what makes an expedition iterative.
        findingsSoFar: cp.round === 1 ? [] : cp.findings,
        max: cp.perRound,
        signal: ctx.signal,
      })
      // An empty plan is the persona saying the question is saturated — a
      // reason to synthesize, never a failure.
      if (plan.length === 0) return { kind: 'next', checkpoint: { ...cp, stage: 'synthesize', round: cp.rounds + 1 }, phase: 'writing the report' }
      return { kind: 'next', checkpoint: { ...cp, stage: 'search', plan, done: 0 }, phase: `round ${cp.round}: ${plan.length} angle(s) planned` }
    }

    // ── search: ONE sonar call, and the checkpoint straight after it ───────
    case 'search': {
      const query = cp.plan[cp.done]
      if (query === undefined) {
        // The round is finished. Defensive rather than reachable — the branch
        // below advances the round as it consumes the last query — but a
        // checkpoint written by an older deploy could land here.
        return { kind: 'next', checkpoint: { ...cp, stage: 'plan', round: cp.round + 1, plan: [], done: 0 }, phase: `round ${cp.round} complete` }
      }
      const n = cp.queriesRun + 1
      ctx.log(`searching (${n}): ${query.slice(0, 80)}`)
      const registry = SourceRegistry.from(cp.sources)
      let note: string
      let failed = cp.searchFailed
      try {
        stopIfAbandoned(ctx)
        const hit = await deps.search({ runId, model: cp.searchModel, query, signal: ctx.signal })
        note = `### Query: ${query}\n${registry.renumber(hit)}`
      } catch (e) {
        // ONE DEAD QUERY COSTS ONE ANGLE, unchanged — except that an abandoned
        // step must not be recorded as a failed search: it is this driver
        // being taken off the run, and the query has not been asked yet.
        if (ctx.signal.aborted) throw e
        failed = true
        note = `### Query: ${query}\n(search failed: ${(e as Error).message})`
      }
      const done = cp.done + 1
      const roundOver = done >= cp.plan.length
      return {
        kind: 'next',
        checkpoint: {
          ...cp,
          // The round advances IN THE SAME WRITE that records its last query,
          // so there is no checkpoint in which a completed round is still
          // pointing at a query that has already run.
          ...(roundOver ? { stage: 'plan' as const, round: cp.round + 1, plan: [], done: 0 } : { done }),
          queriesRun: n,
          findings: [...cp.findings, note],
          sources: registry.list(),
          searchFailed: failed,
        },
        phase: `searched ${n}`,
      }
    }

    // ── synthesize: ONE persona call ───────────────────────────────────────
    case 'synthesize': {
      if (cp.sources.length === 0) {
        // NOTHING CITABLE. This used to throw on the spot and end the run with
        // a sentence nobody could act on. Every one of these is either a
        // transient search outage or a question nothing on the web answers, and
        // only the person who asked can tell those apart — so the run PARKS on
        // it and keeps everything it has. Risk 5: the branch that reads the
        // answer checkpoints in this same step, or throws terminally.
        const answer = ctx.decision
        if (answer && answer.key === noSourceKey(cp.retries)) {
          if (answer.optionId === 'search-again')
            return {
              kind: 'next',
              // The findings are dropped with the round: with no sources in the
              // registry every one of them is a failure note or uncited prose,
              // and carrying them into the retry would put them in the report.
              checkpoint: { ...cp, ...firstRound(input), round: 1, done: 0, findings: [], searchFailed: false, retries: cp.retries + 1 },
              phase: 'searching again',
            }
          throw new Error(NO_SOURCES)
        }
        if (cp.retries >= MAX_NO_SOURCE_ASKS) throw new Error(NO_SOURCES)
        return {
          kind: 'decide',
          question: {
            // RISK 4: the attempt is IN the key. Re-asking after a reclaim
            // produces the same key and dedupes; a genuinely new ask after a
            // retry produces a new one, so the second question is announced
            // rather than inheriting the first one's mark.
            key: noSourceKey(cp.retries),
            question: `No sources came back for "${input.question.slice(0, 120)}". Search again?`,
            detail: `${cp.queriesRun} search(es) ran and none of them returned anything citable${cp.searchFailed ? ' — at least one failed outright' : ''}. Nothing is lost either way: the run keeps what it has.`,
            options: [
              { id: 'search-again', label: 'Search again', detail: 'Re-plan the angles and run the searches again.' },
              { id: 'stop', label: 'Stop', detail: 'End the run without a report.' },
            ],
            href: `/research?r=${runId}`,
          },
        }
      }

      ctx.log('writing the report')
      stopIfAbandoned(ctx)
      const registry = SourceRegistry.from(cp.sources)
      const { doc, ungrounded } = await deps.synthesize({
        runId,
        agentModel: input.agentModel,
        question: input.question,
        mode: input.mode,
        sources: registry.list().map((s) => ({ idx: s.idx, url: s.url, title: s.title })),
        findings: cp.findings,
        searchFailed: cp.searchFailed,
        signal: ctx.signal,
      })

      // Keep only markers that resolve; append the mechanical Sources section.
      //
      // `dropped` is counted rather than only stripped. Deleting an invented
      // citation is the right thing to save, but it also made a model that
      // fabricates half its markers look identical to one that cites perfectly
      // — the exact model-fitness signal this run is in the best position to
      // report, thrown away by the line that fixed the symptom.
      const known = new Set(registry.list().map((s) => s.idx))
      const dropped = [...doc.matchAll(/\[(\d{1,2})\]/g)].filter((m) => !known.has(Number(m[1]))).length
      const cleaned = cleanupMarkers(doc, known)
      const cited = new Set([...cleaned.matchAll(/\[(\d{1,2})\]/g)].map((m) => Number(m[1])))
      const sourcesMd = registry
        .list()
        .map((s) => `${s.idx}. [${s.title ?? s.url}](${s.url})${cited.has(s.idx) ? '' : ' *(consulted)*'}`)
        .join('\n')
      return {
        kind: 'next',
        checkpoint: {
          ...cp,
          stage: 'artifact',
          report: {
            title: cleaned.match(/^# (.+)$/m)?.[1]?.trim() ?? `Research — ${input.question.slice(0, 80)}`,
            body: `${cleaned}\n\n## Sources\n\n${sourcesMd}\n`,
            cited: cited.size,
            dropped,
            ungrounded,
          },
        },
        phase: 'filing the report',
      }
    }

    // ── artifact: get an addressable id BEFORE anything writes a body ──────
    case 'artifact': {
      const report = cp.report
      if (!report) return { kind: 'next', checkpoint: { ...cp, stage: 'synthesize' }, phase: 'writing the report' }
      // THE GUARD AGAINST TWO REPORTS. The checkpoint first (the ordinary
      // resume), then the LINK — which is written in the same breath as the
      // create, so an entry whose predecessor died after creating but before
      // checkpointing still finds the artifact it made instead of making
      // another one.
      const existing = cp.artifactId ?? (await deps.linkedArtifact(runId))
      if (existing) return { kind: 'next', checkpoint: { ...cp, artifactId: existing, stage: 'save' }, phase: 'saving the report' }
      stopIfAbandoned(ctx)
      const artifactId = await deps.createReport({
        runId,
        title: report.title,
        ownerUserId: input.ownerUserId,
        requestedBy: input.requestedBy,
        agentLabel: deps.agentLabel(input.agentModel),
      })
      return { kind: 'next', checkpoint: { ...cp, artifactId, stage: 'save' }, phase: 'saving the report' }
    }

    // ── save: four writes, every one of them keyed ────────────────────────
    case 'save': {
      const report = cp.report
      const artifactId = cp.artifactId
      if (!report || !artifactId) return { kind: 'next', checkpoint: { ...cp, stage: report ? 'artifact' : 'synthesize' }, phase: 'filing the report' }
      stopIfAbandoned(ctx)
      await deps.writeReport({
        artifactId,
        body: report.body,
        ownerUserId: input.ownerUserId,
        agentLabel: deps.agentLabel(input.agentModel),
        memberIds: input.ownerUserId ? await deps.memberIds(runId) : [],
      })
      await deps.saveSources(runId, cp.sources)
      await deps.finishRow({
        runId,
        artifactId,
        stats: { queries: cp.queriesRun, sources: cp.sources.length, cited: report.cited, dropped: report.dropped, ungrounded: report.ungrounded },
      })
      return { kind: 'next', checkpoint: { ...cp, stage: 'publish' }, phase: 'publishing' }
    }

    // ── publish: the last step, and deliberately the smallest ─────────────
    case 'publish': {
      const report = cp.report
      const artifactId = cp.artifactId
      if (!report || !artifactId) return { kind: 'done', result: { artifactId, sources: cp.sources.length } }
      stopIfAbandoned(ctx)
      // Indexing first because it is keyed on (sourceType, sourceId) and
      // hashes its content: a repeat is a no-op. The BELL is not keyed, so it
      // goes last and nothing follows it but the terminal write — risk 7, the
      // tail of the last step, kept as short as it can be made.
      await deps.index({
        runId,
        artifactId,
        title: report.title,
        body: report.body,
        question: input.question,
        mode: input.mode,
        ownerUserId: input.ownerUserId,
      })
      if (input.ownerUserId)
        await deps
          .notify({
            ownerUserId: input.ownerUserId,
            runId,
            title: report.title,
            agentLabel: deps.agentLabel(input.agentModel),
            mode: input.mode,
            sources: cp.sources.length,
          })
          .catch((e) => console.error(`${LOG} ${runId}: the report is saved but the notification failed:`, e))
      return { kind: 'done', result: { artifactId, sources: cp.sources.length, queries: cp.queriesRun } }
    }
  }
}

/** Put the run's failure on the DOMAIN record too, then let it throw.
 *
 *  Not a second source of truth for whether the run is alive — server/research.ts
 *  projects that from the `runs` row — but `research_runs.status` is what
 *  /api/research's duplicate-question check reads in raw SQL, and a question
 *  whose last run failed must be askable again. The driver's own `fail` writes
 *  the run row; this writes the sentence a person reads next to their question,
 *  exactly as the old `catch` in `runResearch` did.
 *
 *  NOT ON AN ABANDONED STEP. An aborted step is this driver being taken off the
 *  run — by a lost lease or a deploy — and the run is very probably about to be
 *  resumed by somebody else. Writing `error` there would put a failure on the
 *  record of a run that is still working. */
async function mirrorFailure(ctx: Ctx, deps: ResearchRunDeps, e: unknown): Promise<void> {
  if (ctx.signal.aborted) return
  const message = e instanceof Error ? e.message : String(e)
  await deps.failRow(ctx.run.id, message).catch((write) => console.error(`${LOG} ${ctx.run.id}: could not record "${message}" on the research row:`, write))
}

export function makeResearchRun(deps: ResearchRunDeps): RunDefinition<ResearchInput, ResearchCheckpoint> {
  return defineRun<ResearchInput, ResearchCheckpoint>({
    kind: RESEARCH_RUN_KIND,
    label: 'Research',
    async step(ctx) {
      try {
        return await advance(ctx, deps)
      } catch (e) {
        await mirrorFailure(ctx, deps, e)
        throw e
      }
    },
    /** WHO MAY DECIDE, and it is the run's owner rather than its members.
     *
     *  Research is owner-scoped: `researchRole` gives the owner the run and
     *  everyone else a read through `research_members` or through the run
     *  having no owner at all. The question this run can park on ("nothing
     *  citable came back — search again?") spends the owner's budget and
     *  belongs to the person who asked it.
     *
     *  MEMBERS ARE DELIBERATELY NOT IN HERE. `audience` is synchronous — it is
     *  called from inside the approvals census, on a row — so it cannot query
     *  the membership table, and a membership list copied onto the run's input
     *  at enqueue time would be a stale copy of a thing people are added to
     *  while a run is in flight.
     *
     *  AN ORG RUN (no owner — a general agent researching for the workspace)
     *  goes to `{ by: 'admin' }`. That is a NARROWING and not a widening: an
     *  ownerless run is already readable by anyone signed in (`researchRole`
     *  returns 'member' for it), so its question is not a disclosure to the
     *  admins, and the admins are the people who can act on "the search models
     *  are not answering". */
    audience: (run): Authority => (run.ownerUserId ? { by: 'user', userIds: [run.ownerUserId] } : { by: 'admin' }),
    /** ONE STAGE'S BUDGET, and it has to be bigger than the transport's worst
     *  case rather than a guess at the typical one: a step that blows this is
     *  filed as an error and NOT retried (see run.ts — the step is probably
     *  still running, and re-entering would put two copies in flight). The
     *  ceiling underneath it is `llm-gateway`'s own `AbortSignal.timeout`, ten
     *  minutes, and a search against a deep-research-class model genuinely
     *  takes minutes.
     *
     *  THE PRICE, stated: this is also the lease TTL, so a research run whose
     *  driver was killed mid-step is reclaimable about eleven minutes later
     *  (plus one 30s sweep). That is the cost of never abandoning a step that
     *  might still be spending — and it is measured against the behavior it
     *  replaces, which was to wait forty-five minutes and then mark the run
     *  FAILED. */
    maxStepMs: 11 * 60_000,
  })
}

/** Registered at module load, next to the work — `registerRun`'s rule, and the
 *  reason server/research.ts imports this module statically: an instance that
 *  has not registered the kind cannot RECLAIM a research run it finds, which is
 *  the whole point. */
export const researchRun = registerRun(makeResearchRun(REAL_RESEARCH_DEPS))
