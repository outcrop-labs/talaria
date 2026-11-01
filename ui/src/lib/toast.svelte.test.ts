// @vitest-environment runes
//
// The toast store's keys: what read-sync (notify-readsync.ts) dismisses by.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { dismissKeyedToasts, dismissToast, dismissToastsByKey, pushToast, toastKeys, toastList } from './toast.svelte'

describe('toast keys — a read elsewhere takes the toast down (I-16)', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => {
    for (const t of [...toastList()]) dismissToast(t.id)
    vi.useRealTimers()
  })

  it('dismisses by key, leaving the others', () => {
    pushToast({ title: 'a', key: 'row-a' })
    pushToast({ title: 'b', key: 'row-b' })
    pushToast({ title: 'saved' })
    expect(dismissToastsByKey(['row-a'])).toBe(1)
    expect(toastList().map((t) => t.title)).toEqual(['b', 'saved'])
    expect(toastKeys()).toEqual(['row-b'])
  })

  it('a mark-all dismisses every notification toast, never an app toast', () => {
    pushToast({ title: 'a', key: 'row-a' })
    pushToast({ title: 'error', tone: 'danger' })
    expect(dismissKeyedToasts()).toBe(1)
    expect(toastList().map((t) => t.title)).toEqual(['error'])
  })

  it('a re-alert of the same row replaces its toast instead of stacking', () => {
    pushToast({ title: 'dm 1', key: 'row-a' })
    pushToast({ title: 'dm 2', key: 'row-a' })
    expect(toastList().map((t) => t.title)).toEqual(['dm 2'])
  })
})
