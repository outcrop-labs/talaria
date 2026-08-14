// ARMING A PROVIDER'S OWN WEB SEARCH, and reading the citations back.
//
// WHY THIS FILE EXISTS. Talaria's research pipeline can search two ways: through
// a tool it drives itself (SearXNG or a registered MCP server), or by letting
// the model search as part of answering. The second one was never actually
// switched on. `buildUpstream` forwards a plain chat body, so "native" meant
// posting an ordinary completion to a model that COULD have searched and
// harvesting whatever it volunteered — which in practice is Perplexity's sonar
// family, because sonar always searches, and nothing else.
//
// That is the whole reason research used to require Perplexity.
//
// WHAT IS ACTUALLY REACHABLE from a gateway that speaks OpenAI-shaped bodies,
// established from each provider's current documentation rather than from
// memory. It is less than one would hope, and saying so here is the point:
//
//   PERPLEXITY    already on. Sonar searches unconditionally; there is no
//                 parameter to send and never was. Citations come back OUT OF
//                 BAND at the response root.
//   OPENROUTER    yes, and this is the one worth having: `plugins: [{id:'web'}]`
//                 searches once per request for ANY model, and the catalog says
//                 which models have provider-native search of their own. This is
//                 how most self-hosted installs reach a frontier model.
//   OPENAI        only by picking a `-search-api` MODEL. `web_search_options` is
//                 a TUNER, not a switch, and it 400s on a model that does not
//                 have search — so sending it hopefully would break every
//                 ordinary call. The real controls live on the Responses API,
//                 which is a different endpoint and a different body shape.
//   ANTHROPIC     no. Its web search is a server tool on the native `/v1/messages`
//                 body, and the OpenAI compatibility layer does not expose it.
//                 A gateway forwarding OpenAI bodies cannot arm it at all.
//
// SO THE TOOL PATH IS THE UNIVERSAL ONE, and native is the optimisation. That is
// the opposite of how this pipeline was built, and it is why `planSearch` now
// falls back to the tool path for any model rather than refusing.
//
// NOTHING HERE IS A MODEL LIST. Provider is read off the endpoint and capability
// off the catalog, so a model that gains search next month is picked up the day
// it is registered.

/** What to merge into an OpenAI-shaped request body to arm the provider's own
 *  search. Empty when there is nothing to send — which includes both "this
 *  provider cannot be armed" and "it is already on". */
export type NativeSearchBody = Record<string, unknown>

/** `provider` as it is spelled on `LlmEndpoint`. Compared case-insensitively and
 *  by prefix, because an operator naming an endpoint "openrouter-prod" means
 *  OpenRouter and should not have to know that we string-match. */
const isProvider = (provider: string | null | undefined, want: string): boolean => {
  // TOLERATES A MISSING PROVIDER, which is not defensive padding: an endpoint row
  // can carry an empty provider, and the honest answer for "we do not know what
  // this is" is to send nothing rather than to throw inside a search stage.
  const p = (provider ?? '').trim().toLowerCase()
  if (p === '') return false
  return p === want || p.startsWith(`${want}-`) || p.startsWith(`${want}_`) || p.includes(want)
}

/** ARM IT, if this provider can be armed over an OpenAI-shaped body.
 *
 *  TAKES THE PROVIDER AND NOTHING ELSE, which is a smaller signature than the
 *  first draft had. It wanted the model's own catalog capability too — until it
 *  turned out that neither branch which could use it wants it: OpenRouter's web
 *  plugin works on ANY model whether or not that model has search of its own,
 *  and OpenAI has no safe parameter at any capability level. An argument no
 *  branch reads is an argument the next reader has to disprove.
 *
 *  DELIBERATELY CONSERVATIVE. Every branch that is not certain sends nothing:
 *  an unrecognised provider, an OpenAI model without search, anything Anthropic.
 *  Sending a parameter a provider rejects turns a working research run into a
 *  400, and the failure mode of sending nothing is merely the tool path — which
 *  is where most runs go anyway. */
