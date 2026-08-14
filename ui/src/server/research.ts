// Research runs — Perplexity-grade cited research, Talaria-native.
//
// Three modes, mapped to depth budgets and sonar horsepower:
//   recon       one search pass, minutes — a cited answer
//   brief       planned queries + one synthesis round — a briefing document
//   expedition  iterative plan → search → gap-check rounds — a deep report
//
// Architecture: the pipeline runs SERVER-SIDE, detached from any chat context.
// The requesting agent's own persona (via its persona gateway) does the
// thinking — query planning, gap analysis, synthesis — so a marketing agent
// researches like a marketer; each stage gets a fresh, bounded prompt so raw
// search dumps never swell a conversation window. Search stages run Perplexity
// sonar models through the org LLM gateway (routing + keys + metering), which
// return content WITH source lists; we keep a global, deduped citation
// registry and require [n] markers in the final document.
//
// The report is a doc artifact (versioned, shareable, exportable) linked to
// the run; completion notifies the requester and indexes into the activity
// brain so future chats/plans can retrieve it.
//
// AUDIT 1.5 / 1.6 — WHAT THE HARNESS PORT CHANGED HERE. All three model calls
// (search, plan, synthesize) went out by hand and none of them was guarded, on
// the one path in the product whose defining failure mode is a fabricated
// citation. They are declared in `harness/defs/research.ts` now and run through
// `runHarness`, which owns the model, the capability floor, the parse, the
// repair turn, the guard pass and the ledger. ONE thing stays HERE on purpose
// and it is commented where it happens: the line-based salvage of a query list
// (the tolerance the old extractor had, kept as a declared salvage rather than
// a silent one).
//
// The `ungrounded_ref` pass used to be the second exception — a `guardSynthesis`
// function in this file, running one rule over a hand-built tool record,
// because the runner had no way for a harness to declare what it was grounded
// against. `HarnessDefinition.ground` closed that, so the rule is in
// `researchSynthesisHarness`'s own guard list and this file counts the findings
// the run already reported. Nothing here talks to guardrails.ts any more.
import { agentCategoryFolder, attachArtifact, createArtifact, getArtifact, saveArtifact } from './artifacts'
import { setEditors } from './kb-perms'
import { db } from './db/pg'
import { describeAgent } from './gateway'
import {
  clampQueries,
  queriesFromLines,
  researchQueriesHarness,
  researchSearchHarness,
  researchSynthesisHarness,
  searchTransport,
  toolSearchTransport,
  type ResearchDepth,
  type SearchSource,
} from './harness/defs/research'
import { capabilityKeysFor, runHarness } from './harness/run'
import { reachFor, type Reach } from './capability-reach'
import { gatewayModels } from './llm-gateway'
import { resolveRoleModel } from './model-roles'
import { addNotification } from './notifications'
import { indexActivity, indexPersonal } from './retrieval/sources'
import { generateTitle } from './titler'
import { createConversation, insertUserMessage, nextSeq, touchConversation } from './conversations'
import { continueConversation } from './chat-persist'
import { forgetResearchOrigin, researchOrigin } from './research-origin'
import { resolveRefs } from './refs'

/** Unchanged as a public name and as a type — `ResearchDepth` is declared in
 *  the harness definition because that module cannot import this one, and this
 *  alias keeps every existing importer of `ResearchMode` working. */
export type ResearchMode = ResearchDepth
export type ResearchStatus = 'queued' | 'running' | 'done' | 'error'

export interface ResearchRun {
  id: string
  ownerUserId: string | null
  requestedBy: string
  agentModel: string
  mode: ResearchMode
  question: string
  title: string | null
  status: ResearchStatus
  phase: string | null
  artifactId: string | null
  /** The conversation this run is discussed in. Null until somebody says
   *  something — see `ensureResearchConversation` for why it is on demand. */
  conversationId: string | null
  /** The run this one extends. A follow-up writes into its PARENT's report and
   *  source list rather than minting a second document about one subject; the
   *  child row survives for provenance — who asked what, and what it cost. */
  parentRunId: string | null
  error: string | null
  stats: Record<string, number>
  createdAt: string
  updatedAt: string
  completedAt: string | null
}

export interface ResearchSource {
  idx: number
  url: string
  title: string | null
  snippet: string | null
}

/** Depth budget + search model preference per mode. Query counts bound each
 *  round; rounds bound the expedition loop. Overridable for tests via env. */
const MODES: Record<ResearchMode, { rounds: number; queries: number; blurb: string }> = {
  recon: { rounds: 1, queries: 1, blurb: 'one fast pass — a cited answer in minutes' },
  brief: { rounds: 1, queries: 3, blurb: 'planned angles, one synthesis — a briefing' },
  expedition: { rounds: 3, queries: 4, blurb: 'iterative deep dive — a full report' },
}
const budgetFor = (mode: ResearchMode) => {
  const b = MODES[mode]
  const rounds = Number(process.env.TALARIA_RESEARCH_MAX_ROUNDS ?? 0) || b.rounds
  return { ...b, rounds }
}

export const RESEARCH_MODES: Array<{ mode: ResearchMode; blurb: string }> = (
  Object.entries(MODES) as Array<[ResearchMode, (typeof MODES)[ResearchMode]]>
).map(([mode, b]) => ({ mode, blurb: b.blurb }))

/** A run stuck "running" past this is a crashed pipeline — surfaced as error. */
const STALE_MINUTES = 45

// ── Search stage: sonar through the org gateway, keeping the sources ─────────

interface SearchHit {
  content: string
  sources: SearchSource[]
}

