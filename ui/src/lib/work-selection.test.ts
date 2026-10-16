// The merged work harness, pinned from both ends: THREE ADDRESSES STILL
// RESOLVE and ONE RAIL ENTRY POINTS AT THEM. Those two facts are the whole of
// the merge that turned Work, Plan and Research into one surface, and nothing
// else in the suite touches either — the harness itself is a component with a
// dozen queries behind it, so the rule it routes by lives in `work-selection`
// (pure) and the rule it is reached by lives in `nav` (plain data).
//
// The real NAV is imported rather than a fixture of it, which is the opposite
// call from nav-sections.test.ts and deliberate: that file tests a DERIVATION
// and wants cores it can shape, while these assertions are about the menu this
// app actually ships — "there is exactly one Work entry" says nothing if the
// entry is one the test wrote. It imports cleanly here because vitest.config.ts
// carries the Svelte plugin, which transforms the Lucide icon modules (raw
// `.svelte` files) that used to make this import fail.
import { describe, expect, it } from 'vitest'
import { GATEABLE_VIEWS, NAV, type NavItem } from './nav'
import { legacyRunIdFor, sessionIdFor } from './work-selection'
import type { Unreads } from './unreads.svelte'

const NAV_ITEMS: NavItem[] = NAV.flatMap((section) => section.items)

// ── The URL is the selection ─────────────────────────────────────────────────

describe('sessionIdFor', () => {
  it('reads the session out of /work/<id>', () => {
    expect(sessionIdFor('/work/c-1')).toBe('c-1')
  })

  it('reads the session out of /plan/<id> too — the old address names the same row', () => {
    // A plan IS a session now. New selections move the URL to `/work`, but a
    // `/plan/<id>` somebody was mailed has to keep opening the thing it named.
    expect(sessionIdFor('/plan/c-1')).toBe('c-1')
  })

  it('selects NOTHING on a research address — a run id is not a conversation id', () => {
    // THE LEAK THIS GUARDS. `/research/<runId>` names a run, and the harness
    // trades it for the run's own discussion before anything
    // conversation-shaped may be asked. Reading it as the session selection
    // would hand the conversation plane an id it has never seen — a 404 the
    // view renders as "this session is gone", for the beat before the redirect
    // lands, on the path a mailed link most often arrives by.
    expect(sessionIdFor('/research/run-42')).toBeNull()
  })

  it('selects nothing on the bare list, with or without the trailing slash', () => {
    // Open with nothing selected is a real state — it is the session list.
    expect(sessionIdFor('/work')).toBeNull()
    expect(sessionIdFor('/work/')).toBeNull()
    expect(sessionIdFor('/plan')).toBeNull()
    expect(sessionIdFor('/plan/')).toBeNull()
  })

  it('does not read a deeper path as a selection', () => {
    // `/work/<id>/anything` means something this surface does not define, and
    // opening the session for it would be a quiet wrong answer.
    expect(sessionIdFor('/work/c-1/extra')).toBeNull()
    expect(sessionIdFor('/plan/c-1/extra')).toBeNull()
  })

  it('does not match a view that merely shares a prefix', () => {
    expect(sessionIdFor('/workspace/c-1')).toBeNull()
    expect(sessionIdFor('/planner/p-1')).toBeNull()
    expect(sessionIdFor('/boards/b-1/t-2')).toBeNull()
    expect(sessionIdFor('/')).toBeNull()
  })

  it('decodes an encoded id and takes ids as opaque', () => {
    // Existence is the loading query's question, which already has a
    // not-found story; validating shape here would turn one honest empty
    // state into two.
    expect(sessionIdFor('/work/a%2Fb')).toBe('a/b')
    expect(sessionIdFor('/work/not-a-uuid')).toBe('not-a-uuid')
  })
})

