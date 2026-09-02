// WHAT TALARIA ITSELF CAN SUPPLY, right now, on this install.
//
// WHY IT IS ITS OWN FILE. `capability-reach.ts` answers "can the run reach this
// capability" and must stay ignorant of HOW any particular tool works — it reads
// a registry and matches names. This file is the other half: it knows that
// `search` is SearXNG over HTTP and `vision` is a harness pointed at whatever
// model the org put in the vision role, and it asks each of them whether it is
// actually working before offering it. Keeping them apart is what lets
// capability-reach have no dependency on the tool implementations, and lets this
// file import them freely.
//
// EVERY ANSWER HERE IS MEASURED, NEVER ASSUMED. That is the whole discipline.
// A supplier this file names is one a harness is about to hand to a model and
// then GRADE the model on using. Offering a search tool that 503s does not
// produce a model that fails gracefully; it produces a sweep that scores our
// outage as the candidate's inability to research, which is the exact category
// error the capability model exists to prevent. When in doubt, supply nothing:
// an honest "no search server here" is a sentence an admin can act on.
import { searchReachable } from './search'
import { resolveRoleModel } from './model-roles'
import type { PlatformSupply } from './capability-reach'

/** How long a probe of our own tool may take before we call it unavailable.
 *  Short: this runs on the path that opens the fitness page and starts a sweep,
 *  and a supplier that takes five seconds to answer "yes" is one the harness's
 *  own turn budget would have failed on anyway. */
const CHECK_MS = 4_000

export interface PlatformSupplyDeps {
  /** Is the org's web-search backend answering? */
  searchOk: () => Promise<boolean>
  /** Has the org assigned a model to the vision role? */
  visionModel: () => Promise<string | null>
}

const within = async <T,>(ms: number, work: () => Promise<T>, fallback: T): Promise<T> => {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      work(),
      new Promise<T>((resolve) => {
        timer = setTimeout(() => resolve(fallback), ms)
      }),
    ])
  } catch {
    return fallback
  } finally {
    if (timer) clearTimeout(timer)
  }
}

const REAL: PlatformSupplyDeps = {
  searchOk: () => within(CHECK_MS, async () => (await searchReachable()).ok, false),
  visionModel: () => within(CHECK_MS, () => resolveRoleModel('vision'), null),
}

/** THE SERVER NAME TALARIA USES FOR ITS OWN TOOLS. It is shown to admins beside
 *  a `supplied` capability tag ("`talaria.web_search` supplies it"), so it has to
 *  read as a place rather than as an implementation detail. */
export const PLATFORM_SERVER = 'talaria'

/** HOW LONG AN ANSWER IS GOOD FOR.
 *
 *  THIS IS NOT AN OPTIMISATION, IT IS A CORRECTNESS FIX. `reachFor` is on the
 *  RESEARCH HOT PATH — the runner asks it whether this deployment can search
 *  before every research run (the Rust twin's shape:
 *  api/src/runs/defs/research.rs) — so an uncached `platformSupply` put a live
 *  HTTP probe of
 *  SearXNG in front of every research run, and a four-second one in front of
 *  every run where SearXNG was down. The first version of this file did exactly
 *  that and two research tests caught it by hanging.
 *
 *  A minute, because these answers change on the timescale of an admin editing a
 *  setting or a container restarting, and a stale "we can search" costs one
 *  harness run that fails honestly. A stale "we cannot" costs a capability. */
const CACHE_MS = 60_000
let cached: { at: number; value: PlatformSupply[] } | null = null

/** Forget the cached answer. For tests, and for the settings route that changes
 *  the search URL — an admin who has just fixed the URL should not wait out a
 *  minute of the old answer. */
export function forgetPlatformSupply(): void {
  cached = null
}

/** Everything this deployment can stand in for, checked. Empty is a perfectly
 *  ordinary answer — an install with no search backend and no vision role
 *  supplies nothing, and should say so rather than pretend. */
