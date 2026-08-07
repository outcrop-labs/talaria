import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SessionUser } from '@/server/auth/session'
import { finalizeItem, focusAction, requiresHumanConfirmation, validateCommandObject } from '@/server/inbox-focus-policy'
import { allowedFocusActionIds, inboxCommandHarness, type FocusCommandInput } from '@/server/harness/defs/inbox-focus'
import { capabilityKey, recordCapability } from '@/server/harness/capability'
import { runHarness } from '@/server/harness/run'
import type { GuardConfig } from '@/server/guardrails'
import type { RawFocusItem } from '@/server/inbox-focus-types'

// A WIDENED PROPOSAL REQUIRES A HUMAN CLICK, held still.
//
// The hole this file covers: `focusAction('approve_task', 'Approve', 'safe')`
// yields `confirmationRequired: false`, and `runFocusAction` read that field and
// nothing else — so a proposal carrying `approve_task` ran
// `completeQualityReview(..., 'approved')` on arrival, wrote an audit row naming
// the human who never clicked, and offered no undo. It could not fire while the
// command allowlist was `[deterministicActionId]`, and the capability-gated
// widening plus the first `value: true` probe fact would have armed it with no
// code change at all.
//
// So the assertions here are about the DIVISION, not about approve_task: a
// proposal a model selected takes the confirmation path, a proposal the regexes
// matched does not, and a person clicking a button on the card is unaffected.
// Everything the module touches is faked — no database, no gateway, no fleet.

interface DecisionRow {
  id: string
  userId: string
  sourceType: string
  sourceId: string
  instruction: string | null
  actionId: string | null
  agentModel: string | null
  delegateModel: string | null
  status: string
  proposal: Record<string, unknown> | null
  outcome: unknown
  confirmationTokenHash: string | null
  expiresAt: Date | null
}

const decisions = new Map<string, DecisionRow>()
let nextDecisionId = 0

/** A postgres.js-shaped tagged template that answers by inspecting the query
 *  text, keyed on the text rather than on call order so a query this module
 *  stops issuing stops being answered instead of inheriting the shape meant for
 *  the one before it. It models `inbox_decisions` faithfully enough for the
 *  guards that matter: the command-proposal lookup, the confirmation write and
 *  the token consume all have to agree about one row. */
const sql = Object.assign(
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]> => {
    const text = strings.join(' ').replace(/\s+/g, ' ').trim()

    if (text.includes('from agent_defs')) {
      return Promise.resolve(text.includes('select 1') ? [{ one: 1 }] : [{ model: 'penny', name: 'Penny' }])
    }

    if (text.startsWith('insert into inbox_decisions')) {
      const id = `decision-${++nextDecisionId}`
      decisions.set(id, {
        id,
        userId: String(values[0]),
        sourceType: String(values[1]),
        sourceId: String(values[2]),
        instruction: (values[3] ?? null) as string | null,
        actionId: (values[4] ?? null) as string | null,
        agentModel: (values[5] ?? null) as string | null,
        delegateModel: (values[6] ?? null) as string | null,
        status: String(values[7]),
        proposal: (values[8] ?? null) as Record<string, unknown> | null,
        outcome: values[9] ?? null,
        confirmationTokenHash: (values[10] ?? null) as string | null,
        expiresAt: (values[11] ?? null) as Date | null,
      })
      return Promise.resolve([{ id }])
    }

    // `commandDecision` — the model's proposal, before any token exists.
    if (text.startsWith('select proposal, agent_model')) {
      const row = decisions.get(String(values[0]))
      const usable =
        row &&
        row.status === 'proposed' &&
        row.instruction !== null &&
        row.confirmationTokenHash === null &&
        row.actionId === values[4] &&
        row.proposal?.sourceFingerprint === values[5]
      return Promise.resolve(usable ? [{ proposal: row.proposal, agentModel: row.agentModel, delegateModel: row.delegateModel }] : [])
    }

    // `proposedConfirmation` upgrading the command row with a token:
    // proposal, tokenHash, expiresAt, agentModel, delegateModel, then id.
    if (text.startsWith('update inbox_decisions set proposal =')) {
      const row = decisions.get(String(values[5]))
      if (!row || row.status !== 'proposed' || row.confirmationTokenHash !== null) return Promise.resolve([])
      row.proposal = values[0] as Record<string, unknown>
      row.confirmationTokenHash = String(values[1])
      row.expiresAt = values[2] as Date
      return Promise.resolve([{ id: row.id }])
    }

    // `consumeConfirmation` — the human's click, redeeming the token.
    if (text.startsWith("update inbox_decisions set status = 'confirmed'")) {
      const row = decisions.get(String(values[0]))
      const ok =
        row &&
        row.status === 'proposed' &&
        row.actionId === values[4] &&
        row.confirmationTokenHash === values[5] &&
        row.proposal?.sourceFingerprint === values[6] &&
        (row.expiresAt?.getTime() ?? 0) > Date.now()
      if (!ok || !row) return Promise.resolve([])
      row.status = 'confirmed'
      row.confirmationTokenHash = null
      return Promise.resolve([{ proposal: row.proposal, agentModel: row.agentModel, delegateModel: row.delegateModel }])
    }

    // `replayDecision` and the `consumeConfirmation` miss probe both read status.
    if (text.startsWith('select status, outcome')) {
      const row = decisions.get(String(values[0]))
      return Promise.resolve(row ? [{ status: row.status, outcome: row.outcome, completedAt: null }] : [])
    }
    if (text.startsWith('select status, expires_at')) {
      const row = decisions.get(String(values[0]))
      return Promise.resolve(row ? [{ status: row.status, expiresAt: row.expiresAt, sourceFingerprint: row.proposal?.sourceFingerprint ?? null }] : [])
    }

    // `completeDecision`: status, outcome, then id.
    if (text.startsWith('update inbox_decisions set status =')) {
      const row = decisions.get(String(values[2]))
      if (row) {
        row.status = String(values[0])
        row.outcome = values[1]
      }
      return Promise.resolve(row ? [{ id: row.id }] : [])
    }

    return Promise.resolve([])
  },
  { json: (value: unknown) => value, unsafe: (value: string) => value },
)

