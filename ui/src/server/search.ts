// LIVE WEB SEARCH, SUPPLIED BY THE DEPLOYMENT.
//
// WHY TALARIA OWNS THIS RATHER THAN REGISTERING SOMEBODY'S MCP SERVER. The
// capability model already knows how to route around a model that cannot
// search: `supplierFor('search', …)` finds a registered tool, and
// `toolSearchTransport` drives it so a blind model still produces cited
// findings. What was missing was a tool to find. Every option was either a
// hosted API with a key and a per-query bill, or a thin third-party MCP bridge —
// a whole extra process and an unvetted dependency in the request path, for
// about fifty lines of `fetch`.
//
// So the ENGINE is SearXNG and the CLIENT is this file. SearXNG is a metasearch
// aggregator: it fans one query across dozens of engines, needs no API key,
// costs nothing per query, and no query leaves the operator's infrastructure.
// Its JSON API returns `title/url/content/engine` per result, which is already
// the shape `sourcesFromPayload` walks.
//
// THE LICENCE DECIDED THE ARCHITECTURE, so it is written down here rather than
// left for somebody to rediscover. Talaria is MIT; SearXNG is AGPL-3.0. Vendoring
// AGPL source into an MIT codebase propagates the terms, so SearXNG runs as its
// own container and Talaria talks to it over HTTP — the arrangement the fleet
// MCP service already uses. Two consequences worth keeping:
//
//   DO NOT PATCH SEARXNG. AGPL §13 obliges anyone OFFERING it as a network
//   service to offer its source to users of that service. Unmodified upstream
//   satisfies that by pointing at the upstream repository; a local patch means
//   publishing the patch.
//
//   DO NOT IMPORT ITS CODE. Configure it. Everything this file needs is on the
//   documented JSON API.
//
// TWO CONFIGURATION FACTS that cost an afternoon each if you meet them cold, and
// which `docker/searxng/settings.yml` sets for you: the JSON format is OFF by
// default and a request for it 403s until `search.formats` includes `json`; and
// the rate limiter throttles our own agents unless it is off for an internal
// instance.
//
// BOTH LIVE IN A MOUNTED settings.yml, NOT IN ENV VARS, and that is worth
// knowing before you try the obvious thing: SearXNG honours environment
// variables for a short allow-list only (base URL, secret key). Setting
// `SEARXNG_SEARCH_FORMATS` is silently ignored, and the instance goes on
// answering 403 while looking correctly configured.
import { getSetting } from './audit'

/** Where SearXNG lives. Env first so a self-host can point at an instance it
 *  already runs; the setting is the in-app override; the default is the service
 *  name in Talaria's own compose file. */
export const SEARCH_URL_KEY = 'search_url'
const DEFAULT_URL = 'http://127.0.0.1:8888'

export async function searchUrl(): Promise<string> {
  const configured = process.env.SEARXNG_URL ?? (await getSetting<string>(SEARCH_URL_KEY, ''))
  return (configured || DEFAULT_URL).replace(/\/$/, '')
}

export interface SearchResult {
  title: string
  url: string
  /** The engine's own snippet. Empty rather than null — every consumer
   *  concatenates it, and a null in that position is a `"null"` in a prompt. */
  snippet: string
  /** Which upstream engine produced it. Kept because a result every engine
   *  agrees on is worth more than one only a single engine returned, and
   *  because it is how an operator notices an engine has stopped answering. */
  engine: string
}

/** RESULTS PER QUERY. Ten is what a research stage can actually read: the
 *  synthesis turn has to fit them all in one prompt alongside the question, and
 *  a model handed fifty passages cites the first five. */
export const DEFAULT_LIMIT = 10
const MAX_LIMIT = 25

/** How long we wait on the search engine. SearXNG fans out to a dozen upstreams
 *  and returns when they do, so this is a real ceiling rather than a formality —
 *  and it must be well under a harness turn budget, because a search that
 *  outlives the turn that asked for it is a timeout charged to the model. */
const TIMEOUT_MS = 20_000

export interface SearchDeps {
  fetch: typeof globalThis.fetch
  url: () => Promise<string>
}

const REAL: SearchDeps = { fetch: (...a) => globalThis.fetch(...a), url: searchUrl }

/** ONE SEARCH. Throws with a sentence a model can act on — the caller is a tool
 *  handler and its error text goes straight into an agent's transcript, so
 *  "fetch failed" is a dead end and "the search service did not answer" is not. */
