import { describe, expect, it } from 'vitest'
import { cardKindFor, sourceLabel, subagentGoal, subagentTone, summarizeChange, toolRowLabel } from './tool-cards'
import type { ChatChip, DocChange } from '@/lib/chips'

const chip = (over: Partial<ChatChip>): ChatChip => ({ id: 'c', kind: 'link', ...over })

describe('cardKindFor', () => {
  it('names the card each kind renders as', () => {
    expect(cardKindFor(chip({ kind: 'image', uploadId: 'up-1', label: 'shot.png' }))).toBe('image')
    expect(cardKindFor(chip({ kind: 'subagent', goal: 'read the spec', status: 'running' }))).toBe('subagent')
    expect(cardKindFor(chip({ kind: 'source', href: 'https://example.com/a', label: 'A' }))).toBe('source')
    expect(
      cardKindFor(
        chip({ kind: 'link', entity: 'document', href: '/artifacts?a=d1', changes: [{ label: 'Intro', added: 2, removed: 1 }] }),
      ),
    ).toBe('document')
  })

  it('falls back to plain for every chip whose kind already decides its shape', () => {
    expect(cardKindFor(chip({ kind: 'tool', tool: 'create_ticket' }))).toBe('plain')
    expect(cardKindFor(chip({ kind: 'approval', actionId: 'p1' }))).toBe('plain')
    expect(cardKindFor(chip({ kind: 'notice', label: 'sent mid-turn' }))).toBe('plain')
    expect(cardKindFor(chip({ kind: 'link', entity: 'board', href: '/boards/b-1' }))).toBe('plain')
  })

  it('routes a source chip with no href to plain, which ChatChips must still render', () => {
    // There is nothing to open, so it cannot be the source pill — and the
    // if-chain's final {:else} is what keeps it on screen as the muted label
    // pill. Without that branch a `plain` chip no earlier branch claimed
    // rendered NOTHING: a row the server persisted, silently absent.
    expect(cardKindFor(chip({ kind: 'source', label: 'A paper with no link' }))).toBe('plain')
  })

  it('falls back to plain when the field the card draws from is missing', () => {
    // No bytes, no thumbnail; no href, nothing to open; no changes, nothing to
    // list — each of these rendered as its card would be an empty frame.
    expect(cardKindFor(chip({ kind: 'image', label: 'shot.png' }))).toBe('plain')
    expect(cardKindFor(chip({ kind: 'source', label: 'A paper' }))).toBe('plain')
    expect(cardKindFor(chip({ kind: 'link', entity: 'document', href: '/artifacts?a=d1', changes: [] }))).toBe('plain')
    expect(cardKindFor(chip({ kind: 'link', entity: 'document', changes: [{ label: 'x', added: 1, removed: 0 }] }))).toBe(
      'plain',
    )
  })
})

describe('summarizeChange', () => {
  it('reads label · N added · M removed lines', () => {
    expect(summarizeChange({ label: 'Rollout', added: 3, removed: 1 })).toBe('Rollout · 3 added · 1 removed lines')
    expect(summarizeChange({ label: 'Appended', added: 4, removed: 0 })).toBe('Appended · 4 added · 0 removed lines')
  })

  it('names the UNIT, because a bare +1 −1 reads as characters', () => {
    // The counts are line deltas: a one-word swap is one line out, one line
    // in. Said as `+1 −1` that looked like a one-character edit to half the
    // people who saw it, and like a 1-of-each rewrite to the rest.
    expect(summarizeChange({ label: '"ships Friday"', added: 1, removed: 1 })).toBe(
      '"ships Friday" · 1 added · 1 removed lines',
    )
  })

  it('keeps the singular exactly where agent-refine keeps it', () => {
    // `summarize` in lib/agent-refine.ts says "line" only for a pure
    // one-line addition. The refine notice and this card report the same fact
    // about the same document, so they say it in the same words.
    expect(summarizeChange({ label: 'Appended a line', added: 1, removed: 0 })).toBe(
      'Appended a line · 1 added · 0 removed line',
    )
    expect(summarizeChange({ label: 'Took a line out', added: 0, removed: 1 })).toBe(
      'Took a line out · 0 added · 1 removed lines',
    )
  })
})

