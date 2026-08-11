import { describe, expect, it } from 'vitest'
import { isGap, NO_TOOLS } from '../define'
import { distillerHarness } from './distiller'

// THE DISTILLER'S OWN ASSERTIONS, held to the standard they hold models to.
//
// Two classes of defect are locked here, and both shipped:
//
//   AN ASSERTION THAT FAILS THE BEST AVAILABLE ANSWER. The reversal check used
//   a phrase regex over the whole string, so "originally to be put in the
//   gateway; reversed — it goes in the API layer" was scored as having recorded
//   the reversed decision. The model had recorded the reversal exactly right.
//
//   AN ASSERTION THAT CANNOT BE ANSWERED. "Shorter than the conversation it
//   distills" was measured against a 151-character toy transcript, while the
//   WIDENED prompt asks the same model for markdown headings. We were failing
//   models for obeying the other instruction we gave them.
//
// BY NAME, NEVER BY INDEX: `evals[3]` silently re-points at a different fixture
// the moment somebody inserts one.
const named = (name: string) => {
  const c = distillerHarness.evals?.find((e) => e.name === name)
  if (!c) throw new Error(`no distiller fixture called "${name}"`)
  return (value: string) => c.check(value, { calls: [], calledBefore: () => false, world: null, exhausted: false })
}

const str = (v: ReturnType<ReturnType<typeof named>>): string | null => (typeof v === 'string' ? v : null)

describe('the reversal fixture rewards a recorded reversal instead of failing it', () => {
  const check = named('keeps only the position the conversation ended on')

  it('accepts a distillation that names the gateway AS the reversed option', () => {
    for (const answer of [
      '- Rate limiter: originally to be put in the gateway; reversed — it goes in the API layer where the tenant is known.\n- Ivan owns it.',
      '- Rate limiting placed in the API layer, not the gateway (the gateway cannot see per-tenant quota).\n- Ivan owns the work.',
      '## Decisions\n- Rate limiter goes in the API layer. The gateway was considered first and dropped.\n- Ivan owns the rate limiting work.',
      // OBSERVED IN A REAL SWEEP, and failed by the second version of this
      // check: a faithful record of the argument AGAINST the gateway, which is
      // the reasoning the fixture wants kept. It contains both "place" and
      // "goes", so a verb list alone does not rescue it either.
      [
        '## Decisions',
        '- Rate limiting goes in the API layer, where the tenant is known.',
        '- gateway rate limiting would require a second cache and a second place that goes stale. (nomad; user agreed)',
        '- Ivan owns it.',
      ].join('\n'),
    ]) {
      expect(check(answer), answer).toBeNull()
    }
  })

  it('still fails a distillation that offers the gateway as the standing placement', () => {
    // The flattened transcript: both placements recorded, the reversal lost —
    // which leaves the owner's brain holding a contradiction.
    // It DOES name the API layer, somewhere — a flattened transcript records
    // both placements. What it never says is which one survived.
    const flattened = '- Rate limiter goes in the gateway, covering every caller at once.\n- Per-tenant quota lives in the API layer.\n- Ivan owns it.'
    expect(str(check(flattened))).toContain('as if it still stood')
  })
})

describe('a pleasantry NAMED as omitted is not a pleasantry kept', () => {
  const check = named('keeps the planted decisions and drops the planted pleasantries')

  it('accepts a distillation that says it dropped the small talk', () => {
    expect(
      check(
        '- Ledger store: Postgres over SQLite (locked).\n- Ledger migration ships Friday.\n- Nadia owns the rollback plan.\n- Weekend pleasantries omitted.',
      ),
    ).toBeNull()
  })

  it('still fails one that carries the pleasantry as substance', () => {
    expect(
      str(check('- Ledger store: Postgres over SQLite.\n- Ledger migration ships Friday.\n- Nadia owns the rollback plan.\n- User had a good weekend.')),
    ).toContain('pleasantries')
  })
})

