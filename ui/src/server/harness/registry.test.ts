// The registry's job is to hold the two halves of the harness system together:
// the executable definitions in defs/, and the metadata in PLATFORM_AGENTS that
// an admin actually sees. Every assertion below exists because eight harnesses
// were written independently and then merged, and each one is a way the merge
// could have gone wrong quietly.
import { describe, expect, it } from 'vitest'
import { RULES } from '@/server/guardrails'
import { PLATFORM_AGENTS } from '@/server/platform-agents'
import { builtinActivityHarnesses, listActivityHarnesses, platformAgentOf } from './registry'

const harnesses = builtinActivityHarnesses()
const RULE_IDS = new Set(RULES.map((r) => r.id))

describe('the registry', () => {
  it('lists every harness exactly once', async () => {
    const ids = harnesses.map((h) => h.id)
    expect(new Set(ids).size).toBe(ids.length)
    expect(ids).toEqual([
      'titler',
      'summarizer',
      'librarian',
      'blurb-writer',
      'distiller',
      'concluder',
      'muse:cron',
      'muse:agent',
      'muse:ticket',
      'muse:skill-form',
      'muse:template-form',
      'muse:draft',
      'judge',
      'inbox-brief',
      'inbox-command',
      'inbox-reply',
      'briefer:brief',
      'briefer:chat',
      'briefer:daily-open',
      'briefer:daily-delta',
      'briefer:daily-chat',
      'briefer:reply',
      'work-session',
      'hermes:knowledge',
      'hermes:documents',
      'hermes:governance',
      'hermes:google',
      'hermes:research',
      'secrets:handles',
      'workbench:light',
      'workbench:standard',
      'workbench:heavy',
      'channel-plan',
      'plan-doc',
      'outreach:check-in',
      'research-queries',
      'research-search',
      'research-synthesis',
    ])
  })

  it('has a registered harness for every definition in defs/', async () => {
    // THE ASSERTION THAT WOULD HAVE CAUGHT PHASE 3. Nine definitions landed with
    // 32 eval fixtures between them and none were added to BUILTINS, because
    // this file was read-only to the agents that wrote them — so the fitness
    // suite could not see a single one and every `check` they wrote was dead
    // code. The list above locks the ORDER; this locks the COVERAGE, and it is
    // the one that fails when somebody adds a definition and forgets the
    // registry line.
    // The negative pattern is load-bearing: `eager: true` IMPORTS every match,
    // and importing a sibling .test.ts registers its describe blocks into this
    // file's run. Filtering the keys afterwards is too late.
    const modules = import.meta.glob(['./defs/*.ts', '!./defs/*.test.ts'], { eager: true }) as Record<string, Record<string, unknown>>
    const declared = new Set<string>()
    for (const mod of Object.values(modules)) {
      for (const value of Object.values(mod)) {
        // A definition, structurally: `defineHarness` is an identity function, so
        // there is no brand to check for and the shape is what identifies one.
        if (!value || typeof value !== 'object') continue
        const v = value as Partial<{ id: unknown; render: unknown; floor: unknown; output: unknown }>
        if (typeof v.id === 'string' && typeof v.render === 'function' && v.floor && v.output) declared.add(v.id)
      }
    }
    expect(declared.size, 'no harness definitions were found — the glob is wrong, not the registry').toBeGreaterThan(0)
    const registered = new Set(harnesses.map((h) => h.id))
    const missing = [...declared].filter((id) => !registered.has(id)).sort()
    expect(missing, 'defined in defs/ but not in BUILTINS — invisible to the fitness suite and the admin panel').toEqual([])
  })

  it('merges the three layers by id', async () => {
    // App and custom layers are stubs today, so the merged list is the builtin
    // one — but the merge is what the fitness suite and the admin panel call,
    // and it must not lose a harness on the way through.
    const merged = await listActivityHarnesses()
    expect(merged.map((h) => h.id)).toEqual(harnesses.map((h) => h.id))
    expect(merged.every((h) => h.source === 'builtin')).toBe(true)
  })

  it('hands back a definition with its input and output types still paired', () => {
    // `use` is the whole reason this list can be heterogeneous without an `any`.
    const titler = harnesses.find((h) => h.id === 'titler')
    expect(titler?.use((def) => def.id)).toBe('titler')
  })
})