// Typed to the real signature so `mock.calls[0][2]` is the review verdict and
// not an index into an empty tuple — the assertion that a confirmed click, and
// only a confirmed click, signs the work off reads that argument.
const completeQualityReview = vi.fn(async (_taskId: string, _actor: string, _verdict: 'approved' | 'rejected', _status: string) => ({
  id: 'task-1',
  status: 'done',
}))
// HOISTED, and it has to be: the last describe imports `harness/capability.ts`
// at the top of the file, which pulls the `@/server/audit` mock factory in
// before ordinary `const`s in this module have initialized.
const { logAudit, settings } = vi.hoisted(() => ({
  logAudit: vi.fn(async () => {}),
  // `app_settings` in a Map. `capability.ts` reads and writes capability facts
  // through `getSetting`/`setSetting`, and the last describe needs a REAL
  // capability record — a faked `capabilities` dep would only prove that the
  // fake widens the harness, which is not the question.
  settings: new Map<string, unknown>(),
}))
const requestFocusCommand = vi.fn()

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/api-guard', () => ({ actorOf: (user: { id: string; email: string | null }) => user.email ?? user.id }))
vi.mock('@/server/auth/session', () => ({ randomToken: () => 'test-confirmation-token' }))
vi.mock('@/server/secretbox', () => ({ seal: (value: string) => `sealed:${value}`, open: (value: string) => value.replace('sealed:', '') }))
vi.mock('@/server/boards', () => ({ boardRole: async () => 'editor', canEdit: () => true }))
vi.mock('@/server/channels', () => ({ channelRole: async () => 'member', insertChannelMessage: async () => ({ id: 'm', seq: 1 }), markChannelRead: async () => {} }))
vi.mock('@/server/channel-replies', () => ({ notifyDmMessage: async () => {}, notifyUserMentions: async () => {}, triggerAgentReplies: async () => {} }))
vi.mock('@/server/google/pending-actions', () => ({ decideAction: async () => null, listPending: async () => [] }))
vi.mock('@/server/audit', () => ({
  logAudit,
  getSetting: async <T,>(key: string, fallback: T): Promise<T> => (settings.has(key) ? (settings.get(key) as T) : fallback),
  setSetting: async (key: string, value: unknown): Promise<void> => {
    settings.set(key, value)
  },
}))
vi.mock('@/server/tasks', () => ({ completeQualityReview, getTask: async () => ({ id: 'task-1', boardId: 'board-1', status: 'quality_review' }) }))
vi.mock('@/server/statuses', () => ({
  statusMeta: async () => ({ keys: ['inbox', 'in_progress', 'quality_review', 'done'], reviewKeys: ['quality_review'], doneKeys: ['done'], assignedKey: 'assigned' }),
}))
vi.mock('@/server/fleet-agents', () => ({ routedModelFor: (model: string) => model }))
// `memberModelAllowlist` / `modelAllowedFor` are here for `harness/model.ts`,
// which the last describe reaches through `runHarness`. Neither is consulted on
// a pinned run; a missing export is an import-time error either way.
vi.mock('@/server/model-access', () => ({
  gatewayModelsFor: async () => [],
  memberModelAllowlist: async () => [],
  modelAllowedFor: () => true,
}))
vi.mock('@/server/inbox-focus-assistant', () => ({ requestFocusBrief: async () => null, requestFocusCommand }))
vi.mock('@/server/inbox-focus-sources', () => ({
  approvalItems: async () => [],
  channelItems: async () => [],
  notificationItems: async () => [],
  taskItems: async () => [reviewItem()],
}))