export async function searchWeb(query: string, opts: { limit?: number; deps?: Partial<SearchDeps> } = {}): Promise<SearchResult[]> {
  const deps = { ...REAL, ...opts.deps }
  const q = query.trim()
  if (!q) throw new Error('a search needs a query')
  const limit = Math.max(1, Math.min(opts.limit ?? DEFAULT_LIMIT, MAX_LIMIT))
  const base = await deps.url()

  const params = new URLSearchParams({ q, format: 'json' })
  let res: Response
  try {
    res = await deps.fetch(`${base}/search?${params.toString()}`, {
      headers: { accept: 'application/json' },
      signal: AbortSignal.timeout(TIMEOUT_MS),
    })
  } catch (err) {
    throw new Error(
      `the search service at ${base} did not answer (${err instanceof Error ? err.message : String(err)}). Tell whoever asked that live search is unavailable right now rather than answering from memory.`,
    )
  }
  if (res.status === 403) {
    // THE ONE MISCONFIGURATION EVERYONE HITS, named exactly, because the generic
    // "403" sends an operator looking for an auth problem that does not exist.
    throw new Error(
      `the search service refused the JSON format (403). SearXNG ships with JSON off: add \`json\` to \`search.formats\` in its settings.yml and restart it.`,
    )
  }
  if (!res.ok) throw new Error(`the search service answered ${res.status}`)

  const body = (await res.json().catch(() => null)) as { results?: unknown[] } | null
  return resultsFrom(body).slice(0, limit)
}

/** SearXNG's payload, narrowed. Defensive because the shape is an aggregate of
 *  a dozen engines and a single one returning something odd must cost that
 *  result, not the search. */
export function resultsFrom(body: unknown): SearchResult[] {
  const rows = (body as { results?: unknown[] } | null)?.results
  if (!Array.isArray(rows)) return []
  const out: SearchResult[] = []
  const seen = new Set<string>()
  for (const row of rows) {
    if (!row || typeof row !== 'object') continue
    const r = row as Record<string, unknown>
    const url = typeof r.url === 'string' ? r.url : ''
    // A RESULT WITHOUT A URL CANNOT BE CITED, and an uncitable result in a
    // research pipeline is worse than one fewer result — `ungrounded_ref` grounds
    // every claim against these.
    if (!url || seen.has(url)) continue
    seen.add(url)
    out.push({
      title: typeof r.title === 'string' ? r.title : url,
      url,
      snippet: typeof r.content === 'string' ? r.content : '',
      engine: typeof r.engine === 'string' ? r.engine : 'unknown',
    })
  }
  return out
}

/** Is a search engine reachable at all? Used by the setup check and by the
 *  admin surface, so "search is unavailable" is something an operator is told
 *  once rather than discovering through a model's excuse.
 *
 *  "REACHABLE" MEANS "FINDS THINGS", NOT "ANSWERS THE PHONE", and the difference
 *  is the whole reason this function needed changing. A SearXNG whose every
 *  general engine is CAPTCHA-walled — the DEFAULT state of a fresh self-hosted
 *  instance, see the engines block in `docker/searxng/settings.template.yml` —
 *  returns HTTP 200 with valid JSON and `results: []`. The old check asked only
 *  whether `searchWeb` threw, so it reported healthy while search found nothing
 *  for anybody, and `platformSupply` went on advertising a `web_search` tool
 *  that could not search. A model handed an empty result set does not fail
 *  loudly; it answers from memory in the same confident voice.
 *
 *  So a canary query that comes back EMPTY is a failure, and the sentence names
 *  the actual cause rather than the symptom. A false negative here costs a
 *  capability tag; a false positive costs an uncited research report. */
export async function searchReachable(deps?: Partial<SearchDeps>): Promise<{ ok: boolean; url: string; error: string | null }> {
  const url = await (deps?.url ?? REAL.url)()
  try {
    const hits = await searchWeb(CANARY, { limit: 1, ...(deps ? { deps } : {}) })
    if (hits.length === 0) {
      return {
        ok: false,
        url,
        error: `the search service at ${url} answered but found nothing, which usually means every engine is refusing it. Ask it which: \`curl -s '${url}/search?q=test&format=json' | jq .unresponsive_engines\` — a fresh SearXNG defaults to engines that CAPTCHA a self-hosted instance.`,
      }
    }
    return { ok: true, url, error: null }
  } catch (err) {
    return { ok: false, url, error: err instanceof Error ? err.message : String(err) }
  }
}

/** THE CANARY QUERY, and it is deliberately a common word rather than "talaria".
 *  A health check has to fail only when SEARCH is broken — a rare term that
 *  genuinely has no hits on a small indie index would make a working instance
 *  report itself down. */
const CANARY = 'wikipedia'