describe('every harness declaration', () => {
  it('names itself for a human', () => {
    for (const h of harnesses) {
      expect(h.label.trim(), h.id).not.toBe('')
      expect(h.job.trim(), h.id).not.toBe('')
      expect(h.floor.note.trim(), h.id).not.toBe('')
    }
  })

  it('narrows the guard to rule ids that EXIST', () => {
    // The sharpest silent failure in the whole declaration surface:
    // `narrowGuardConfig` turns a rule on only when the harness names it, so a
    // typo does not disable one rule — it disables ALL of them, with no error
    // anywhere and a `guard` block in the file that reads as protection.
    for (const h of harnesses) {
      for (const rule of h.guard?.rules ?? []) {
        expect(RULE_IDS.has(rule), `${h.id} declares guard rule "${rule}", which is not in the registry`).toBe(true)
      }
    }
  })

  it('makes every harness NAME its rules, because an omitted block runs all of them', () => {
    // The sibling of the case above, and the level it could not reach. That one
    // catches a rule id that does not exist; this one catches the block not
    // being there at all — and the two failures are opposites, which is why one
    // assertion cannot cover both. `narrowGuardConfig` returns the FULL config
    // when `rules` is undefined, so deleting a `guard` block does not weaken a
    // harness, it runs EVERY enabled rule on it. That is what put the judge on
    // `zero_tool_claim` and `fabricated_outage` — rules structurally wrong for a
    // verdict, which describes claimed work rather than doing any — and inflated
    // `guard_findings.model` for whichever model an admin had chosen to judge
    // with, which is the per-model confabulation rate the fitness page reads.
    //
    // So: no harness may leave the block off. Every one of the 23 has an answer
    // and every answer is written down. If a future harness genuinely wants all
    // rules, it says so by listing them.
    for (const h of harnesses) {
      expect(h.guard, `${h.id} declares no guard block, which silently opts it into every enabled rule`).not.toBeNull()
      expect(h.guard?.rules?.length, `${h.id} has a guard block that names no rules — same effect as having none`).toBeGreaterThan(0)
    }
  })

  it('keeps the refusal list empty unless it actually refuses', () => {
    // `runHarness` reads `floor.capabilities` only when `refuseBelow` is true.
    // Declaring capabilities without refusing is inert, and it reads to the next
    // author as a hard requirement — which is exactly how this port arrived with
    // two spellings of "needs JSON, runs anyway". The ask belongs in `requires`.
    for (const h of harnesses) {
      if (!h.floor.refuseBelow) expect(h.floor.capabilities, `${h.id} declares a floor it never enforces`).toEqual([])
    }
  })

  it('never puts a capability in the floor that is not also required', () => {
    // define.ts: the floor is the non-negotiable SUBSET of `requires`.
    for (const h of harnesses) {
      for (const cap of h.floor.capabilities) {
        expect(h.requires, `${h.id} refuses on "${cap}" without requiring it`).toContain(cap)
      }
    }
  })

  it('spells the Utility fallback exactly one way', () => {
    // `role: 'utility'` and the 'utility' step resolve the same model through
    // the same allowlist gate, and differ only in the `chain_step` recorded on
    // the run — so two harnesses resolving identically would report differently
    // to the fitness page. One spelling: the step. `role` stays available for a
    // harness that genuinely has a role of its own.
    for (const h of harnesses) expect(h.model.role, `${h.id} declares role: 'utility' instead of the 'utility' step`).not.toBe('utility')
  })

  it('gives every widening a reason an admin can read', () => {
    for (const h of harnesses) {
      if (!h.widen) continue
      expect(h.widen.requires.length, `${h.id} widens on nothing`).toBeGreaterThan(0)
      expect(h.widen.note.trim(), h.id).not.toBe('')
    }
  })

  it('keeps the census of contracts a SCHEMA cannot state', () => {
    // THE CENSUS, in `check-invariants.mjs`'s sense: the list is the document,
    // and moving a harness on or off it has to be a deliberate edit here.
    //
    // WHAT IT PROTECTS. `harness_runs.schema_valid` is the OBSERVED half of the
    // model-fitness matrix, and it is only worth reading if it agrees with the
    // offline eval fixtures. For a harness whose correctness is a RELATION
    // between the input and the output, a schema — a module constant built
    // before the input exists — cannot state the contract, so the runner
    // recorded `true` for a value the caller then dropped, while the harness's
    // own `EvalCase.check` rejected the identical value. Four shipped bugs were
    // that one gap; `output.verify` is where each of them now lives.
    //
    // Deleting a `verify` fails this test. That is the point: it is the one
    // change that silently turns the column back into an optimistic liar, and
    // nothing else in the tree would notice.
    const VERIFIES: Record<string, string> = {
      'blurb-writer': 'the keys must be the model ids that were sent - a display-name key writes nothing',
      'muse:ticket': 'a relative date resolves against the "now" in the context, not the model\'s own idea of today',
      'channel-plan': 'a tag must name a label some workflow in the input map defines, or dispatch misroutes on it',
      'inbox-command': 'the proposed actionId must be one the owner\'s instruction authorized, on the surface this run offered',
    }
    const declared = harnesses.filter((h) => h.use((def) => def.output.verify !== undefined)).map((h) => h.id)
    expect(declared.sort()).toEqual(Object.keys(VERIFIES).sort())
    for (const [id, why] of Object.entries(VERIFIES)) expect(why.trim(), id).not.toBe('')
  })

  it('ships eval fixtures, because an unscored harness is an invisible one', () => {
    for (const h of harnesses) {
      expect(h.evalNames.length, `${h.id} declares no eval fixtures`).toBeGreaterThan(0)
      expect(new Set(h.evalNames).size, `${h.id} has two fixtures with the same name`).toBe(h.evalNames.length)
    }
  })

  it('asks for a temperature in range, or none at all', () => {
    for (const h of harnesses) {
      if (h.temperature === null) continue
      expect(h.temperature, h.id).toBeGreaterThanOrEqual(0)
      expect(h.temperature, h.id).toBeLessThanOrEqual(1)
    }
  })
})

