import { describe, expect, it } from 'vitest'

import { attachmentHref } from '@/lib/attachment-token'
import { renderMarkdown } from './markdown'

// The read side of the attachment chip: the token the editor serializes
// (`[name](attachment://<uuid>?s=<size>&m=<enc(mime)>)`, grammar in
// @/lib/attachment-token) must come out of the unified pipeline as a
// download chip — not a dead link, not a raw scheme, and never by
// breaking the link/image rules that were there before it.

const UUID = 'e3b0c442-98fc-4221-a441-4a05b3f0e4d2'
const token = (name: string) =>
  `[${name}](${attachmentHref({ id: UUID, mime: 'application/pdf', size: 2048 })})`

describe('renderMarkdown: attachment chips', () => {
  it('renders the token as a download anchor to /api/uploads with name and human size', () => {
    const html = renderMarkdown(token('report.pdf'))
    expect(html).toContain(`href="/api/uploads/${UUID}"`)
    expect(html).toContain('download')
    expect(html).toContain('>report.pdf<')
    expect(html).toContain('>2 KB<')
    // The chip's visual, from the chat surface's file chip (MessageAttachments).
    expect(html).toContain('inline-flex')
    expect(html).toContain('rounded-md border border-line bg-raised')
  })

  it('drops the raw attachment: scheme from the wire — never an attachment:// href', () => {
    const html = renderMarkdown(token('report.pdf'))
    expect(html).not.toContain('attachment://')
  })

  it('renders a chip inline inside a list item', () => {
    const html = renderMarkdown(`- see ${token('notes.txt')} first`)
    expect(html).toContain('<li class=')
    expect(html).toContain(`href="/api/uploads/${UUID}"`)
  })

  it('renders per-instance chips for the same file attached twice (two ids)', () => {
    const other = '9a1f2b3c-1111-2222-3333-444455556666'
    const html = renderMarkdown(
      `${token('a.pdf')} and [b.pdf](${attachmentHref({ id: other, mime: 'application/pdf', size: 5 })})`,
    )
    expect(html).toContain(`href="/api/uploads/${UUID}"`)
    expect(html).toContain(`href="/api/uploads/${other}"`)
    expect(html).toContain('>5 B<')
  })

  it('falls through a regex miss as an ordinary link with a blanked href', () => {
    // Not our grammar (no query) — must render as the plain-link path,
    // whose safeUrl gate blanks the unknown scheme. No chip classes.
    const html = renderMarkdown('[hand edit](attachment://not-our-grammar)')
    expect(html).toContain('<a')
    expect(html).not.toContain('attachment://')
    expect(html).not.toContain('inline-flex')
  })

  it('still renders ordinary links and images exactly as before', () => {
    const links = renderMarkdown('[site](https://example.com)')
    expect(links).toContain('href="https://example.com"')
    expect(links).toContain('text-accent')
    expect(links).not.toContain('inline-flex')

    const img = renderMarkdown('![alt](/api/uploads/9a1f2b3c-1111-2222-3333-444455556666)')
    expect(img).toContain('<img')
    expect(img).toContain(`src="/api/uploads/9a1f2b3c-1111-2222-3333-444455556666"`)
  })

  it('still neuters javascript: urls', () => {
    const html = renderMarkdown('[bad](javascript:alert(1))')
    expect(html).not.toContain('javascript:')
  })
})
