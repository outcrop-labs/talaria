import { describe, expect, it } from 'vitest'
import { renderMarkdown } from '@/components/ui/markdown'
import {
  chipsBesideProse,
  classifyPlatformHref,
  findPlatformLinks,
  invokeText,
  toolsUnlockedBy,
  type ChatChip,
} from '@/lib/chips'

describe('classifyPlatformHref', () => {
  it('recognizes the platform entities agents share', () => {
    expect(classifyPlatformHref('/boards/board-1')).toMatchObject({ entity: 'board', id: 'board-1' })
    expect(classifyPlatformHref('/boards/b-1/t-2')).toMatchObject({
      entity: 'ticket',
      id: 't-2',
      boardId: 'b-1',
      href: '/boards/b-1/t-2',
    })
    expect(classifyPlatformHref('/knowledge?doc=doc-9')).toMatchObject({ entity: 'kb', id: 'doc-9' })
    expect(classifyPlatformHref('/knowledge/space-1/doc-9')).toMatchObject({ entity: 'kb', id: 'doc-9' })
    expect(classifyPlatformHref('/artifacts?a=file-3')).toMatchObject({ entity: 'document', id: 'file-3' })
    expect(classifyPlatformHref('/artifacts/my?a=file-3')).toMatchObject({ entity: 'document', href: '/artifacts?a=file-3' })
    expect(classifyPlatformHref('/comms/channel/chan-1')).toMatchObject({ entity: 'channel', id: 'chan-1' })
  })

  it('strips the instance origin and still recognizes the path', () => {
    expect(classifyPlatformHref('https://talaria.example/boards/b-1/t-2')).toMatchObject({
      entity: 'ticket',
      href: '/boards/b-1/t-2',
    })
  })

  it('leaves unrecognized URLs alone', () => {
    expect(classifyPlatformHref('https://example.com/pricing')).toBeNull()
    expect(classifyPlatformHref('/admin?tab=people')).toBeNull()
    expect(classifyPlatformHref('/boards')).toBeNull()
    expect(classifyPlatformHref('/knowledge')).toBeNull()
    expect(classifyPlatformHref('mailto:a@b.c')).toBeNull()
    expect(classifyPlatformHref('javascript:alert(1)')).toBeNull()
  })
})

describe('findPlatformLinks', () => {
  it('finds a bare path in a sentence and ignores a path glued to a word', () => {
    const hits = findPlatformLinks('Filed it at /boards/b-1/t-2 for review.')
    expect(hits).toHaveLength(1)
    expect(hits[0]?.link.entity).toBe('ticket')
    expect(findPlatformLinks('see/boards/b-1/t-2')).toHaveLength(0)
  })
})

describe('renderMarkdown chips', () => {
  it('renders a recognized platform link as a chip placeholder, not a raw anchor', () => {
    const html = renderMarkdown('See [the ticket](/boards/b-1/t-2) and https://example.com/docs')
    expect(html).toContain('data-platform-chip')
    expect(html).toContain('data-href="/boards/b-1/t-2"')
    expect(html).toContain('data-entity="ticket"')
    expect(html).toContain('href="https://example.com/docs"')
    expect(html).not.toContain('<a href="/boards/b-1/t-2"')
  })

  it('chips a bare platform path the agent pasted as text', () => {
    const html = renderMarkdown('The board is /boards/board-1.')
    expect(html).toContain('data-platform-chip')
    expect(html).toContain('data-entity="board"')
  })

  it('keeps an unrecognized URL as a normal link', () => {
    const html = renderMarkdown('https://example.com/not-ours')
    expect(html).not.toContain('data-platform-chip')
    expect(html).toContain('href="https://example.com/not-ours"')
  })
})

describe('chipsBesideProse', () => {
  const link: ChatChip = { id: 'l', kind: 'link', href: '/boards/b-1', entity: 'board', title: 'Ops' }
  const approval: ChatChip = { id: 'a', kind: 'approval', actionId: 'p1', summary: 'Send email', status: 'pending' }

  it('drops a link chip the prose already paints and keeps the approval', () => {
    expect(chipsBesideProse([link, approval], 'open /boards/b-1')).toEqual([approval])
  })
})

describe('unlock and invoke', () => {
  it('names the tools an approval unlocks', () => {
    expect(toolsUnlockedBy('gmail_send')).toEqual(['draft_email'])
    expect(toolsUnlockedBy('ticket_move')).toEqual(['triage_ticket'])
    expect(toolsUnlockedBy('unknown')).toEqual([])
  })

  it('builds the run sentence from filled inputs only', () => {
    expect(invokeText('draft_email', { to: 'a@b.c', subject: '', body: 'hi' })).toContain('"to": "a@b.c"')
    expect(invokeText('draft_email', { to: 'a@b.c', subject: '', body: 'hi' })).not.toContain('subject')
  })
})