describe('the PLATFORM_AGENTS cross-check', () => {
  const agentOf = new Map(harnesses.map((h) => [h.id, platformAgentOf(h)]))

  it('gives every assignable platform agent at least one harness to drive', () => {
    for (const agent of PLATFORM_AGENTS) {
      if (!agent.assignable) continue
      const driven = harnesses.filter((h) => agentOf.get(h.id) === agent.id)
      expect(driven.length, `platform agent "${agent.id}" has no harness — its model assignment goes nowhere`).toBeGreaterThan(0)
    }
  })

  it('records the places the lists deliberately do NOT line up', () => {
    // Locked so that a change is a decision rather than a surprise. See the
    // header of registry.ts for the argument behind each one.
    //
    // 'briefer' is assignable: false — the Inbox and the briefing both run on
    // the owner's own assistant, so there is no pin for a harness to declare.
    // It now HAS harnesses (briefer:brief, briefer:chat); what it does not have
    // is an assignment slot, which is a different sentence and the one that
    // matters.
    expect(PLATFORM_AGENTS.find((a) => a.id === 'briefer')?.assignable).toBe(false)
    expect(harnesses.filter((h) => agentOf.get(h.id) === 'briefer')).toEqual([])
    // ...and from the other side: the harnesses with no platform agent are
    // exactly the ones whose model comes from the SUBJECT of the call — the
    // owner's assistant, the agent on the ticket, the agent in the channel or
    // the plan, the researching agent. Nothing for an admin to assign, but the
    // fitness suite still has to be able to score every one of them.
    //
    // This list growing is how a harness that should have declared a pin gets
    // caught: adding an id here is a claim that no admin slot names its model.
    //
    // THERE ARE THREE KINDS, not two, and the third arrived with the Workbench
    // harnesses. A harness's model comes from a PLATFORM AGENT (an admin
    // assigns it on Models → Platform), from a MODEL ROLE (an admin assigns it
    // on the same page, under a different registry — `MODEL_ROLES`), or from the
    // SUBJECT of the call (the agent on the ticket, in the channel, on the
    // plan). Only the third has nothing for an admin to assign, and only the
    // third may therefore declare no way to resolve a model.
    const roleAssigned = harnesses.filter((h) => agentOf.get(h.id) === null && h.model.role !== undefined).map((h) => h.id)
    expect(roleAssigned).toEqual(['workbench:light', 'workbench:standard', 'workbench:heavy'])

    expect(harnesses.filter((h) => agentOf.get(h.id) === null && h.model.role === undefined).map((h) => h.id)).toEqual([
      'inbox-brief',
      'inbox-command',
      'inbox-reply',
      'briefer:brief',
      'briefer:chat',
      'briefer:daily-open',
      'briefer:daily-delta',
      'briefer:daily-chat',
      'briefer:reply',
      'work-session',
      // The Hermes family: its model is the AGENT IN THE CONVERSATION, so there
      // is nothing for an admin to assign and no role to fall back to — the
      // third kind, exactly like work-session.
      'hermes:knowledge',
      'hermes:documents',
      'hermes:governance',
      'hermes:google',
      'hermes:research',
      // Same third kind: the workspace grants a handle to an AGENT, so the
      // model spending it is whichever one that agent runs.
      'secrets:handles',
      'channel-plan',
      'plan-doc',
      'outreach:check-in',
      'research-queries',
      'research-search',
      'research-synthesis',
    ])
  })

  it('gives a subject-of-the-call harness NO chain to fall back to', () => {
    // The drift this locks: nine of those twelve declared `chain: ['utility',
    // 'first-routable']` and three declared `chain: []`, for the same situation,
    // each with a comment explaining why its spelling was the right one. The
    // non-empty version justified itself as "so the fitness suite can score this
    // on an install with no fleet" — but the fitness suite PINS the candidate
    // model, which is its entire question, so nothing ever read those steps.
    //
    // What they would have done if read is the reason this is an assertion and
    // not a preference: a work-session turn resolved off `utility` runs on a
    // model that is not the assigned agent and is still filed to the ticket as
    // that agent's work. Empty means the runner returns "no model available for
    // harness <id>" instead — a sentence an operator can act on.
    for (const h of harnesses) {
      // A ROLE-ASSIGNED harness is excluded, and it is the one legitimate
      // exception: `workbench:*` names a `MODEL_ROLES` role, which an admin
      // assigns exactly like a platform agent — the model is not the subject of
      // the call, so the argument above does not apply to it.
      if (agentOf.get(h.id) !== null || h.model.role !== undefined) continue
      expect(h.model.chain, `${h.id} declares a fallback chain, but its model comes from the subject of the call`).toEqual([])
      expect(h.model.pin, `${h.id} declares a pin it can never use`).toBeUndefined()
      expect(h.model.role, `${h.id} declares a role it can never use`).toBeUndefined()
    }
  })

  it('keeps the judge exception in one place', () => {
    // The judge HAS a platform agent but declares no pin: its model lives in
    // judge_config so the Guard panel and the Platform panel cannot disagree.
    const judge = harnesses.find((h) => h.id === 'judge')
    expect(judge?.model.pin).toBeUndefined()
    expect(agentOf.get('judge')).toBe('judge')
  })

  it('keeps the label a harness shows and the label its agent shows in step', () => {
    // Only where the mapping is one-to-one: 'muse' drives three harnesses and
    // each names its own job ("Muse — ticket edit"), which is right.
    for (const agent of PLATFORM_AGENTS) {
      const driven = harnesses.filter((h) => agentOf.get(h.id) === agent.id)
      if (driven.length !== 1) continue
      expect(driven[0]?.label, `platform agent "${agent.id}" and its harness disagree about their own name`).toBe(agent.label)
    }
  })
})
