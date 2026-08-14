// CITATION MARKERS PAST [99] — three silent failures, one regex.
//
// `\d{1,2}` was correct for exactly as long as research meant Perplexity: sonar
// answers with a handful of pre-ranked sources, so a run never approached the
// two-digit line. Research is model-agnostic now, and the tool path is the
// common one — an expedition is up to twelve queries against a web-search tool,
// each returning a page of results, with every distinct URL numbered. Three
// figures is ordinary there.
//
// None of the three failures below announced itself. That is why they are worth
// a file: each one produced a plausible report.
import { describe, expect, it, vi } from 'vitest'

vi.mock('@/server/db/pg', () => ({ db: async () => (() => Promise.resolve([])) as never }))

const { SourceRegistry, stripUnknownMarkers } = await import('@/server/research')

const src = (n: number) => ({ url: `https://example.com/s${n}`, title: `S${n}`, snippet: null })

describe('renumbering a hit onto global numbering', () => {
  it('maps a local marker onto a three-digit global index', () => {
    // NOT one of the failures — stated because it looks like it should be. The
    // markers `renumber` READS are local to one search hit ([1], [2], [3]) and
    // it is the OUTPUT that carries the global number, so two-digit matching
    // was always sufficient here. A mutation to `\d{1,2}` leaves this test
    // green, which is the correct result and the reason the comment on
    // `MARKER_RE` says so out loud.
    const reg = new SourceRegistry()
    for (let i = 1; i <= 103; i++) reg.add(src(i))
    expect(reg.renumber({ content: 'A stalled subscriber pins WAL [1].', sources: [src(104)] })).toBe('A stalled subscriber pins WAL [104].')
  })

  it('keeps two-digit renumbering working, which is every sonar run', () => {
    const reg = new SourceRegistry()
    reg.add(src(1))
    expect(reg.renumber({ content: 'Claim [1] and claim [2].', sources: [src(2), src(3)] })).toBe('Claim [2] and claim [3].')
  })

  it('leaves a marker it has no mapping for alone rather than guessing', () => {
    const reg = new SourceRegistry()
    expect(reg.renumber({ content: 'Claim [9].', sources: [src(1)] })).toBe('Claim [9].')
  })

  it('numbers a repeated URL once, so the registry does not drift past the report', () => {
    const reg = new SourceRegistry()
    expect(reg.add(src(1))).toBe(1)
    expect(reg.add(src(1))).toBe(1)
    expect(reg.add(src(2))).toBe(2)
  })

  it('does not treat a four-digit number as a marker', () => {
    // `[2024]` in prose is a year. Renumbering it would corrupt the sentence.
    const reg = new SourceRegistry()
    expect(reg.renumber({ content: 'Stable since [2024], per [1].', sources: [src(1)] })).toBe('Stable since [2024], per [1].')
  })
})

describe('stripping citations the registry does not have', () => {
  // THE PATH THAT ACTUALLY BROKE, and it is the last thing standing between an
  // invented citation and a human reading the report.
  const known = Array.from({ length: 120 }, (_, i) => i + 1)

  it('removes an invented THREE-digit marker, which used to survive into the report', () => {
    // The registry carries 120. [150] is invention — and two-digit matching
    // neither counted it nor stripped it, so it reached the saved document
    // looking exactly like a real citation.
    const out = stripUnknownMarkers('Throughput collapses above 40k writes [150].', known)
    expect(out.cleaned).toBe('Throughput collapses above 40k writes .')
    expect(out.dropped).toBe(1)
  })

  it('counts three-digit citations that DO resolve, so a thorough report is not scored as a thin one', () => {
    const out = stripUnknownMarkers('WAL is pinned [104] and slots do not follow failover [118].', known)
    expect(out.dropped).toBe(0)
    expect([...out.cited].sort((a, b) => a - b)).toEqual([104, 118])
  })

  it('still strips a two-digit invention, which is every sonar run', () => {
    const out = stripUnknownMarkers('A claim [7] and an invention [99].', [1, 7])
    expect(out.cleaned).toBe('A claim [7] and an invention .')
    expect(out.dropped).toBe(1)
  })

  it('leaves a four-digit number alone, because that is a year', () => {
    // Stripping it would delete dates out of reports.
    const out = stripUnknownMarkers('Stable since [2024], per [7].', [7])
    expect(out.cleaned).toBe('Stable since [2024], per [7].')
    expect(out.dropped).toBe(0)
  })

  it('unwraps a fenced document, which some models still emit', () => {
    expect(stripUnknownMarkers('```markdown\n# Title\n\nA claim [7].\n```', [7]).cleaned).toBe('# Title\n\nA claim [7].')
  })
})