const { runFocusAction, runFocusCommand } = await import('@/server/inbox-focus')

/** Exactly what `taskItems` builds for a ticket awaiting review: two sign-off
 *  actions, both `risk: 'safe'`, neither carrying `confirmationRequired`. */
function reviewItem(): RawFocusItem {
  return finalizeItem({
    key: 'task:task-1',
    sourceType: 'task',
    sourceId: 'task-1',
    priority: 'p1',
    statusLabel: 'REVIEW',
    createdAt: '2026-08-06T12:00:00.000Z',
    updatedAt: '2026-08-06T12:00:00.000Z',
    dueAt: null,
    question: 'Approve the completed work for “Ledger migration”?',
    recommendation: 'Review the evidence, then approve it or request changes.',
    recommendedActionId: 'approve_task',
    evidence: [{ label: 'Task', text: 'Dex reports the migration is done and the tests pass.' }],
    metadata: { board: 'Platform', status: 'quality_review' },
    sourceHref: '/boards/board-1/task-1',
    briefStatus: 'fallback',
    actions: [focusAction('approve_task', 'Approve', 'safe'), focusAction('request_changes', 'Request changes', 'safe')],
    bucket: 3,
  })
}

const USER: SessionUser = {
  id: 'user-1',
  sub: 'sub-1',
  email: 'owner@example.com',
  name: 'Owner',
  picture: null,
  provider: 'google',
  role: 'admin',
}

/** The command input the harness would have been rendered with, rebuilt here so
 *  the gate test asks `allowedFocusActionIds` the same question production does
 *  rather than a paraphrase of it. */
const commandInput = (item: RawFocusItem, deterministicActionId: string | null): FocusCommandInput => ({
  item,
  instruction: 'approve it',
  history: [],
  mode: 'normal',
  deterministicActionId,
  role: 'orchestrator',
  specialist: null,
})

const APPROVE = { kind: 'proposal' as const, message: 'Ready to approve.', actionId: 'approve_task' }

/** The whole turn: the model proposes, the Inbox hands the proposal back with
 *  its decision id, and the client immediately asks for it to run — which is
 *  exactly what `InboxFocusShell` does on any proposal. */
async function proposeThenRun(instruction: string, options: { delegateModel?: string } = {}) {
  const command = await runFocusCommand(USER, { key: 'task:task-1', instruction, ...options })
  expect(command?.actionId).toBe('approve_task')
  const proposal = decisions.get(command!.decisionId)?.proposal
  const result = await runFocusAction(USER, { key: 'task:task-1', actionId: 'approve_task', commandDecisionId: command!.decisionId })
  return { command: command!, proposal, result }
}

beforeEach(() => {
  decisions.clear()
  settings.clear()
  nextDecisionId = 0
  completeQualityReview.mockClear()
  logAudit.mockClear()
  requestFocusCommand.mockReset()
})

