/** THE CITATION REGISTRY — one implementation, two hosts.
 *
 *  `server/research.ts` (the in-process pipeline) and
 *  `server/runs/defs/research.ts` (the durable run) both renumber search hits
 *  onto one global source list, and each carried a private copy of this class
 *  until the copies diverged where it mattered: the run's `add` allocated
 *  `size + 1` where the pipeline's allocates HIGHEST + 1, so a run seeded from
 *  a parent whose source list carries a gap — size 3, highest [5] — handed [4]
 *  to a brand-new URL and silently re-aimed a citation a human already read.
 *
 *  A LEAF because the two hosts cannot import each other: the run definition is
 *  loaded at boot for the reclaim sweep, and `server/research.ts` imports IT —
 *  an import back would close a cycle. The two hosts' source shapes were
 *  identical (`idx, url, title, snippet`), so the run's `RegistrySource` is not
 *  mapped to the pipeline's `ResearchSource` — it IS one, and the duplicate
 *  name is gone. */

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
export const MARKER_RE = /\[(\d{1,3})\]/g

export interface ResearchSource {
  idx: number
  url: string
  title: string | null
  snippet: string | null
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
   *  continue from the highest, whatever gaps that leaves.
   *
   *  Ascending `idx`, because insertion order is `list()` order: a registry
   *  rebuilt from an unsorted checkpoint renders its sources out of order even
   *  though every number still points where it always did. */
  static from(sources: readonly ResearchSource[]): SourceRegistry {
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
    // HIGHEST + 1, not size + 1. A seeded registry can carry gaps — a parent
    // whose source [4] was deleted leaves size 3 and a highest of 5 — and
    // `size + 1` would hand [4] to a brand new URL, quietly re-aiming every
    // citation the parent's text makes to [4].
    const idx = Math.max(0, ...[...this.byUrl.values()].map((v) => v.idx)) + 1
    this.byUrl.set(s.url, { idx, title: s.title, snippet: s.snippet })
    return idx
  }

  /** Rewrite one search hit's LOCAL [n] markers onto global numbering.
   *
   *  The hit SHAPE is declared here rather than imported from either host: this
   *  module is their shared leaf, so importing back either way would close a
   *  cycle for the sake of two fields. */
  renumber(hit: { content: string; sources: Array<{ url: string; title: string | null; snippet: string | null }> }): string {
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
