import { describe, expect, it } from 'vitest'
import { EFFORTS, humanAssigneeId, isHumanAssignee, OFF_BOARD_STATUSES, PRIORITIES, STATUS_LABEL, TASK_STATUSES } from '@/lib/task-const'

describe('assignee encoding', () => {
  it('distinguishes a human from an agent model id', () => {
    expect(isHumanAssignee('user:3f2504e0-4f89-11d3-9a0c-0305e82c3301')).toBe(true)
    expect(isHumanAssignee('claude-opus-5')).toBe(false)
    expect(isHumanAssignee('')).toBe(false)
    // A model id that merely contains "user:" is not a human.
    expect(isHumanAssignee('agent/user:thing')).toBe(false)
  })

  it('strips the prefix to recover the user id', () => {
    expect(humanAssigneeId('user:3f2504e0-4f89-11d3-9a0c-0305e82c3301')).toBe('3f2504e0-4f89-11d3-9a0c-0305e82c3301')
  })
})

describe('constants', () => {
  it('has a label for every status, including the off-board ones', () => {
    // Imported, not spelled out: a copy of this list that drifts leaves tickets
    // in a status no view draws. check-invariants.mjs enforces that, and caught
    // this very line when the test was ported.
    for (const s of [...TASK_STATUSES, ...OFF_BOARD_STATUSES]) expect(STATUS_LABEL[s]).toBeTruthy()
  })

  it('keeps the ordered scales ordered', () => {
    expect(PRIORITIES).toEqual(['low', 'medium', 'high', 'urgent'])
    expect(EFFORTS).toEqual(['xs', 's', 'm', 'l', 'xl'])
  })
})
