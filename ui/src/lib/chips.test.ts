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

  it('keeps an edited-document chip even when the prose paints its URL', () => {
    // The card says what the edit moved; the prose's pill says only that the
    // document exists. Dropping it would take the change list with it.
    const edited: ChatChip = {
      id: 'd',
      kind: 'link',
      href: '/artifacts?a=file-3',
      entity: 'document',
      title: 'Rollout',
      changes: [{ label: 'Appended a section', added: 6, removed: 0 }],
    }
    expect(chipsBesideProse([edited], 'updated /artifacts?a=file-3')).toEqual([edited])
  })

  it('keeps a source chip, which is off-platform and never painted by the markdown', () => {
    const source: ChatChip = {
      id: 'source:9f2c1a',
      kind: 'source',
      label: 'Retry budgets',
      href: 'https://example.com/retry',
      detail: 'A budget caps the retries a request may spend.',
    }
    expect(chipsBesideProse([source], 'see https://example.com/retry')).toEqual([source])
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

describe('the agent plan chip', () => {
  const plan: ChatChip = {
    id: 'todo',
    kind: 'todo',
    label: '1/3 done · now: draft the tickets',
    items: [
      { text: 'read the retry spec', status: 'done' },
      { text: 'draft the tickets', status: 'active' },
      { text: 'update the doc', status: 'pending' },
    ],
  }

  it('survives the prose filter, because a plan is never painted by the text', () => {
    // Only a link whose href the prose already carries is dropped; a plan has
    // no href and the agent's words never contain the timeline.
    expect(chipsBesideProse([plan], 'I will start on the tickets now.')).toEqual([plan])
  })

  it('carries the whole list so an old chip still reads as the plan of its moment', () => {
    // The stream is read top-to-bottom. A delta would need replaying forwards
    // to mean anything, and a reader scrolling back is not doing that.
    expect(plan.items).toHaveLength(3)
    expect(plan.items?.filter((i) => i.status === 'active')).toHaveLength(1)
  })
})

describe('the ask chip', () => {
  const ask: ChatChip = {
    id: 'ask:9f2',
    kind: 'question',
    label: 'Which board should these tickets land on?',
    description: 'Both have an inbox column.',
    options: [
      { label: 'Platform', detail: 'the infra board' },
      { label: 'Growth', detail: null },
    ],
  }

  it('survives the prose filter so a question cannot scroll past unanswered', () => {
    expect(chipsBesideProse([ask], 'I need to know which board before I file these.')).toEqual([ask])
  })

  it('carries the answers as their own words, which is what gets sent', () => {
    // Clicking sends the label through the composer's send path, so the reply
    // is an ordinary message and there is no second record of the answer.
    expect(ask.options?.map((o) => o.label)).toEqual(['Platform', 'Growth'])
    // A missing detail is null rather than absent — the server emits it.
    expect(ask.options?.[1]?.detail).toBeNull()
  })
})

describe('the notice chip', () => {
  const notice: ChatChip = {
    id: 'steer:m-7',
    kind: 'notice',
    label: 'sent mid-turn',
    detail: 'The agent took this into the reply it was already writing.',
  }

  it('survives the prose filter, because a notice is never painted by the words', () => {
    // Same reason the plan and the question survive: only a link whose href
    // the prose already carries is dropped.
    expect(chipsBesideProse([notice], 'actually, target rc')).toEqual([notice])
  })

  it('says what happened and offers nothing to do about it', () => {
    // The row renders label + title and no controls: a notice with an action
    // would be an approval, which is a different chip with a different story.
    expect(notice.label).toBe('sent mid-turn')
    expect(notice.detail).toContain('already writing')
    expect(notice.actionId).toBeUndefined()
    expect(notice.options).toBeUndefined()
  })
})