describe('legacyRunIdFor', () => {
  it('reads the run out of /research/<runId>', () => {
    expect(legacyRunIdFor('/research/run-42')).toBe('run-42')
  })

  it('reads no run off a session address — the two ids never cross', () => {
    // The mirror of the leak above: a conversation id arriving as a run id
    // would fetch a run detail that does not exist and offer its report tab.
    expect(legacyRunIdFor('/work/c-1')).toBeNull()
    expect(legacyRunIdFor('/plan/c-1')).toBeNull()
  })

  it('selects nothing on the bare research address', () => {
    // The harness with nothing selected, same as a bare `/work`.
    expect(legacyRunIdFor('/research')).toBeNull()
    expect(legacyRunIdFor('/research/')).toBeNull()
  })

  it('does not read a deeper path, or a prefix look-alike, as a run', () => {
    expect(legacyRunIdFor('/research/run-42/sources')).toBeNull()
    expect(legacyRunIdFor('/researchers/run-42')).toBeNull()
  })

  it('decodes an encoded run id', () => {
    expect(legacyRunIdFor('/research/a%2Fb')).toBe('a/b')
  })
})

// ── One entry for every session ──────────────────────────────────────────────

describe('NAV', () => {
  it('holds exactly one Work entry, and no Plan or Research entry', () => {
    // THE MERGE, AS THE PERSON SEES IT. Three rail items pointing at three
    // views of one shape made somebody decide which filing cabinet a thought
    // belonged in before they could have it. A second entry reappearing here
    // — a `/plan` item restored "for discoverability", say — puts that choice
    // back in front of them, so this is an exact count rather than a
    // presence check.
    const work = NAV_ITEMS.filter((item) => item.to === '/work')
    expect(work).toHaveLength(1)
    expect(work[0]).toMatchObject({ to: '/work', label: 'Work' })
    // Nothing under those old heads either: an item at `/plan/<something>`
    // would be the same second door with a longer address.
    expect(NAV_ITEMS.filter((item) => item.to === '/plan' || item.to.startsWith('/plan/'))).toEqual([])
    expect(NAV_ITEMS.filter((item) => item.to === '/research' || item.to.startsWith('/research/'))).toEqual([])
  })

  it('carries the sessions badge on that entry, and on nothing else', () => {
    // A merged entry that counted only work sessions would read "nothing
    // waiting" over an unread plan turn in the surface it opens, so the badge
    // is the SUM (`sessions`, see unreads-sum.ts). Exactly one item may carry
    // it: two would show the same waiting work twice in one rail.
    const work = NAV_ITEMS.find((item) => item.to === '/work')
    expect(work?.badge).toBe('sessions')
    expect(NAV_ITEMS.filter((item) => item.badge === 'sessions').map((item) => item.to)).toEqual(['/work'])
    // And the name must be a FIELD OF THE UNREADS ANSWER, because nav-badges
    // reads `unreads.data?.[item.badge] ?? 0` — a badge naming nothing that
    // arrives reads 0 forever, which is the silent-zero bug the `!` doctrine
    // exists to prevent. The annotation is the assertion; it is a type error
    // the day `sessions` stops being a field.
    const badgeField: keyof Unreads = 'sessions'
    expect(work?.badge).toBe(badgeField)
  })

  it('keeps no per-kind badge in the rail now that the entries are gone', () => {
    // The `plan`/`research`/`work` badge names still exist on NavItem (the
    // arms are still answered per kind), but nothing in the rail may ride one
    // alone — that is what made three entries each tell a third of the truth.
    expect(NAV_ITEMS.filter((item) => item.badge === 'plan' || item.badge === 'research' || item.badge === 'work')).toEqual([])
  })
})

describe('GATEABLE_VIEWS', () => {
  const paths = GATEABLE_VIEWS.map((view) => view.to)

  it('offers the merged Work view', () => {
    expect(paths).toContain('/work')
  })

  it('still offers /plan and /research as legacy denial keys', () => {
    // A denial is stored per user AGAINST THE PATH it was granted against, and
    // both old paths still resolve (the harness reads the selection out of
    // either). Dropping these rows would leave an existing denial pointing at
    // nothing and silently re-open the surface to someone it was taken from —
    // a permission that quietly widens, which is the worst direction for one
    // to drift.
    expect(paths).toContain('/plan')
    expect(paths).toContain('/research')
  })

  it('offers them WITHOUT a rail entry behind them, which is what makes them legacy', () => {
    // The pairing that says this is deliberate rather than leftover: a
    // gateable path with no nav item is a door nobody is shown and a denial
    // can still name.
    for (const legacy of ['/plan', '/research']) {
      expect(paths).toContain(legacy)
      expect(NAV_ITEMS.some((item) => item.to === legacy)).toBe(false)
    }
  })
})