export function nativeSearchBody(provider: string | null | undefined): NativeSearchBody {
  // OpenRouter: one search per request, any model, normalised citations back.
  // The `web` plugin is documented as deprecated in favour of the server tool,
  // and is kept because it is the one that does not depend on the MODEL choosing
  // to call a tool — a search stage that may or may not have searched is not a
  // search stage. Revisit when the server tool can be forced.
  if (isProvider(provider, 'openrouter')) return { plugins: [{ id: 'web' }] }

  // Perplexity searches unconditionally. Nothing to send, and sending
  // `web_search_options` would be a guess about a tuner we have no reason to
  // touch.
  if (isProvider(provider, 'perplexity')) return {}

  // OpenAI: the switch is the MODEL, not a parameter. `web_search_options` 400s
  // on a model without search — "Web search options not supported with this
  // model" — so it is only ever safe on a model the catalog already says
  // searches, and even then it only tunes what is already on. Nothing to gain,
  // a hard failure to lose.
  if (isProvider(provider, 'openai')) return {}

  // Anthropic and everything unrecognised: no OpenAI-shaped activation exists.
  return {}
}

/** Whether this provider's native search can be armed at all from here. Used by
 *  the admin-facing explanation, so an operator is told "this model can search
 *  but we cannot switch it on over this endpoint" rather than being left to
 *  wonder why every run takes the tool path. */
export function canArmNative(provider: string | null | undefined): boolean {
  return isProvider(provider, 'openrouter') || isProvider(provider, 'perplexity')
}

export interface HarvestedSource {
  url: string
  title: string | null
  snippet: string | null
}

/** EVERY SHAPE A CITATION COMES BACK IN, normalised.
 *
 *  THE BUG THIS FIXES. The search stage only ever read Perplexity's two
 *  out-of-band fields, so a model that searched and cited through the OPENAI
 *  ANNOTATION SHAPE — which is what OpenAI returns and what OpenRouter
 *  normalises every one of its engines to — recorded ZERO sources. The run then
 *  had findings with nothing to cite, and the fixtures scored the model for
 *  writing an uncited brief. The model had done its job; we dropped the evidence.
 *
 *  Three shapes, all real, in the order they are most likely to be authoritative:
 *    `search_results`  Perplexity, richest (title + snippet + url)
 *    `annotations`     OpenAI / OpenRouter, per-message, `url_citation`
 *    `citations`       Perplexity's older flat list of URLs
 *
 *  MERGED, NOT FIRST-MATCH-WINS, because a response may legitimately carry more
 *  than one — and deduped on URL, because the same source appearing in two
 *  shapes is one source, not two. First spelling of a URL keeps its title. */
export function harvestSources(body: unknown): HarvestedSource[] {
  // A NON-OBJECT BODY IS A REAL CASE, not defensive padding: this parses a live
  // provider response, and a gateway erroring can return `null`, a string, or an
  // HTML page. Throwing here would turn "the provider had a bad minute" into a
  // failed research run with a stack trace instead of a retry.
  if (typeof body !== 'object' || body === null) return []
  const j = body as {
    search_results?: Array<{ url?: string; title?: string; snippet?: string }>
    citations?: Array<string | { url?: string; title?: string }>
    choices?: Array<{
      message?: {
        annotations?: Array<{ type?: string; url_citation?: { url?: string; title?: string; content?: string } }>
      }
    }>
  }
  const out: HarvestedSource[] = []
  const seen = new Set<string>()
  const add = (url: unknown, title?: unknown, snippet?: unknown): void => {
    if (typeof url !== 'string' || url.trim() === '') return
    const key = url.trim()
    if (seen.has(key)) return
    seen.add(key)
    out.push({ url: key, title: typeof title === 'string' && title !== '' ? title : null, snippet: typeof snippet === 'string' && snippet !== '' ? snippet : null })
  }

  for (const s of j.search_results ?? []) add(s?.url, s?.title, s?.snippet)
  for (const c of j.choices ?? []) {
    for (const a of c?.message?.annotations ?? []) {
      // `type` is the discriminator and there are others (`file_citation`), so a
      // reader that took every annotation would file a local file path as a web
      // source.
      if (a?.type && a.type !== 'url_citation') continue
      add(a?.url_citation?.url, a?.url_citation?.title, a?.url_citation?.content)
    }
  }
  // Old Perplexity: a flat list of URL strings. Tolerates the object form too,
  // because more than one gateway has been seen to normalise it that way.
  for (const c of j.citations ?? []) (typeof c === 'string' ? add(c) : add(c?.url, c?.title))

  return out
}