/** WHICH MODEL SEARCHES, AND HOW — resolved once per run, model-agnostic.
 *
 *  WHAT THIS REPLACED, and why it had to. Research used to require a Perplexity
 *  sonar model: `MODES[].search` held a hardcoded list of sonar spellings, and
 *  `startResearch` threw "register a Perplexity sonar model on /models first"
 *  when none was registered. An org running Llama on its own hardware, or
 *  anything that is not sonar, could not research at all — on a platform whose
 *  own SearXNG can search perfectly well for any model that can call a tool.
 *
 *  Talaria also has a standing rule against hardcoded model lists (they rot the
 *  week a vendor renames something), and that list was one.
 *
 *  THE ORDER, and each step is a different question:
 *
 *    1. THE ADMIN'S CHOICE. A `research-<mode>` role assignment is somebody
 *       saying "use this one", and nothing here second-guesses it.
 *    2. A MODEL THAT SEARCHES BY ITSELF, established from CAPABILITY FACTS
 *       rather than from its name — the catalog derives `search` from
 *       `web_search_options` (model-catalog.ts), and a probe can too. This is
 *       where sonar wins when it is registered, without sonar being named.
 *    3. ANYTHING ROUTABLE, PLUS OUR OWN SEARCH. The tool path: our harness
 *       drives a registered search tool or the platform's SearXNG. This is the
 *       step that makes the feature model-agnostic, and it is the common case
 *       on a self-hosted install.
 *
 *  IT FAILS ONLY WHEN BOTH ARE ABSENT, and then it says so in terms an admin can
 *  act on — a search backend OR a search-capable model, not one vendor. */
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