describe('a proposal a model selected', () => {
  it('RETURNS A CONFIRMATION RATHER THAN SIGNING THE WORK OFF', async () => {
    // "what do you make of this?" matches no regex, so nothing deterministic
    // authorized anything. A widened model can still reach `approve_task` —
    // and this is the click that stands between it and `completeQualityReview`.
    requestFocusCommand.mockResolvedValue(APPROVE)
    const { proposal, result } = await proposeThenRun('what do you make of this?')

    expect(proposal?.source).toBe('widened')
    expect(result.status).toBe('confirmation_required')
    expect(result.confirmationToken).toBeTruthy()
    expect(completeQualityReview).not.toHaveBeenCalled()
    expect(logAudit).not.toHaveBeenCalled()
  })

  it('executes on the owner’s confirming click, through the token it was issued', async () => {
    // The other half: the gate has to be passable. This is the same path a
    // `confirmationRequired` action has always taken, which is why the decision
    // routes through it instead of growing a second flow.
    requestFocusCommand.mockResolvedValue(APPROVE)
    const { result } = await proposeThenRun('what do you make of this?')

    const confirmed = await runFocusAction(USER, {
      key: 'task:task-1',
      actionId: 'approve_task',
      decisionId: result.decisionId,
      confirmationToken: result.confirmationToken,
    })

    expect(confirmed.status).toBe('completed')
    expect(completeQualityReview).toHaveBeenCalledTimes(1)
    expect(completeQualityReview.mock.calls[0]?.[2]).toBe('approved')
    // The audit row names the session user, which is only honest because the
    // user clicked. That was the sharpest edge of the original bug.
    expect(logAudit).toHaveBeenCalledTimes(1)
  })

  it('spends its confirmation token once — a second redemption replays, it does not re-approve', async () => {
    requestFocusCommand.mockResolvedValue(APPROVE)
    const { result } = await proposeThenRun('what do you make of this?')
    const args = { key: 'task:task-1', actionId: 'approve_task', decisionId: result.decisionId, confirmationToken: result.confirmationToken }

    expect((await runFocusAction(USER, args)).status).toBe('completed')
    // `replayDecision` answers the second call from the stored outcome. The
    // status is the same and the work is not done twice, which is the property
    // that would have been lost had the token branch stayed nested inside
    // `if (action.confirmationRequired)`.
    expect((await runFocusAction(USER, args)).status).toBe('completed')
    expect(completeQualityReview).toHaveBeenCalledTimes(1)
  })
})

describe('a proposal the regexes matched', () => {
  it('still executes directly — the owner already said "approve it"', async () => {
    // The product must stay usable. `deterministicProposal` matched
    // `approve_task` from the instruction itself, and a model that echoed the
    // one id it was shown adds no authority to that.
    requestFocusCommand.mockResolvedValue(APPROVE)
    const { proposal, result } = await proposeThenRun('approve it')

    expect(proposal?.source).toBe('deterministic')
    expect(result.status).toBe('completed')
    expect(completeQualityReview).toHaveBeenCalledTimes(1)
  })

  it('a human clicking the card is never asked to confirm their own click', async () => {
    const result = await runFocusAction(USER, { key: 'task:task-1', actionId: 'approve_task' })
    expect(result.status).toBe('completed')
    expect(completeQualityReview).toHaveBeenCalledTimes(1)
  })
})

describe('the specialist fall-through', () => {
  it('needs a click even when it proposes the very action the regexes matched', async () => {
    // The delegate is a second model the owner pointed at this item, not their
    // own assistant answering a deterministic instruction, and the fall-through
    // UNIONS the two seats' judgement. The instruction here does match the
    // regex, which is what makes this the sharp case: it is the SEAT that
    // requires the click, not the action id.
    requestFocusCommand.mockImplementation(async (_model: string, input: { role: string }) =>
      input.role === 'specialist' ? APPROVE : null,
    )
    const { command, proposal, result } = await proposeThenRun('approve it', { delegateModel: 'dex' })

    expect(command.consultedModel).toBe('dex')
    expect(proposal?.source).toBe('delegate')
    expect(result.status).toBe('confirmation_required')
    expect(completeQualityReview).not.toHaveBeenCalled()
  })

  it('does not need one when the orchestrator answered and the regexes agree', async () => {
    requestFocusCommand.mockResolvedValue(APPROVE)
    const { proposal, result } = await proposeThenRun('approve it', { delegateModel: 'dex' })

    expect(proposal?.source).toBe('deterministic')
    expect(result.status).toBe('completed')
  })
})