export async function platformSupply(deps?: Partial<PlatformSupplyDeps>): Promise<PlatformSupply[]> {
  // Injected deps mean a test is driving this deliberately; never serve those a
  // cached answer, and never let them poison the cache for anybody else.
  if (!deps && cached && Date.now() - cached.at < CACHE_MS) return cached.value
  const d = { ...REAL, ...deps }
  const [searchOk, visionModel] = await Promise.all([d.searchOk().catch(() => false), d.visionModel().catch(() => null)])
  const out: PlatformSupply[] = []
  // `web_search` is the one tool on Talaria's native surface, and
  // `research-search` is the harness that offers it.
  if (searchOk) out.push({ capability: 'search', server: PLATFORM_SERVER, tool: 'web_search' })
  // `describe_image` reads an image with the ROLE model, so the role being
  // filled is exactly the condition — see `vision.ts`, whose floor refuses
  // rather than degrading when it is not.
  if (visionModel) out.push({ capability: 'vision', server: PLATFORM_SERVER, tool: 'describe_image' })
  if (!deps) cached = { at: Date.now(), value: out }
  return out
}

/** Is this supplier Talaria itself, rather than a registered MCP server? Every
 *  caller that dispatches a tool call has to ask, because the two go to
 *  completely different places. */
export const isPlatformServer = (server: string): boolean => server === PLATFORM_SERVER

/** RUN ONE OF TALARIA'S OWN TOOLS — the other half of `platformSupply`, and the
 *  half whose absence made the first half a lie.
 *
 *  WHAT WENT WRONG WITHOUT IT. `platformSupply` advertised
 *  `{ server: 'talaria', tool: 'web_search' }` and every dispatcher in the tree
 *  sent tool calls to `callMcpTool`, which looks the server up in the MCP
 *  registry and throws `MCP server "talaria" is not registered`. The tool loop
 *  caught that, fed the model `The search tool failed: ...` as the tool RESULT,
 *  and the model — having dutifully called the tool it was offered — answered
 *  from memory anyway. Nothing crashed. The sweep recorded a search stage that
 *  ran, called its tool, and produced no sources.
 *
 *  That is the exact failure this file's header warns about: a supplier we
 *  cannot honor is worse than no supplier, because `null` refuses honestly and
 *  this returns a confident uncited answer. Advertising and dispatch have to
 *  ship together, and now they are in the same file so they cannot drift apart.
 *
 *  Shaped as `McpToolResult` — `{ text, structured }` — because every caller
 *  already speaks it and a platform tool should be indistinguishable from a
 *  registered one at the call site. */
export async function callPlatformTool(tool: string, args: Record<string, unknown>): Promise<{ text: string; structured: unknown }> {
  if (tool === 'web_search') {
    const query = typeof args.query === 'string' ? args.query : ''
    if (!query.trim()) return { text: 'web_search needs a "query" — say what to look up.', structured: null }
    const limit = typeof args.limit === 'number' ? args.limit : undefined
    const { searchWeb } = await import('./search')
    const results = await searchWeb(query, limit !== undefined ? { limit } : {})
    if (results.length === 0) return { text: `No results for "${query}".`, structured: [] }
    // BOTH FORMS, because the two readers want different things: the model reads
    // `text`, and `sourcesFromPayload` reads `structured` to build the citation
    // list. Returning only prose is how a search stage produces findings nobody
    // can trace back to a URL.
    const text = results.map((r, i) => `${i + 1}. ${r.title}\n   ${r.url}\n   ${r.snippet}`).join('\n\n')
    return { text, structured: results }
  }

  if (tool === 'describe_image') {
    // Imported lazily: `vision.ts` pulls in the harness runner, and this module
    // is imported by `capability-reach.ts` which sits on the research and
    // fitness-page paths. A static edge would drag the whole runner into both.
    const { describeImage } = await import('./vision')
    const image = typeof args.image === 'string' ? args.image : ''
    const question = typeof args.question === 'string' ? args.question : ''
    const out = await describeImage({ image, question })
    if (out.error) return { text: out.error, structured: null }
    return { text: out.text, structured: { description: out.text, model: out.model } }
  }

  throw new Error(`"${tool}" is not one of Talaria's own tools`)
}