export async function planSearch(mode: ResearchMode, deps?: { models?: () => Promise<Array<{ id: string }>> }): Promise<SearchPlan | null> {
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
export async function searchModelFor(mode: ResearchMode): Promise<string | null> {
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
function adaptBudget(budget: ReturnType<typeof budgetFor>, searchModel: string) {
  if (!isDeepResearchModel(searchModel)) return budget
  return { ...budget, rounds: Math.min(budget.rounds, 2), queries: 1 }
}

/** One search query → the search model's cited answer + its source list.
 *
 *  Signature unchanged, and so is what a THROW means: the round loop catches
 *  per query and records "(search failed: ...)" against that angle, so one dead
 *  query costs one angle rather than the run, and a run where every query died
 *  ends with no citable sources — which the pipeline already treats as a hard
 *  error.
 *
 *  The capability floor is what keeps it honest. The harness declares `requires:
 *  ['search']` and refuses below it, so a model an admin assigned to a
 *  `research-*` role that CANNOT REACH search fails loudly here instead of
 *  answering fluently from training data and handing the synthesis stage an
 *  uncited brief to write up (audit 1.6).
 *
 *  TWO WAYS TO REACH IT, and this stage picks between them:
 *
 *    natively   the model browses. Sources come off the provider's own
 *               `search_results`/`citations` fields — `searchTransport`.
 *    by tool    the model does not browse, but this org has registered a
 *               web-search MCP server and the model calls tools. Sources come
 *               off the tool's payload — `toolSearchTransport`.
 *
 *  WHICH ONE WINS IS NOT THIS FILE'S DECISION — `capability-reach.ts` makes it,
 *  and `planSearch` reads the answer. That matters because this comment used to
 *  say "native wins when both are available", which stopped being true and
 *  contradicted the code one module away for a while. In a codebase where the
 *  comments are the design record, a stale one is worse than none.
 *
 *  What is true, and worth knowing here: NATIVE IS ONLY WORTH PREFERRING WHEN IT
 *  IS ACTUALLY ARMED. `buildUpstream` has to send the provider's activation —
 *  `web_search_options`, a plugin, an `:online` suffix — or "native" degrades
 *  into a plain completion that happens to have been sent to a model that could
 *  have searched, and we harvest whatever citations it volunteered. See
 *  `nativeSearchBody` in llm-gateway.ts.
 *
 *  THE STAGE'S OUTPUT IS IDENTICAL EITHER WAY — prose findings plus a source
 *  list — which is what makes the tool path a real alternative rather than a
 *  downgrade, and why nothing downstream branches on which one ran. */
async function searchStage(model: string, query: string, runId: string, viaTool: { server: string; tool: string } | null): Promise<SearchHit> {
  const sources: SearchSource[] = []
  const run = await runHarness(
    researchSearchHarness,
    { query },
    {
      caller: `research:${runId}`,
      model,
      deps: { transport: viaTool ? toolSearchTransport(runId, sources, viaTool) : searchTransport(runId, sources) },
    },
  )
  // `researchSearchHarness` declares `onFailure: 'throw'` and `runHarness` now
  // honors it on every path that fails to produce a value — the transport and
  // the pre-call ones included — so this is a TYPE narrowing rather than a
  // second copy of the policy. `HarnessResult.value` is `O | null` on every
  // result alike, and the compiler cannot see the guarantee the runner makes.
  if (run.value === null) throw new Error(run.error ?? `search stage produced nothing on "${model}"`)
  return { content: run.value, sources }
}

// ── Persona stages: the agent's own brain plans and synthesizes ──────────────
// Both pin `RunContext.model` to the requesting agent's own persona, which is
// the feature: a marketing agent researches like a marketer. `runHarness` picks
// the fleet transport for it, sends no tools, and guards the reply with an
// honest `Available` for a stream that reports tool names and nothing else.

/** This round's search queries, or an empty list when the persona says the
 *  question is saturated. Never throws — a failed plan is one lost round, and
 *  the loop already treats an empty list as "stop".
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
async function planQueries(agentModel: string, runId: string, question: string, findingsSoFar: string[], max: number): Promise<string[]> {
  const run = await runHarness(
    researchQueriesHarness,
    { question, max, findingsSoFar },
    // Both persona stages of a run carry the same attribution — see the
    // synthesis call for why `source: 'research'` exists at all. An expedition
    // plans up to three times, so leaving this off would make the planning half
    // of a run's cost the half nobody could find.
    { caller: `research:${runId}`, model: agentModel, ledger: { source: 'research', refId: runId } },
  )
  if (run.value !== null) return clampQueries(run.value, max)
  if (run.raw && run.findings.length === 0) return clampQueries(queriesFromLines(run.raw), max)
  return []
}

// ── The citation registry ─────────────────────────────────────────────────────

/** A CITATION MARKER, AND IT IS NOT TWO DIGITS.
 *
 *  `\d{1,2}` was correct for exactly as long as research meant Perplexity: sonar
 *  answers with a handful of pre-ranked sources, so a registry never approached
 *  [99]. Research is model-agnostic now and the tool path is the common one — an
 *  expedition is up to twelve queries against a web-search tool, each returning
 *  a page of results, with every distinct URL numbered. Three figures is
 *  ordinary there.
 *
 *  WHERE IT ACTUALLY BROKE, which is narrower than it first looks and is worth
 *  writing down because the first version of this comment got it wrong. Both
 *  failures are on the REPORT, whose markers are global:
 *
 *    · AN INVENTED [150] SURVIVED. `finishRun` strips markers the registry does
 *      not know; a three-digit one was not matched, so it was neither counted
 *      as dropped nor removed, and it reached the saved document looking exactly
 *      like a real citation.
 *    · THE CITED COUNT UNDERCOUNTED, so a thorough report scored as a thin one,
 *      and `reportProblem` read an all-three-digit report as citing NOTHING.
 *
 *  `SourceRegistry.renumber` was NOT affected and never could be: the markers it
 *  rewrites are LOCAL to one search hit — [1], [2], [3] — and it is the OUTPUT
 *  that carries the global number. Stated because the mutation test that proved
 *  it is easy to read as redundant.
 *
 *  Bounded at three digits rather than left open: `[2024]` in prose is a year,
 *  and matching it would strip dates out of reports. */
const MARKER_RE = /\[(\d{1,3})\]/g

/** Exported for its own test. The renumbering it does is one of the three
 *  places two-digit markers failed silently, and it is not reachable through
 *  `runResearch` without standing up the whole pipeline. */
/** THE SOURCES SECTION THE PIPELINE APPENDS, matched so it can be replaced.
 *
 *  Anchored to a line start and to the END of the document, because a report may
 *  legitimately discuss the word "Sources" in its prose and a loose match would
 *  truncate a report at the first mention of one. */
const SOURCES_SECTION = /\n*^## Sources\s*$[\s\S]*$/m

/** The report's prose, without the mechanical source list. */
export const reportBodyOnly = (body: string): string => body.replace(SOURCES_SECTION, '').trimEnd()

/** EXTEND A REPORT WITH WHAT A FOLLOW-UP FOUND, keeping it one document.
 *
 *  A follow-up used to produce a SECOND report about the same subject, with its
 *  own source numbering, and nothing linking the two. The answer to one question
 *  lived in two places and the reader assembled it.
 *
 *  WHAT THIS HAS TO GET RIGHT, and each one silently corrupts a document if it
 *  does not:
 *
 *    THE OLD PROSE IS UNTOUCHED. Somebody has read it and may have quoted it.
 *    THE SOURCE LIST IS REBUILT, not appended to — it is mechanical output, and
 *      two of them at the bottom of one document is the failure that made the
 *      old `## Sources` regex worth anchoring.
 *    CITED IS RECOMPUTED OVER THE WHOLE DOCUMENT. A source the parent cites and
 *      the follow-up does not must not become "(consulted)" because this pass
 *      only looked at the new section.
 *    THE NEW SECTION SAYS WHAT ASKED FOR IT. A reader coming back a week later
 *      needs to see that the last three paragraphs answer a different question
 *      than the top of the document does. */
export function extendReport(
  parentBody: string,
  addition: { question: string; markdown: string },
  sources: readonly ResearchSource[],
): string {
  const prose = reportBodyOnly(parentBody)
  // The follow-up's own `# Title` is dropped: the document already has one, and
  // a second H1 mid-document reads as a new document to every renderer and
  // every table of contents.
  const section = addition.markdown.trim().replace(/^#\s+.*$/m, '').trim()
  const heading = `## Follow-up: ${addition.question.trim()}`
  const merged = `${prose}\n\n${heading}\n\n${section}`
  const cited = new Set([...merged.matchAll(MARKER_RE)].map((m) => Number(m[1])))
  const list = sources
    .slice()
    .sort((a, b) => a.idx - b.idx)
    .map((s) => `${s.idx}. [${s.title ?? s.url}](${s.url})${cited.has(s.idx) ? '' : ' *(consulted)*'}`)
    .join('\n')
  return `${merged}\n\n## Sources\n\n${list}\n`
}

/** KEEP ONLY THE CITATIONS THAT RESOLVE, and count what was thrown away.
 *
 *  Extracted from the pipeline so it can be tested at all: it is the one place
 *  an invented citation is caught before a human reads the report, and it sat
 *  inline in a function that needs a database, a gateway and an artifact store
 *  to reach.
 *
 *  `dropped` is COUNTED, not merely stripped. Deleting an invented citation is
 *  the right thing to save, but it also made a model that fabricates half its
 *  markers look identical to one that cites perfectly — the exact model-fitness
 *  signal this run is in the best position to report, thrown away by the line
 *  that fixed the symptom. */
export function stripUnknownMarkers(doc: string, knownIdx: readonly number[]): { cleaned: string; dropped: number; cited: Set<number> } {
  const known = new Set(knownIdx)
  const dropped = [...doc.matchAll(MARKER_RE)].filter((m) => !known.has(Number(m[1]))).length
  const cleaned = doc
    .trim()
    .replace(/^```[a-z]*\n?|\n?```$/g, '')
    .replace(MARKER_RE, (m, n) => (known.has(Number(n)) ? m : ''))
  const cited = new Set([...cleaned.matchAll(MARKER_RE)].map((m) => Number(m[1])))
  return { cleaned, dropped, cited }
}

export class SourceRegistry {
  private byUrl = new Map<string, { idx: number; title: string | null; snippet: string | null }>()

  /** SEED FROM A REPORT ALREADY WRITTEN, so a follow-up continues its numbering
   *  instead of starting again at [1].
   *
   *  THIS IS WHAT KEEPS THE OLD TEXT TRUE. Every [n] in the parent's prose
   *  points at a row in its source list; renumbering — or reusing [3] for a new
   *  URL — would silently re-aim citations that a human already read and
   *  believed. So the parent's indices are taken verbatim and new sources
   *  continue from the highest, whatever gaps that leaves. */
  static from(sources: readonly ResearchSource[]): SourceRegistry {
    const reg = new SourceRegistry()
    for (const s of sources) reg.byUrl.set(s.url, { idx: s.idx, title: s.title, snippet: s.snippet })
    return reg
  }
  add(s: { url: string; title: string | null; snippet: string | null }): number {
    const existing = this.byUrl.get(s.url)
    if (existing) {
      if (!existing.title && s.title) existing.title = s.title
      return existing.idx
    }
    // HIGHEST + 1, not size + 1. A seeded registry can carry gaps — a parent
    // whose source [4] was deleted leaves size 3 and a highest of 5 — and
    // `size + 1` would hand [4] to a brand new URL, quietly re-aiming every
    // citation the parent's text makes to [4].
    const idx = Math.max(0, ...[...this.byUrl.values()].map((v) => v.idx)) + 1
    this.byUrl.set(s.url, { idx, title: s.title, snippet: s.snippet })
    return idx
  }
  /** Rewrite one search hit's LOCAL [n] markers onto global numbering. */
  renumber(hit: SearchHit): string {
    const map = new Map<number, number>()
    hit.sources.forEach((s, i) => map.set(i + 1, this.add(s)))
    return hit.content.replace(MARKER_RE, (m, n) => {
      const g = map.get(Number(n))
      return g ? `[${g}]` : m
    })
  }
  list(): ResearchSource[] {
    return [...this.byUrl.entries()].map(([url, s]) => ({ idx: s.idx, url, title: s.title, snippet: s.snippet }))
  }
  get size(): number {
    return this.byUrl.size
  }
}

/** WHY A RUN ENDED WITH NOTHING TO CITE, in the words of the person who has to
 *  fix it. The sentence up to the em dash is load-bearing — `fitness/evals.ts`
 *  matches on it to tell a model's failure apart from a deployment's — and what
 *  follows it is the part that saves an evening.
 *
 *  IT EXISTS BECAUSE THE OLD MESSAGE BLAMED THE WRONG THING. "Search returned
 *  nothing citable" reads as "the search backend found nothing", so the first
 *  move it invites is checking the backend — and on the run that prompted this,
 *  the backend was healthy and answering the very queries the run had planned.
 *  Nothing had searched at all: a model recorded as searching natively was sent
 *  a plain completion and wrote three thousand tokens from memory. A run that
 *  never searched and a run that searched and found nothing are different
 *  problems with different fixes, and only one of them is about the backend. */
function noSourcesReason(viaTool: { server: string; tool: string } | null, searchModel: string, searchFailed: boolean): string {
  const base = 'no sources found — search returned nothing citable'
  if (searchFailed) return `${base}: every search query for this run errored, so nothing reached the registry.`
  if (viaTool) {
    return `${base}: "${viaTool.server}.${viaTool.tool}" was called for every query and no result carried a URL. This one IS the search backend — check that it is answering.`
  }
  return (
    `${base}: "${searchModel}" was asked to search natively and its replies carried no citations, which means it answered from memory rather than searching. ` +
    `Assign a model whose provider returns citations on a plain completion (the sonar family), or register a web-search tool this model can call.`
  )
}

/** The artifact as a ref chip, checked against the person whose conversation it
 *  is about to appear in. Empty when the conversation is gone or they cannot
 *  read it — `resolveRefs` already drops what it must, and an attachment is not
 *  worth failing a delivery over. */
async function refsForConversation(conversationId: string, artifactId: string): Promise<unknown[]> {
  try {
    const sql = await db()
    const rows = (await sql`
      select u.id, u.email, u.name from conversations c join users u on u.id = c.user_id where c.id = ${conversationId}
    `) as unknown as Array<{ id: string; email: string | null; name: string | null }>
    const reader = rows[0]
    if (!reader) return []
    return await resolveRefs(reader, [{ type: 'artifact', id: artifactId }])
  } catch {
    return []
  }
}

/** TELL THE CONVERSATION THAT ASKED, and let the agent take it from there.
 *
 *  A run started from a chat used to end in silence. The tool description says
 *  "poll research_status", but a chat turn has no way to wait out a six-minute
 *  brief: the agent checks a few times, sees `running`, and its turn ends. The
 *  only completion signal was `addNotification(ownerUserId, …)`, and
 *  `ownerUserId` is set for PERSONAL ASSISTANTS only — so for a departmental
 *  agent, which is most of them, nothing at all was told.
 *
 *  WHAT LANDS: a user-role message carrying the outcome, then a normal
 *  continuation. The agent answers with the whole conversation still in front of
 *  it, so it reports in its own voice and can pick up whatever it was doing —
 *  which is the difference between being told and merely being notified. It has
 *  `get_document` if it wants to read the report before summarizing.
 *
 *  WHY THE NOTE IS ROLE `user` rather than a system line: `continueConversation`
 *  starts a turn only when the last message is the user's, and that rule is not
 *  an accident to route around — it is what stops a conversation talking to
 *  itself. The `metadata.kind` marks it as platform-generated so the UI can
 *  render it as an event rather than as something the human typed, and
 *  `authorUserId` stays null for the same reason.
 *
 *  NEVER THROWS, AND NEVER BLOCKS THE RUN. The report is saved and the run is
 *  already `done` by the time this is called; a delivery failure must not undo
 *  that. If the agent happens to be mid-reply the continuation no-ops, and the
 *  note simply sits at the end of the conversation for the next turn to cover —
 *  the same queued-message chaining every other path relies on. */
async function tellTheAskingConversation(runId: string, agentModel: string, note: string, artifactId?: string): Promise<void> {
  const convId = await researchOrigin(runId)
  // Null is ordinary: started from the Research page, by a cron, or long enough
  // ago that the key expired. Nobody is waiting in a chat.
  if (!convId) return
  try {
    // THE REPORT ITSELF, ATTACHED — not a sentence about where it was filed.
    // The first run through this path ended with the agent saying the document
    // was "in Documents under Engineering", which is a description a person then
    // has to go and act on, and was wrong about the folder besides. A ref chip
    // is the platform's own object: the UI renders it as a card that opens the
    // artifact, and `refBlocks` feeds its text to the agent on this turn and
    // every later history rebuild, so the agent can summarize the report without
    // a `get_document` round trip.
    //
    // ACL-CHECKED AGAINST THE READER, like every other ref: `resolveRefs` drops
    // anything the conversation's owner cannot read, so a chip can never be the
    // thing that discloses a private artifact.
    const attachments = artifactId ? await refsForConversation(convId, artifactId) : []
    await insertUserMessage(convId, await nextSeq(convId), note, attachments, null, { kind: 'research-complete', runId })
    await touchConversation(convId)
    await continueConversation(convId, { agentModel })
  } catch (e) {
    console.error(`[research] could not report run ${runId} back to conversation ${convId}:`, (e as Error).message)
  } finally {
    await forgetResearchOrigin(runId)
  }
}

// ── Run lifecycle ─────────────────────────────────────────────────────────────

const ROW = `id, owner_user_id as "ownerUserId", requested_by as "requestedBy", agent_model as "agentModel",
  mode, question, title, status, phase, artifact_id as "artifactId", error, stats, conversation_id as "conversationId",
  parent_run_id as "parentRunId",
  created_at as "createdAt", updated_at as "updatedAt", completed_at as "completedAt"`

/** Runs a viewer may see: their own, ones shared with them, and org runs
 *  (no owner — general agents researching for the org). null viewer (a
 *  general agent) sees org runs only. */
export async function listResearchRuns(viewerUserId: string | null, limit = 60): Promise<ResearchRun[]> {
  const sql = await db()
  await sweepStale()
  if (viewerUserId === null) {
    return (await sql.unsafe(`select ${ROW} from research_runs where owner_user_id is null order by created_at desc limit $1`, [limit])) as unknown as ResearchRun[]
  }
  return (await sql.unsafe(
    `select ${ROW} from research_runs
     where owner_user_id is null or owner_user_id = $1
        or exists(select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = $1)
     order by created_at desc limit $2`,
    [viewerUserId, limit],
  )) as unknown as ResearchRun[]
}

/** The viewer's standing on a run: owner, member (incl. org runs), or none. */
export async function researchRole(viewerUserId: string | null, runId: string): Promise<'owner' | 'member' | null> {
  const sql = await db()
  const rows = (await sql`
    select owner_user_id as "ownerUserId",
           exists(select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = ${viewerUserId}) as member
    from research_runs where id = ${runId}
  `) as unknown as Array<{ ownerUserId: string | null; member: boolean }>
  const r = rows[0]
  if (!r) return null
  if (r.ownerUserId === null) return 'member' // org run — anyone signed in
  if (viewerUserId && r.ownerUserId === viewerUserId) return 'owner'
  return r.member ? 'member' : null
}

export interface ResearchMember {
  userId: string
  name: string | null
  email: string | null
  role: 'owner' | 'collaborator'
}

export async function listResearchMembers(runId: string): Promise<ResearchMember[]> {
  const sql = await db()
  return (await sql`
    select u.id as "userId", u.name, u.email, 'owner' as role
    from research_runs r join users u on u.id = r.owner_user_id where r.id = ${runId}
    union all
    select u.id as "userId", u.name, u.email, 'collaborator' as role
    from research_members rm join users u on u.id = rm.user_id where rm.run_id = ${runId}
  `) as unknown as ResearchMember[]
}

export async function addResearchMember(runId: string, userId: string): Promise<void> {
  const sql = await db()
  await sql`insert into research_members (run_id, user_id) values (${runId}, ${userId}) on conflict do nothing`
}

export async function removeResearchMember(runId: string, userId: string): Promise<void> {
  const sql = await db()
  await sql`delete from research_members where run_id = ${runId} and user_id = ${userId}`
}

/** The run's report artifact (via artifact_links), if it exists yet. */
export async function researchArtifactFor(runId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select artifact_id as id from artifact_links where target_type = 'research' and target_id = ${runId} limit 1
  `) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
}

export async function getResearchRun(id: string): Promise<{ run: ResearchRun; sources: ResearchSource[] } | null> {
  const sql = await db()
  await sweepStale()
  const rows = (await sql.unsafe(`select ${ROW} from research_runs where id = $1`, [id])) as unknown as ResearchRun[]
  if (!rows[0]) return null
  const sources = (await sql`
    select idx, url, title, snippet from research_sources where run_id = ${id} order by idx asc
  `) as unknown as ResearchSource[]
  return { run: rows[0], sources }
}

async function sweepStale(): Promise<void> {
  const sql = await db()
  await sql`
    update research_runs set status = 'error', error = 'run went stale (app restarted mid-research?)', updated_at = now()
    where status in ('queued', 'running') and updated_at < now() - make_interval(mins => ${STALE_MINUTES})
  `
}

async function setPhase(runId: string, phase: string): Promise<void> {
  const sql = await db()
  await sql`update research_runs set phase = ${phase}, status = 'running', updated_at = now() where id = ${runId}`
}

/** Create a run and kick the pipeline detached. Returns the queued run. */
export async function startResearch(input: {
  question: string
  mode: ResearchMode
  agentModel: string
  ownerUserId: string | null
  requestedBy: string
  /** The run this extends. Its findings are written into that run's report and
   *  numbered against its sources — see `extendReport`. */
  parentRunId?: string | null
}): Promise<ResearchRun> {
  // REFUSES ONLY WHEN THE WORKSPACE GENUINELY CANNOT SEARCH — no search backend
  // AND no model that searches by itself. It used to refuse whenever no
  // Perplexity sonar model was registered, which is a different and much
  // narrower condition: an org with SearXNG and any tool-calling model can
  // research perfectly well, and was being told to go and buy something.
  const plan = await planSearch(input.mode)
  if (!plan) throw new Error(NO_SEARCH_REASON)
  const sql = await db()
  const rows = (await sql`
    insert into research_runs (owner_user_id, requested_by, agent_model, mode, question, parent_run_id)
    values (${input.ownerUserId}, ${input.requestedBy}, ${input.agentModel}, ${input.mode}, ${input.question}, ${input.parentRunId ?? null})
    returning ${sql.unsafe(ROW)}
  `) as unknown as ResearchRun[]
  const run = rows[0]!
  // The Titler names the run from its question — fire-and-forget, the list
  // shows the raw question until the title lands.
  void generateTitle('research', input.question)
    .then(async (t) => {
      if (t) await sql`update research_runs set title = ${t} where id = ${run.id}`
    })
    .catch(() => {})
  void runResearch(run.id).catch(() => {})
  return run
}

/** The pipeline. Never throws — failures land on the run row. */
async function runResearch(runId: string): Promise<void> {
  const sql = await db()
  try {
    const got = await getResearchRun(runId)
    if (!got || got.run.status === 'error') return
    const { question, mode, agentModel, ownerUserId, requestedBy } = got.run
    // ONE DECISION, MADE ONCE, and both halves come from it. Resolving the
    // model here and the PATH separately a few lines later was how the two came
    // to be able to disagree — `planSearch` answers both together, off the same
    // capability read.
    // THROWN, not handled here: the catch below already records the error on
    // the run AND tells the waiting agent why, which is the whole point of that
    // block. A second failure path would be a second place for the two to
    // disagree. This can only fire if the org's search config changed between
    // the run being queued and being picked up — `startResearch` refuses first.
    const plan = await planSearch(mode)
    if (!plan) throw new Error(NO_SEARCH_REASON)
    const searchModel = plan.model
    const budget = adaptBudget(budgetFor(mode), searchModel)
    const searchTool = plan.via === 'tool' ? plan.supplier : null
    const agentLabel = describeAgent(agentModel).label
    // A FOLLOW-UP CONTINUES ITS PARENT'S NUMBERING. Seeded from the parent's
    // source list so [1]..[n] keep meaning what the already-written prose says
    // they mean, and anything new starts above the highest.
    const parent = got.run.parentRunId ? await getResearchRun(got.run.parentRunId) : null
    const registry = parent ? SourceRegistry.from(parent.sources) : new SourceRegistry()
    const notes: string[] = []
    let queriesRun = 0
    let searchFailed = false

    // Round loop: plan queries (persona), search each (sonar), then — in
    // expedition — let the persona name what's still missing and go again.
    let nextQueries: string[] = mode === 'recon' ? [question] : []
    for (let round = 1; round <= budget.rounds; round++) {
      if (nextQueries.length === 0) {
        await setPhase(runId, round === 1 ? 'planning search angles' : `round ${round}: chasing gaps`)
        nextQueries = await planQueries(agentModel, runId, question, round === 1 ? [] : notes, budget.queries)
        if (nextQueries.length === 0) break // the persona says we're saturated
      }
      for (const q of nextQueries) {
        queriesRun++
        await setPhase(runId, `searching (${queriesRun}): ${q.slice(0, 80)}`)
        try {
          const hit = await searchStage(searchModel, q, runId, searchTool)
          notes.push(`### Query: ${q}\n${registry.renumber(hit)}`)
        } catch (e) {
          searchFailed = true
          notes.push(`### Query: ${q}\n(search failed: ${(e as Error).message})`)
        }
      }
      nextQueries = []
    }

    if (registry.size === 0) throw new Error(noSourcesReason(searchTool, searchModel, searchFailed))

    // Synthesis: the persona writes the document against the global registry.
    // Throws on an unusable reply rather than saving an empty report — the
    // searches are already paid for and this document is the run's only
    // deliverable, so "keep what you had" is not an option here.
    //
    // THE GROUNDING PASS IS INSIDE THIS CALL. `searchFailed` and the findings
    // are on the INPUT because `researchSynthesisHarness` declares `ground`
    // (see its comment): the search hits are this turn's tool results, so
    // `ungrounded_ref` — the definitive research failure mode — runs on the
    // report, from the runner, with one findings row per fabricated link. This
    // adapter used to run that rule itself in a `guardSynthesis` function; the
    // rule moved into the harness's declared list and the function is gone.
    await setPhase(runId, 'writing the report')
    const synthesis = await runHarness(
      researchSynthesisHarness,
      { question, mode, sources: registry.list().map((s) => ({ idx: s.idx, url: s.url, title: s.title })), findings: notes, searchFailed },
      // `source: 'research'` + the run id is what makes a run's COST answerable.
      // The persona stages used to meter as an ownerless chat turn, so research
      // spend was indistinguishable from every other persona call in
      // `usage_events`. The search stages meter themselves through the gateway.
      { caller: `research:${runId}`, model: agentModel, ledger: { source: 'research', refId: runId } },
    )
    // A TYPE NARROWING, not a policy — `researchSynthesisHarness` declares
    // `onFailure: 'throw'` and the runner honors it on every path that fails to
    // produce a value. It used to be a policy, restated here by hand, because
    // 'throw' covered a CONTRACT failure and nothing else: a persona gateway
    // answering 502 mid-synthesis arrived as `value: null`, and the `?? ''` this
    // replaced saved an artifact containing only the Sources list, marked the
    // run `done`, indexed the empty report and notified the requester — with the
    // gateway's own sentence dropped on the floor.
    if (synthesis.value === null) throw new Error(synthesis.error ?? 'the report came back empty')
    const doc = synthesis.value

    // Keep only markers that resolve; append the mechanical Sources section.
    //
    // `dropped` is counted rather than only stripped. Deleting an invented
    // citation is the right thing to save, but it also made a model that
    // fabricates half its markers look identical to one that cites perfectly —
    // the exact model-fitness signal this run is in the best position to
    // report, thrown away by the line that fixed the symptom.
    const { cleaned, dropped, cited } = stripUnknownMarkers(doc, registry.list().map((s) => s.idx))

    // The stat, read off the run rather than off a second guard pass. Filtered
    // to `ungrounded_ref` because `synthesis.findings` also carries the
    // `secret_leak` / `pii_leak` hits, and this number has one meaning on the
    // research surface: how many links this model asserted that no search
    // result backs. `dropped` above is its deterministic sibling — invented [n]
    // markers — and the two are counted separately on purpose.
    //
    // The guard ran on the RAW reply, not on `cleaned`. That is not a widening:
    // `ungrounded_ref` looks for URLs on policed hosts and for UUIDs, and
    // `cleaned` only strips code fences and unresolvable [n] markers, neither of
    // which can be either.
    const ungrounded = synthesis.findings.filter((f) => f.check === 'ungrounded_ref').length
    const sourcesMd = registry
      .list()
      .map((s) => `${s.idx}. [${s.title ?? s.url}](${s.url})${cited.has(s.idx) ? '' : ' *(consulted)*'}`)
      .join('\n')
    const body = `${cleaned}\n\n## Sources\n\n${sourcesMd}\n`
    const title = cleaned.match(/^# (.+)$/m)?.[1]?.trim() ?? `Research — ${question.slice(0, 80)}`


    // The report is a real artifact, filed under the researching agent's
    // cabinet. Ownership decides reach: an ORG run (no owner) publishes
    // org-visible; a user's run stays PRIVATE to them, with members granted
    // editor on the doc — sharing the run is the only way anyone else sees it.
    // ── ONE SUBJECT, ONE DOCUMENT ────────────────────────────────────────────
    //
    // A follow-up writes into the report it came from instead of minting a
    // second one about the same subject — which is what made "dig into the
    // second point" produce two documents that did not reference each other,
    // with two source lists numbered from [1], and left the reader to assemble
    // the answer.
    //
    // ONLY THE DESTINATION BRANCHES. Everything after this — the sources rows,
    // the stats, indexing, the notification, the report back into the
    // conversation — is identical for both, and duplicating it here to save one
    // conditional is how the two paths would drift.
    const parentArtifactId = parent?.run.artifactId ?? null
    const artifact = parentArtifactId
      ? { id: parentArtifactId }
      : await createArtifact({
          kind: 'doc',
          title,
          createdBy: requestedBy,
          ownerUserId,
          folderId: await agentCategoryFolder(agentLabel, 'Research', requestedBy),
        })

    if (parentArtifactId) {
      // Read the parent's CURRENT body rather than anything cached: a human may
      // have edited the report between the follow-up being asked for and it
      // finishing, and overwriting that would be the worst outcome here.
      const current = (await getArtifact(parentArtifactId))?.body ?? ''
      await saveArtifact(parentArtifactId, { body: extendReport(current, { question, markdown: cleaned }, registry.list()) }, agentLabel)
    } else {
      await saveArtifact(artifact.id, { body, visibility: ownerUserId ? 'private' : 'org' }, agentLabel)
      if (ownerUserId) {
        const members = (await sql`select user_id as id from research_members where run_id = ${runId}`) as unknown as Array<{ id: string }>
        if (members.length) {
          await setEditors('artifact', artifact.id, members.map((m) => ({ principalType: 'user' as const, principalId: m.id, role: 'editor' as const }))).catch(() => {})
        }
      }
      await attachArtifact(artifact.id, { targetType: 'research', targetId: runId }, agentLabel)
    }

    // SOURCES BELONG TO THE REPORT'S RUN. When extending, the numbering they
    // were given is the parent's and the report's citations resolve against the
    // parent's list — writing them under the child would leave the document
    // citing rows nothing can find.
    const sourceRunId = parent?.run.id ?? runId
    for (const s of registry.list()) {
      await sql`
        insert into research_sources (run_id, idx, url, title, snippet)
        values (${sourceRunId}, ${s.idx}, ${s.url}, ${s.title}, ${s.snippet}) on conflict do nothing
      `
    }
    await sql`
      update research_runs set status = 'done', phase = null, artifact_id = ${artifact.id},
        stats = ${sql.json({ queries: queriesRun, sources: registry.size, cited: cited.size, dropped, ungrounded })},
        updated_at = now(), completed_at = now()
      where id = ${runId}
    `

    // Placement follows ownership: a personal run's report goes to the owner's
    // private brain (them + their assistant); an org-wide run's report goes to
    // the ambient index, marked orgWide so scopes actually match it.
    const reportDoc = {
      sourceType: 'research',
      sourceId: artifact.id,
      title,
      text: `${title}\n\n${body}`,
      payload: ownerUserId ? { runId, question, mode } : { runId, question, mode, orgWide: true },
      href: `/research?r=${runId}`,
    }
    if (ownerUserId) void indexPersonal(ownerUserId, reportDoc).catch(() => {})
    else void indexActivity(reportDoc).catch(() => {})
    if (ownerUserId) {
      void addNotification(ownerUserId, {
        kind: 'research',
        title: `Research ready: ${title.slice(0, 120)}`,
        body: `${agentLabel} finished ${mode === 'recon' ? 'a recon' : mode === 'brief' ? 'a brief' : 'an expedition'} — ${registry.size} sources`,
        href: `/research?r=${runId}`,
      }).catch(() => {})
    }

    // THE REPORT IS ATTACHED, and the note says so — which is what stops the
    // agent reaching for prose directions. Told only that a document exists
    // somewhere, a model writes "it's filed under Engineering" and the reader
    // has to go hunting; told that the reader can already see the card, it
    // spends its words on what the research actually found. The id is still here
    // for `get_document`, because a long report is worth opening even when its
    // first six thousand characters arrived on the chip.
    await tellTheAskingConversation(
      runId,
      agentModel,
      `Your ${mode} on "${question}" has finished: ${registry.size} source${registry.size === 1 ? '' : 's'}, ` +
        `${cited.size} cited. The report "${title}" is attached to this message as a card the reader can open (documentId ${artifact.id}).` +
        (searchFailed ? ' Some search queries failed, so the picture may be incomplete.' : '') +
        ' Tell whoever asked WHAT YOU FOUND — they can already open the report, so do not describe where it is filed.',
      artifact.id,
    )
  } catch (e) {
    const message = (e as Error).message
    await sql`
      update research_runs set status = 'error', phase = null, error = ${message.slice(0, 2000)}, updated_at = now()
      where id = ${runId}
    `
    // A FAILED RUN IS REPORTED TOO, and this is the half that matters most for
    // the person waiting: the old silence was indistinguishable from a run still
    // working, so an agent asked to research something could only keep saying it
    // was checking. The agent gets the real reason and can decide whether to try
    // a different angle or say plainly that it could not find out.
    const got = await getResearchRun(runId).catch(() => null)
    if (got) {
      await tellTheAskingConversation(
        runId,
        got.run.agentModel,
        `Your ${got.run.mode} on "${got.run.question}" failed: ${message} Tell whoever asked, in plain words, and say what you can still do.`,
      )
    }
  }
}