// ── The thing that finally arms widening does not open the gate ──────────────
//
// Every describe above stubs `requestFocusCommand`, so none of them has ever
// executed the path that ARMS widening: a `recordCapability({ value: true })`
// fact. Until the tier-1 probes shipped, nothing in Talaria wrote one — the
// gateway only ever writes `value: false` from a 400 — so `runHarness`'s
// widening branch had never been reachable in production, and this gate had
// never been tested against a live one.
//
// So this block writes the two facts `runProbes` writes, with `source: 'probe'`,
// and lets the REAL `capability.ts` answer the runner. Nothing about widening is
// faked: the transport is, the capability record is not.
const PROBED_AT = '2026-08-06T12:00:00.000Z'
const GUARD_OFF: GuardConfig = { mode: 'off', checks: {}, minConfidence: 1, policedHosts: [], coach: false }

/** Exactly what `runProbes` records for a model that scored 4/4 on tool-select
 *  and 3/3 on the exact-instruction trials. */
async function probeTrue(): Promise<void> {
  const key = capabilityKey('spark', 'pl-main')
  await recordCapability(key, 'tool-select', { value: true, source: 'probe', at: PROBED_AT, score: 1, detail: 'picked the correct tool on all 4 prompts' })
  await recordCapability(key, 'instruction-following', { value: true, source: 'probe', at: PROBED_AT, score: 1, detail: 'reproduced all 3 exact-output instructions verbatim' })
}

/** `requestFocusCommand`, for real: run the command harness against a recorded
 *  reply with the probe facts in place, then gate the value on the allowlist the
 *  run's OWN `widened` flag produced. This is the production shape — the widened
 *  surface and the surface the answer is validated against are one object. */
function harnessBackedCommand(reply: string): (model: string, input: FocusCommandInput) => Promise<unknown> {
  return async (_model, input) => {
    const result = await runHarness(inboxCommandHarness, input, {
      caller: 'test:inbox-authority',
      model: 'pl-main',
      deps: {
        // The capability lookups are deliberately NOT overridden.
        routing: async (model) => ({ endpoints: ['spark'], upstreamModel: model }),
        transport: async () => ({ kind: 'gateway', text: reply, toolNames: [], usage: null, contractDropped: false }),
        guardConfig: async () => GUARD_OFF,
        guardText: async () => [],
        recordFindings: async () => {},
        recordRun: async () => {},
      },
    })
    widenedLastRun = result.widened
    return result.value ? validateCommandObject(result.value, new Set(allowedFocusActionIds(input, result.widened))) : null
  }
}
let widenedLastRun = false

