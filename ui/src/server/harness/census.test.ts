import { expect, it } from 'vitest'
import { PROBES } from '../fitness/probes'
import { builtinActivityHarnesses } from './registry'
import { MODEL_ROLES } from '../model-roles'
import { PLATFORM_AGENTS } from '../platform-agents'

// THE NUMBERS THE DOCS QUOTE, held as assertions.
//
// `docs/HARNESSES.md` states all of these in prose — 23 harnesses, 70 fixtures,
// twelve that widen, exactly two that refuse, 20 slots, 22 probe calls on a
// gateway candidate — and prose has no build step. This repo has already shipped
// a `docs/SDK.md` that described a React SDK for months after the tree stopped
// having one; the failure mode is not "a stale sentence", it is a developer
// following a document that cannot work.
//
// So a change that moves any of these fails HERE, next to a comment naming the
// document to update, rather than being discovered by whoever trusted the number.
// Failing this test is not a bug: add the harness, then update the doc and the
// expectation together.
it('census', () => {
  const hs = builtinActivityHarnesses()
  const fixtures = hs.reduce((n, h) => n + h.evalNames.length, 0)
  const widens = hs.filter((h) => h.widen).length
  const refuses = hs.filter((h) => h.floor.refuseBelow).map((h) => h.id)
  const report = {
    harnesses: hs.length,
    fixtures,
    widens,
    refuses,
    roles: MODEL_ROLES.length,
    platformAgents: PLATFORM_AGENTS.length,
  }
  expect(report).toEqual({
    harnesses: 31,
    fixtures: 284,
    widens: 12,
    // EVERY JSON HARNESS REFUSES, derived rather than declared — see
    // `withJsonFloor` in define.ts. A harness whose output contract is a schema
    // needs structured output by construction, so a model MEASURED unable to
    // produce a parseable object is unfit for it, and sending prose to a parser
    // and recording the wreckage as the model's failure is the behaviour this
    // replaced. The two text harnesses that already refused keep their own
    // reasons.
    refuses: [
      'blurb-writer',
      'muse:cron',
      'muse:agent',
      'muse:ticket',
      'judge',
      'inbox-brief',
      'inbox-command',
      'channel-plan',
      'research-queries',
      'research-search',
    ],
    roles: 11,
    platformAgents: 9,
  })
})

// The tier-1 bill, which is the number an admin decides on before spending
// money. It moved twice: once when `tools` and `tool-select` were armed, and
// again when `vision` stopped skipping — the image channel now builds its own
// multimodal body (`runnerImageAsk`) instead of waiting on a tree-wide
// `Message.content` widening, and its fixture went from one degenerate 1x1 to
// three real colour fields.
it('probe calls', () => {
  const total = PROBES.reduce((n, p) => n + p.calls, 0)
  // NOTHING SKIPS ON A GATEWAY CANDIDATE ANY MORE. Every capability in the union
  // is measurable there, which is the point of the tier: a blank tag should mean
  // "nobody has run this yet", never "Talaria has no way to ask".
  const gateway = total
  // A fleet persona still skips the three that need a seam its container does
  // not give us: both tool probes (its loop runs inside the agent, where we can
  // neither place a definition nor observe a call) and vision (its turn is
  // rendered in there too, so there is no raw body to attach an image to).
  const persona = PROBES.filter((p) => !['vision', 'tools', 'tool-select'].includes(p.id)).reduce((n, p) => n + p.calls, 0)
  expect({ probes: PROBES.length, total, gateway, persona }).toEqual({ probes: 9, total: 25, gateway: 25, persona: 17 })
})