describe('sourceLabel', () => {
  it('prefers the page title', () => {
    expect(sourceLabel({ label: 'Retry budgets', href: 'https://example.com/retry' })).toBe('Retry budgets')
  })

  it('falls back to the host, then to the href itself', () => {
    expect(sourceLabel({ href: 'https://docs.example.com/a/b?c=1' })).toBe('docs.example.com')
    expect(sourceLabel({ label: '   ', href: 'https://docs.example.com/a' })).toBe('docs.example.com')
    // Not parseable as a URL: show what we were given rather than nothing.
    expect(sourceLabel({ href: 'not a url' })).toBe('not a url')
    expect(sourceLabel({})).toBe('')
  })

  it('spells the host the way SourcesList spells it — no leading www.', () => {
    // The same page, read by the same search, must not be "www.nytimes.com"
    // in the chat row and "nytimes.com" in the sources list beside it.
    expect(sourceLabel({ href: 'https://www.nytimes.com/2026/01/01/a.html' })).toBe('nytimes.com')
    // `hostname`, not `host`: a port is machine detail, not a name a reader
    // recognises, and `www2.` is a real subdomain rather than the prefix.
    expect(sourceLabel({ href: 'http://docs.example.com:8080/a' })).toBe('docs.example.com')
    expect(sourceLabel({ href: 'https://www2.example.com/a' })).toBe('www2.example.com')
  })
})

describe('subagentGoal', () => {
  it('drops the goal the producer already wrote into the label', () => {
    // The label IS "<role> · <goal>", so the second line was the same
    // sentence a second time.
    const goal = 'read the retry spec and report what the budget is'
    expect(subagentGoal({ label: `Researcher · ${goal}`, goal })).toBeNull()
  })

  it('drops it even when the label clipped it', () => {
    // The label's copy is cut at 80 chars, which is why the comparison is on
    // a prefix: the goal is the same goal however the label ended.
    const goal = `${'read the spec and then '.repeat(6)}report`
    expect(subagentGoal({ label: `Researcher · ${goal.slice(0, 80)}`, goal })).toBeNull()
  })

  it('keeps a goal the label does not say', () => {
    expect(subagentGoal({ label: 'Researcher', goal: 'read the spec' })).toBe('read the spec')
    expect(subagentGoal({ goal: 'read the spec' })).toBe('read the spec')
  })

  it('is null when there is no goal at all', () => {
    expect(subagentGoal({ label: 'Researcher' })).toBeNull()
    expect(subagentGoal({ label: 'Researcher', goal: '   ' })).toBeNull()
  })
})

describe('subagentTone', () => {
  it('maps the child statuses onto dot tones', () => {
    expect(subagentTone('running')).toBe('accent')
    expect(subagentTone('done')).toBe('ok')
    expect(subagentTone('error')).toBe('danger')
  })

  it('is idle for a status the child has not reported', () => {
    expect(subagentTone(undefined)).toBe('idle')
    // 'pending' belongs to approvals; a delegate chip never carries it, and
    // guessing a health tone for it would be inventing one.
    expect(subagentTone('pending')).toBe('idle')
  })
})

describe('the DocChange contract', () => {
  it('carries a preview only where a Replace needs one', () => {
    const changes: DocChange[] = [
      { label: 'Replaced the rollout note', added: 1, removed: 1, preview: { old: 'ships Friday', new: 'ships Monday' } },
      { label: 'Appended a section', added: 6, removed: 0 },
    ]
    expect(changes[0]?.preview?.new).toBe('ships Monday')
    expect(changes[1]?.preview).toBeUndefined()
  })
})

describe('toolRowLabel', () => {
  it('shows a short preview and hides a paragraph', () => {
    expect(toolRowLabel('ls -la')).toBe('ls -la')
    expect(toolRowLabel('  padded  ')).toBe('padded')
    expect(toolRowLabel(undefined)).toBe('')
    expect(toolRowLabel('x'.repeat(121))).toBe('')
    expect(toolRowLabel('x'.repeat(120))).toHaveLength(120)
  })
})
