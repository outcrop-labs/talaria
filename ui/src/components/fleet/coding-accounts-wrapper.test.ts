// The coding-accounts panel sits in the agent Overview's Behavior group,
// whose children each draw a hairline above them. With the feature off its
// route 404s (data `null`) and the panel must draw NOTHING, its padded
// wrapper included, or the group shows an empty band; loading still draws
// inside the caller's padding.
import { render } from 'svelte/server'
import { describe, expect, it, vi } from 'vitest'

const state = vi.hoisted(() => ({ data: undefined as unknown, isPending: true }))
vi.mock('@tanstack/svelte-query', () => ({
  createQuery: vi.fn(() => ({
    get data() {
      return state.data
    },
    get isPending() {
      return state.isPending
    },
    isError: false,
    fetchStatus: 'fetching',
    refetch: vi.fn(),
  })),
  useQueryClient: vi.fn(),
}))
vi.mock('@/components/models/model-names.svelte', () => ({ useModelNames: () => () => undefined }))

import CodingAccounts from './CodingAccounts.svelte'

describe('CodingAccounts', () => {
  it('draws nothing, not even its padded wrapper, when the feature is off (a 404 is null)', () => {
    state.data = null
    state.isPending = false
    const body = render(CodingAccounts, { props: { agentId: 'a1', class: 'px-5 py-4' } }).body
    expect(body).not.toContain('px-5 py-4')
    expect(body).not.toContain('Coding accounts')
  })

  it('draws its loading state inside the caller padding while the read is in flight', () => {
    state.data = undefined
    state.isPending = true
    const body = render(CodingAccounts, { props: { agentId: 'a1', class: 'px-5 py-4' } }).body
    expect(body).toContain('class="px-5 py-4"')
  })
})
