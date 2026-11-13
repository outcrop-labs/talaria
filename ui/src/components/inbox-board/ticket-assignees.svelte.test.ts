// @vitest-environment runes
import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, expect, test, vi } from 'vitest'
import type { InboxPerson } from '@/lib/inbox-board'
import type { HomeSummary } from '@/routes/app/home/home'

const h = vi.hoisted(() => ({
  client: null as unknown,
  answers: new Map<string, (body: unknown[]) => void>(),
}))

vi.mock('@/lib/fetch-json', () => ({
  getList: vi.fn(
    (url: string) =>
      new Promise((resolve) => {
        h.answers.set(url, resolve)
      }),
  ),
}))

vi.mock('@tanstack/svelte-query', async (importOriginal) => {
  const real = await importOriginal<typeof import('@tanstack/svelte-query')>()
  const createQueries = ((options: Parameters<typeof real.createQueries>[0], client?: () => QueryClient) =>
    real.createQueries(options, client ?? (() => h.client as QueryClient))) as typeof real.createQueries
  return { ...real, createQueries }
})

import { useTicketAssignees } from './ticket-assignees.svelte'

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

function answer(url: string, body: unknown[]): void {
  const resolve = h.answers.get(url)
  if (!resolve) throw new Error(`no read in flight for ${url}`)
  h.answers.delete(url)
  resolve(body)
}

const item = (id: string, queue: string) => ({ id, boardId: 'b-help', board: 'Helpdesk', ticketRef: id, title: id, status: queue, updatedAt: '2026-11-12T09:00:00Z' })

const QUEUES = {
  triage: { count: 1, items: [item('t-triage', 'inbox')] },
  review: { count: 0, items: [] },
  blocked: { count: 1, items: [item('t-blocked', 'blocked')] },
} as unknown as HomeSummary['queues']

afterEach(() => {
  h.answers.clear()
  ;(h.client as QueryClient | null)?.clear()
})

test('owners appear once the board reads answer, though the queues arrived after mount', async () => {
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  let queues = $state<HomeSummary['queues'] | null>(null)
  const seen = { blocked: null as readonly InboxPerson[] | null, triage: null as readonly InboxPerson[] | null }
  const stop = $effect.root(() => {
    const lookup = useTicketAssignees(
      () => queues,
      () => [{ id: 'forge', label: 'Forge' }],
    )
    $effect(() => {
      seen.blocked = lookup('t-blocked')
      seen.triage = lookup('t-triage')
    })
  })
  await settle()
  expect(seen.blocked).toBeNull()

  queues = QUEUES
  await settle()
  answer('/api/boards/b-help/tasks', [
    { id: 't-blocked', assignees: ['forge'] },
    { id: 't-triage', assignees: [] },
  ])
  answer('/api/boards/b-help/members', [])
  await settle()

  expect(seen.blocked).toEqual([{ name: 'Forge', agent: true }])
  expect(seen.triage).toEqual([])
  stop()
})
