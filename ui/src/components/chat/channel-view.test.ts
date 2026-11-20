import { describe, expect, it, vi } from 'vitest'

// channels.svelte.ts reads through svelte-query, whose barrel pulls .svelte
// files the node environment cannot transform; nothing here queries.
vi.mock('@tanstack/svelte-query', () => ({ createQuery: vi.fn(), useQueryClient: vi.fn() }))

import type { ChannelMessage } from '@/lib/channels.svelte'
import { actorLabel, authorTypeOf, importedAuthor, rowMenuEntries, threadAvatars, type MessageCtx } from './channel-view'

const ctx = (over: Partial<MessageCtx> = {}): MessageCtx => ({
  channelId: 'c1',
  me: 'dana@example.invalid',
  isChannelOwner: false,
  labelFor: (model) => `agent:${model}`,
  // The directory knows a teammate whose NAME is the imported author's — the
  // collision an imported comment must never resolve into.
  userLabel: (author) => (author === 'dana@example.invalid' ? 'Dana Reyes' : author.split('@')[0] || author),
  pictureFor: (_a, type) => (type === 'user' ? 'https://example.invalid/photo.png' : null),
  selfMentions: [],
  ...over,
})

const msg = (over: Partial<ChannelMessage>): ChannelMessage => ({
  id: 'm1',
  seq: 1,
  authorType: 'user',
  author: 'dana@example.invalid',
  content: 'hello',
  status: 'complete',
  createdAt: '2026-11-20T12:00:00Z',
  ...over,
})

describe('imported authors (a tracker sync’s comments)', () => {
  it('splits the label into the name and the tracker, and keeps a bare label whole', () => {
    expect(importedAuthor('Dana Reyes (ClickUp)')).toEqual({ name: 'Dana Reyes', source: 'ClickUp' })
    expect(importedAuthor('Dana (Ops) Reyes (ClickUp)')).toEqual({ name: 'Dana (Ops) Reyes', source: 'ClickUp' })
    expect(importedAuthor('Someone')).toEqual({ name: 'Someone', source: null })
    expect(importedAuthor('  ')).toEqual({ name: 'Imported', source: null })
  })

  it('is labelled from its own label, never through the user directory', () => {
    const c = ctx({ userLabel: () => 'RESOLVED AS A TEAMMATE' })
    expect(actorLabel(c, 'Dana Reyes (ClickUp)', 'imported')).toBe('Dana Reyes')
    expect(actorLabel(c, 'dana@example.invalid', 'user')).toBe('RESOLVED AS A TEAMMATE')
    expect(actorLabel(c, 'claude', 'agent')).toBe('agent:claude')
  })

  it('a thread’s participant list tells people, agents and imported labels apart', () => {
    expect(authorTypeOf('dana@example.invalid')).toBe('user')
    expect(authorTypeOf('Dana Reyes (ClickUp)')).toBe('imported')
    expect(authorTypeOf('anthropic/claude-sonnet')).toBe('agent')
    const rollup = threadAvatars(ctx(), ['Dana Reyes (ClickUp)', 'dana@example.invalid'])
    expect(rollup).toEqual([
      { key: 'Dana Reyes (ClickUp)', name: 'Dana Reyes', src: null },
      { key: 'dana@example.invalid', name: 'Dana Reyes', src: 'https://example.invalid/photo.png' },
    ])
  })

  it('is never the viewer’s own: no delete offered unless the viewer owns the room', () => {
    const imported = msg({ authorType: 'imported', author: 'Dana Reyes (ClickUp)' })
    const labels = (entries: ReturnType<typeof rowMenuEntries>) =>
      entries.flatMap((e) => (typeof e === 'object' && 'label' in e ? [e.label] : []))
    expect(labels(rowMenuEntries(imported, ctx(), () => {}))).not.toContain('Delete message')
    // Even when the label happens to equal the viewer's identity string.
    expect(labels(rowMenuEntries({ ...imported, author: 'dana@example.invalid' }, ctx(), () => {}))).not.toContain('Delete message')
    expect(labels(rowMenuEntries(imported, ctx({ isChannelOwner: true }), () => {}))).toContain('Delete message')
    expect(labels(rowMenuEntries(msg({}), ctx(), () => {}))).toContain('Delete message')
  })
})
