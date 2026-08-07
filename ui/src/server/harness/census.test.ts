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
    harnesses: 26,
    fixtures: 247,
    widens: 12,
    refuses: ['judge', 'research-search'],
    roles: 11,
    platformAgents: 9,
  })
})

// The tier-1 bill, which is the number an admin decides on before spending
// money — and the number the arming round moved, because `tools` and
// `tool-select` used to skip on every model and now make five calls on any
// gateway-served one.
it('probe calls', () => {
  const total = PROBES.reduce((n, p) => n + p.calls, 0)
  // `vision` is the only probe that skips everywhere (Message.content is a
  // string), so a gateway candidate is billed for everything else.
  const gateway = PROBES.filter((p) => p.id !== 'vision').reduce((n, p) => n + p.calls, 0)
  // A fleet persona additionally skips both tool probes: its tool loop runs
  // inside the agent container, where Talaria can neither place a definition nor
  // observe a call.
  const persona = PROBES.filter((p) => !['vision', 'tools', 'tool-select'].includes(p.id)).reduce((n, p) => n + p.calls, 0)
  expect({ probes: PROBES.length, total, gateway, persona }).toEqual({ probes: 9, total: 23, gateway: 22, persona: 17 })
})