describe('a capability a probe recorded true', () => {
  it('WIDENS THE SURFACE AND STILL CANNOT SIGN THE WORK OFF WITHOUT A CLICK', async () => {
    await probeTrue()
    requestFocusCommand.mockImplementation(harnessBackedCommand('{"message":"Ready to approve.","actionId":"approve_task"}'))

    // No regex matches this instruction, so the ONLY reason `approve_task` is
    // proposable at all is the probe fact.
    const { proposal, result } = await proposeThenRun('what do you make of this?')

    expect(widenedLastRun).toBe(true)
    expect(proposal?.source).toBe('widened')
    // `focusAction('approve_task', 'Approve', 'safe')` yields
    // `confirmationRequired: false`, which is the field the original bug read
    // and nothing else. The seat is what requires the click.
    expect(reviewItem().actions.find((a) => a.id === 'approve_task')?.confirmationRequired).toBe(false)
    expect(requiresHumanConfirmation(focusAction('approve_task', 'Approve', 'safe'), 'widened')).toBe(true)
    expect(result.status).toBe('confirmation_required')
    expect(completeQualityReview).not.toHaveBeenCalled()
    expect(logAudit).not.toHaveBeenCalled()
  })

  it('is the difference: with no fact recorded the same reply is not even proposable', async () => {
    // The control. Same harness, same reply, no probe — `runHarness` leaves the
    // allowlist at the regex match (here, nothing), `validateCommandObject`
    // returns null, and the turn falls through to a clarification. Widening is
    // real, and this is what it is worth.
    requestFocusCommand.mockImplementation(harnessBackedCommand('{"message":"Ready to approve.","actionId":"approve_task"}'))
    const command = await runFocusCommand(USER, { key: 'task:task-1', instruction: 'what do you make of this?' })

    expect(widenedLastRun).toBe(false)
    expect(command?.actionId ?? null).toBeNull()
  })

  // BOTH facts are written, and that is the point of the two cases below. An
  // earlier version wrote only `tool-select` and left `instruction-following`
  // unmeasured, so `widened: false` was satisfied by the missing SECOND fact and
  // the assertion held even with the provenance check deleted — a test that
  // passes under the mutation it exists to catch. Writing the whole pair makes
  // `source` the only thing standing between this seat and a widened surface.
  it('does not widen on a capability the GATEWAY learned — only a deliberate measurement counts', async () => {
    // `llm-gateway.ts` writes `learned` facts from an upstream 400 and they are
    // always `value: false`, so a `learned: true` cannot arise in production.
    // Asserting it anyway pins the direction: if a future writer ever learns a
    // positive fact from a provider's own advertisement, THIS is the test that
    // has to be read before it is allowed to widen anything.
    const key = capabilityKey('spark', 'pl-main')
    const at = new Date().toISOString()
    await recordCapability(key, 'tool-select', { value: true, source: 'learned', at })
    await recordCapability(key, 'instruction-following', { value: true, source: 'learned', at })
    requestFocusCommand.mockImplementation(harnessBackedCommand('{"message":"Ready to approve.","actionId":"approve_task"}'))
    await runFocusCommand(USER, { key: 'task:task-1', instruction: 'what do you make of this?' })

    expect(widenedLastRun).toBe(false)
  })

  it('does not widen on a DECLARED capability either — a vendor cannot open this seat by claiming one', async () => {
    // The one that would actually happen. `declared` is what an admin types and
    // what a model catalog would sync, and the floor above accepts it as grounds
    // to REFUSE — so the temptation to accept it here too is real, and the
    // asymmetry is the whole argument in run.ts step 3: refusing on a claim
    // costs a click, widening on a claim hands a model the item's action list.
    const key = capabilityKey('spark', 'pl-main')
    const at = new Date().toISOString()
    await recordCapability(key, 'tool-select', { value: true, source: 'declared', at })
    await recordCapability(key, 'instruction-following', { value: true, source: 'declared', at })
    requestFocusCommand.mockImplementation(harnessBackedCommand('{"message":"Ready to approve.","actionId":"approve_task"}'))
    const command = await runFocusCommand(USER, { key: 'task:task-1', instruction: 'what do you make of this?' })

    expect(widenedLastRun).toBe(false)
    // And the consequence, not just the flag: the allowlist stayed at the regex
    // match (nothing), so the model's `approve_task` never became a proposal.
    expect(command?.actionId ?? null).toBeNull()

    // Re-measure the SAME pair as probe facts and the seat opens — the only
    // difference between the two halves of this case is `source`.
    await recordCapability(key, 'tool-select', { value: true, source: 'probe', at, score: 1 })
    await recordCapability(key, 'instruction-following', { value: true, source: 'probe', at, score: 1 })
    const { proposal, result } = await proposeThenRun('what do you make of this?')
    expect(widenedLastRun).toBe(true)
    expect(proposal?.source).toBe('widened')
    // ...and it STILL cannot sign the work off. Widening moves the surface, not
    // the seat.
    expect(result.status).toBe('confirmation_required')
    expect(completeQualityReview).not.toHaveBeenCalled()
  })
})

describe('the authority gate is unchanged', () => {
  it('validateCommandObject still rejects an action the item does not carry, widened or not', () => {
    // Untouched by any of the above, and it must stay that way: widening changes
    // WHICH actions are offered, the gate decides whether the model stayed
    // inside them, and confirmation decides whether a human has to click. Three
    // separate questions.
    const item = reviewItem()
    for (const widened of [true, false]) {
      const allowed = new Set(allowedFocusActionIds(commandInput(item, widened ? null : 'approve_task'), widened))
      expect(validateCommandObject({ message: 'Deleting it now.', actionId: 'delete_task' }, allowed)).toBeNull()
    }
    // And the widened list is the item's own actions, never anything beyond.
    const wide = new Set(allowedFocusActionIds(commandInput(item, null), true))
    expect([...wide]).toEqual(['approve_task', 'request_changes'])
    expect(validateCommandObject({ message: 'Ready to approve.', actionId: 'approve_task' }, wide)?.kind).toBe('proposal')
  })
})