describe('compression is asked as a ratio, and abstained from when it cannot be asked', () => {
  const check = named('is shorter than the conversation it distills')

  it('accepts a correct WIDENED distillation, headings and all', () => {
    // The exact answer the widened prompt asks for. Under the old raw-length
    // comparison against a four-line transcript, this class of answer failed.
    const widened = [
      '## Decisions',
      '- Ledger store: Postgres over SQLite. Locked.',
      '- Ledger migration ships Friday, before the release cut.',
      '- Connection pooler deferred to its own ticket, no date.',
      '- Ledger UI out of scope this quarter.',
      '## Outcomes',
      '- Nadia owns the rollback plan.',
    ].join('\n')
    expect(check(widened)).toBeNull()
  })

  it('fails a distillation that is most of its source', () => {
    const restated = 'Postgres Friday Nadia rollback ledger migration. '.repeat(40)
    expect(str(check(restated))).toContain('restated the conversation')
  })

  it('the transcript is long enough for the question to be fair', () => {
    // THE GUARD ON THE GUARD. `restated` reports a GAP — our defect, not the
    // model's — for a transcript too short to compress. That branch must not be
    // reachable from the shipped fixture, or the harness would be quietly
    // abstaining on every run instead of measuring anything.
    const widened = '## Decisions\n- Postgres over SQLite, locked. Ledger migration ships Friday. Nadia owns the rollback.'
    expect(isGap(check(widened))).toBe(false)
  })
})

describe('the seat-cap fixture wants the number that was decided, not the one argued for', () => {
  const check = named('carries a single stated decision')

  it('accepts the decided number', () => {
    expect(check('- Free tier seat cap set to three (down from a proposed five); locked, not to be reopened.')).toBeNull()
  })

  it('fails a distillation that records the cap without the number', () => {
    expect(str(check('- The free tier seat cap was discussed and settled at the lower option.'))).toContain('three')
  })
})

// ── The "nothing durable" fixture, calibrated against what models really sent ─

describe('says a conversation held nothing durable', () => {
  const fixture = (distillerHarness.evals ?? []).find((e) => e.name.startsWith('says a conversation held nothing durable'))!
  const grade = (v: string) => fixture.check(v, NO_TOOLS)

  it('accepts the prompt’s own correct answer — headings with nothing under them', () => {
    expect(grade('## Decisions\n\n## Facts\n\n## Open\n')).toBeNull()
  })

  it('accepts saying it plainly', () => {
    expect(grade('Nothing durable — the conversation was scheduling and small talk.')).toBeNull()
  })

  it('accepts FAITHFUL COMPRESSION, which is what it used to fail', () => {
    // Verbatim from the sweep. Three of eleven models were scored here for
    // these, and every line traces to the transcript: "no, nothing to hold",
    // "it can wait, it was not urgent", "I will ping you when I surface".
    // Nothing was invented, and invention is what this fixture is named for.
    expect(grade('## Decisions\n- User will not hold tasks until ~3 PM\n- Meeting message can wait\n')).toBeNull()
    expect(grade('## Decisions\n- User will ping Nomad when available after ~3 PM\n')).toBeNull()
  })

  it('cannot be passed by a reply that engages with nothing', () => {
    // THE REGISTRY-WIDE CENSUS CAUGHT THIS, which is the whole reason it exists:
    // when this check was first loosened, replaying the literal string
    // `{"nope": true}` scored a PASS here — two invented words and fourteen
    // characters slipped under both thresholds. "There was nothing durable" is a
    // claim ABOUT this conversation and has to touch it.
    expect(grade('{"nope": true}')).toContain('does not engage')
  })

  it('still fails REAL invention — the thing it exists to catch', () => {
    // Nothing in this conversation is about Postgres, migrations or headcount.
    const invented = grade('## Decisions\n- The team agreed to migrate the ledger to Postgres\n- Hiring is frozen until the migration completes\n')
    expect(invented).toContain('never mentioned')
  })

  it('still fails a "distillation" that is not one', () => {
    // Padding a chatter transcript back out to its own length is the other way
    // to get this wrong, and it is not invention — so it gets its own sentence.
    const padded = grade(`## Facts\n${'- the user will ping Nomad after three when they surface\n'.repeat(14)}`)
    expect(padded).toContain('characters about a')
  })
})
