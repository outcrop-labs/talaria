import assert from 'node:assert/strict'
import test from 'node:test'
import {
  dedupeItems,
  confirmationMissInvalidates,
  deterministicProposal,
  finalizeItem,
  focusAction,
  sortItems,
  validateCommandObject,
} from './inbox-focus-policy'
import type { FocusItem, RawFocusItem } from './inbox-focus-types'

function item(
  key: string,
  bucket: number,
  options: Partial<RawFocusItem> = {},
): RawFocusItem {
  const [sourceType, sourceId] = key.split(':') as [RawFocusItem['sourceType'], string]
  return finalizeItem({
    key,
    sourceType,
    sourceId,
    priority: bucket === 0 ? 'p0' : bucket < 4 ? 'p1' : 'p2',
    statusLabel: 'TEST',
    createdAt: '2026-08-03T12:00:00.000Z',
    updatedAt: '2026-08-03T12:00:00.000Z',
    dueAt: null,
    question: key,
    recommendation: 'Review it.',
    recommendedActionId: null,
    evidence: [],
    metadata: {},
    sourceHref: `/${sourceType}/${sourceId}`,
    briefStatus: 'fallback',
    actions: [],
    bucket,
    ...options,
  })
}

test('ranking honors source bucket, due date, explicit priority, then age', () => {
  const now = Date.parse('2026-08-03T12:00:00.000Z')
  const ranked = sortItems([
    item('task:low', 1, { metadata: { priority: 'low' }, createdAt: '2026-08-01T12:00:00.000Z' }),
    item('channel:direct', 2),
    item('task:urgent', 1, { metadata: { priority: 'urgent' }, createdAt: '2026-08-03T11:00:00.000Z' }),
    item('approval:approval', 0),
    item('task:due', 1, { dueAt: '2026-08-04T12:00:00.000Z', metadata: { priority: 'low' } }),
  ], now)

  assert.deepEqual(ranked.map(({ key }) => key), [
    'approval:approval',
    'task:due',
    'task:urgent',
    'task:low',
    'channel:direct',
  ])
})

test('linked notifications yield to their richer source card', () => {
  const task = item('task:123', 1, { sourceHref: '/boards/board/123' })
  const notification = item('notification:note', 2, { sourceHref: '/boards/board/123' })
  assert.deepEqual(dedupeItems([task, notification]).map(({ key }) => key), ['task:123'])
})

test('only an explicit owner instruction deterministically authorizes an action', () => {
  const focusItem = {
    ...item('task:123', 3),
    actions: [focusAction('approve_task', 'Approve', 'safe'), focusAction('request_changes', 'Request changes', 'safe')],
  } satisfies FocusItem

  assert.equal(deterministicProposal(focusItem, 'Can you summarize this?'), null)
  assert.equal(deterministicProposal(focusItem, 'Approve this work')?.actionId, 'approve_task')
  assert.equal(deterministicProposal(focusItem, 'Do not approve this task'), null)
  assert.equal(deterministicProposal(focusItem, 'I am not ready to reject this draft'), null)
  assert.equal(deterministicProposal(focusItem, 'Should I approve this task?'), null)
  assert.equal(
    validateCommandObject({ message: 'Rejected', actionId: 'request_changes' }, new Set(['approve_task'])),
    null,
  )
})

test('negated or explanatory language never authorizes deterministic actions', () => {
  const focusItem = {
    ...item('notification:123', 5),
    actions: [focusAction('mark_read', 'Mark read', 'reversible')],
  } satisfies FocusItem

  assert.equal(deterministicProposal(focusItem, 'Mark this notification read')?.actionId, 'mark_read')
  assert.equal(deterministicProposal(focusItem, 'Do not mark this read'), null)
  assert.equal(deterministicProposal(focusItem, 'I already marked this read'), null)
  assert.equal(deterministicProposal(focusItem, 'Should I mark this read?'), null)
})

test('reply instructions retain the exact proposed message and require allowlisting', () => {
  const focusItem = {
    ...item('channel:123', 2),
    actions: [focusAction('reply', 'Reply', 'confirmation')],
  } satisfies FocusItem
  const proposal = deterministicProposal(focusItem, 'Reply: Shipping is approved for Tuesday.')
  assert.deepEqual(proposal?.payload, { message: 'Shipping is approved for Tuesday.' })
  assert.equal(
    validateCommandObject({ message: 'Ready', actionId: 'reply', payload: { message: 'Hello' } }, new Set()),
    null,
  )
})

test('a bad confirmation token does not invalidate a still-current proposal', () => {
  const now = Date.parse('2026-08-03T12:00:00.000Z')
  assert.equal(confirmationMissInvalidates({
    status: 'proposed',
    expiresAt: new Date(now + 60_000),
    storedFingerprint: 'same',
    currentFingerprint: 'same',
    now,
  }), false)
  assert.equal(confirmationMissInvalidates({
    status: 'proposed',
    expiresAt: new Date(now - 1),
    storedFingerprint: 'same',
    currentFingerprint: 'same',
    now,
  }), true)
  assert.equal(confirmationMissInvalidates({
    status: 'proposed',
    expiresAt: new Date(now + 60_000),
    storedFingerprint: 'old',
    currentFingerprint: 'new',
    now,
  }), true)
})