// ── The conversation a run is discussed in ───────────────────────────────────
//
// A research run used to be a one-shot: ask, wait, read. The only thing anyone
// could do afterwards was read it again, and the two colleagues it was shared
// with had nowhere to say "dig into the second point" or "this source is a
// vendor blog". A view whose whole content is a document does not need to be a
// view.
//
// So a run gets a conversation of its own, exactly the shape a plan has —
// several people, one agent, one document that grows beside the talk.

/** WHAT THE AGENT IS FOR IN THIS ROOM.
 *
 *  It already knows the report: the findings, the numbered sources and what each
 *  one supports. Most turns here are that — answering from what was found, which
 *  costs one call and no searching.
 *
 *  THE RULE THAT MATTERS is the one about not answering past the evidence. This
 *  is the surface where a confident sentence is most likely to be believed,
 *  because everything around it carries a citation: the report is cited, the
 *  sources are listed, and a teammate skim-reading has no way to tell which
 *  sentence came from a source and which came from the model. So an answer the
 *  report does not support has to say so and offer to go and find out — which is
 *  a real offer, because commissioning more research is a tool it holds.
 *
 *  IT DOES NOT SEARCH BY REFLEX. A follow-up run costs minutes and money, and a
 *  question the report already answers is not a reason to spend either. */
