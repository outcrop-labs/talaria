// ARMING A PROVIDER'S SEARCH, AND READING THE CITATIONS BACK.
//
// Two things this file holds, and the second is a bug that was live.
//
// The pipeline never sent ANY activation, so its "native" path was a plain
// completion posted to a model that could have searched. That is true only of
// Perplexity, which searches unconditionally — and is exactly why research used
// to require Perplexity.
//
// And it read only Perplexity's two out-of-band citation fields, so a model
// citing through the OPENAI ANNOTATION shape contributed ZERO sources. The run
// then wrote findings it could not cite and the fixtures scored the model for an
// uncited brief. The model had done the work; we dropped the evidence.
import { describe, expect, it } from 'vitest'
import { canArmNative, harvestSources, nativeSearchBody } from './native-search'

describe('arming the provider’s own search', () => {
  it('sends OpenRouter the web plugin — the one that is actually reachable', () => {
    // Works for ANY model there, whether or not that model has search of its
    // own, which is what makes it the useful branch on a self-hosted install
    // reaching a frontier model through OpenRouter.
    expect(nativeSearchBody('openrouter')).toEqual({ plugins: [{ id: 'web' }] })
    // Operators name endpoints things like "openrouter-prod". Matching by
    // prefix and substring means they do not have to know we string-match.
    expect(nativeSearchBody('OpenRouter-prod')).toEqual({ plugins: [{ id: 'web' }] })
  })

  it('sends Perplexity nothing, because sonar already searches', () => {
    expect(nativeSearchBody('perplexity')).toEqual({})
  })

  it('sends OpenAI nothing — `web_search_options` is a tuner that 400s elsewhere', () => {
    // The switch on OpenAI's chat surface is the MODEL (`-search-api`), not a
    // parameter, and sending the tuner to a model without search is a hard 400:
    // "Web search options not supported with this model." Nothing to gain, a
    // broken research run to lose.
    expect(nativeSearchBody('openai')).toEqual({})
  })

  it('sends Anthropic nothing, because there is no OpenAI-shaped way to arm it', () => {
    // Its web search is a server tool on the native /v1/messages body, and the
    // OpenAI compatibility layer does not expose it.
    expect(nativeSearchBody('anthropic')).toEqual({})
  })

  it('sends nothing for an unknown or missing provider rather than guessing', () => {
    // An endpoint row can carry an empty provider. Throwing inside a search
    // stage over that would turn a config gap into a failed run.
    for (const p of ['', '   ', null, undefined, 'some-local-thing']) {
      expect(nativeSearchBody(p)).toEqual({})
    }
  })

  it('says which providers can be armed at all, for the admin explanation', () => {
    expect(canArmNative('openrouter')).toBe(true)
    expect(canArmNative('perplexity')).toBe(true)
    // Not a failing of the model — a limit of the surface we talk to it over,
    // and an operator is better told that than left wondering.
    expect(canArmNative('anthropic')).toBe(false)
    expect(canArmNative(null)).toBe(false)
  })
})

describe('harvesting citations, in every shape they come back in', () => {
  it('reads Perplexity’s rich search_results', () => {
    expect(harvestSources({ search_results: [{ url: 'https://a.test', title: 'A', snippet: 's' }] })).toEqual([
      { url: 'https://a.test', title: 'A', snippet: 's' },
    ])
  })

  it('reads the OpenAI/OpenRouter annotation shape — the one that was dropped', () => {
    // THE BUG. This is what OpenAI returns and what OpenRouter normalises every
    // one of its engines to, and nothing read it.
    const body = {
      choices: [
        {
          message: {
            annotations: [
              { type: 'url_citation', url_citation: { url: 'https://b.test', title: 'B', content: 'snip' } },
              { type: 'url_citation', url_citation: { url: 'https://c.test' } },
            ],
          },
        },
      ],
    }
    expect(harvestSources(body)).toEqual([
      { url: 'https://b.test', title: 'B', snippet: 'snip' },
      { url: 'https://c.test', title: null, snippet: null },
    ])
  })

  it('ignores annotations that are not web citations', () => {
    // `file_citation` is a real annotation type. Filing a local file path as a
    // web source would put it in the report's bibliography.
    const body = { choices: [{ message: { annotations: [{ type: 'file_citation', url_citation: { url: 'file:///etc/passwd' } }] } }] }
    expect(harvestSources(body)).toEqual([])
  })

  it('reads Perplexity’s older flat citations list, and the object form some gateways emit', () => {
    expect(harvestSources({ citations: ['https://d.test'] })).toEqual([{ url: 'https://d.test', title: null, snippet: null }])
    expect(harvestSources({ citations: [{ url: 'https://e.test', title: 'E' }] })).toEqual([{ url: 'https://e.test', title: 'E', snippet: null }])
  })

  it('MERGES the shapes rather than taking the first that matched', () => {
    // The old code was `a ?? b ?? []`, so a response carrying both kept only
    // one. A response may legitimately carry more than one.
    const body = {
      search_results: [{ url: 'https://a.test', title: 'A' }],
      choices: [{ message: { annotations: [{ type: 'url_citation', url_citation: { url: 'https://b.test' } }] } }],
      citations: ['https://c.test'],
    }
    expect(harvestSources(body).map((s) => s.url)).toEqual(['https://a.test', 'https://b.test', 'https://c.test'])
  })

  it('dedupes on url, keeping the richest first spelling', () => {
    // The same source in two shapes is one source. Keeping both would inflate
    // the citation count the report is graded on.
    const body = {
      search_results: [{ url: 'https://a.test', title: 'A', snippet: 's' }],
      citations: ['https://a.test'],
    }
    expect(harvestSources(body)).toEqual([{ url: 'https://a.test', title: 'A', snippet: 's' }])
  })

  it('survives a body with nothing in it, or the wrong shape entirely', () => {
    for (const body of [{}, null, undefined, 'not json', { choices: [{}] }, { search_results: [{}] }, { citations: [null] }]) {
      expect(harvestSources(body)).toEqual([])
    }
  })
})