export const RESEARCH_MODE_PROMPT = `This is a conversation ABOUT a research report you produced, on the Research surface. Several teammates may be in it; the report and its numbered sources sit beside the chat.

Answer from the report and its sources first. Cite the same [n] markers the report uses so anyone can check you, and never state something as established that the sources do not support — this is a surface where everything looks cited, so an uncited claim of yours will be read as a finding.

When the answer genuinely is not in what was found, say so plainly and offer to look into it. If a teammate asks you to dig further, commission follow-up research with the research tool rather than answering from memory; its findings are added to the same report, so the document stays the one place the answer lives.

Do not re-research something the report already covers — say what it says and point at the section.`

/** The conversation for a run, created on first use.
 *
 *  ON DEMAND rather than at run creation, for a reason worth stating: most runs
 *  are read once and never discussed, and a conversation row per run would make
 *  the chat list a list of things nobody said anything about. The first message
 *  is what makes it exist.
 *
 *  Owned by the run's owner and pinned to the agent that DID the research, so
 *  the teammate answering questions about the report is the one that wrote it. */
export async function ensureResearchConversation(runId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select conversation_id as "conversationId", owner_user_id as "ownerUserId", agent_model as "agentModel", question, title
      from research_runs where id = ${runId}
  `) as unknown as Array<{ conversationId: string | null; ownerUserId: string | null; agentModel: string; question: string; title: string | null }>
  const run = rows[0]
  if (!run) return null
  if (run.conversationId) return run.conversationId
  // A run started by an agent with no human owner has nobody to own the
  // conversation. It is still readable; it just cannot be talked in.
  if (!run.ownerUserId) return null

  const id = await createConversation(run.ownerUserId, run.agentModel, run.title ?? run.question.slice(0, 80), 'research', null)
  await sql`update research_runs set conversation_id = ${id} where id = ${runId} and conversation_id is null`
  // Re-read rather than trusting the write: two clients opening the same run at
  // once both get here, and the loser must return the winner's conversation
  // instead of a second one nobody is reading.
  const after = (await sql`select conversation_id as "conversationId" from research_runs where id = ${runId}`) as unknown as Array<{
    conversationId: string | null
  }>
  return after[0]?.conversationId ?? id
}

/** The run a conversation belongs to, for the surfaces that start from the chat
 *  side rather than the report side. */
export async function researchRunForConversation(conversationId: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select id from research_runs where conversation_id = ${conversationId}`) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
}
