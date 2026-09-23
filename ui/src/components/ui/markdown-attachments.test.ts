import { describe, expect, it } from 'vitest'
import { renderMarkdown } from '@/components/ui/markdown'

// TALA-2: the inline-attachment placeholder is plain markdown in the doc body
// — `![name|size](upload:<id>)` for images, `[name|size](upload:<id>)` for
// everything else. These pins cover the read-side contract: both spellings
// render the hydratable placeholder span (name + size intact), the scheme
// survives a serialize → parse round trip, and re-ordering the body neither
// orphans nor duplicates the reference.

const chip = (html: string) => html.match(/<span data-kb-upload="([^"]*)"[^>]*>/)?.[1]

describe('inline KB attachment placeholders', () => {
  it('renders a non-image chip with filename and size', () => {
    const html = renderMarkdown('[report.pdf|12.3 KB](upload:abc-123)')
    expect(html).toContain('data-kb-upload="abc-123"')
    expect(html).toContain('data-filename="report.pdf"')
    expect(html).toContain('data-size="12.3 KB"')
    // Not a navigable link — the chip opens the in-app viewer.
    expect(html).not.toContain('<a ')
  })

  it('renders an image placeholder that Markdown.svelte hydrates', () => {
    const html = renderMarkdown('![photo.png|1.2 KB](upload:id-42)')
    expect(chip(html)).toBe('id-42')
    expect(html).toContain('data-filename="photo.png"')
  })

  it('keeps a filename containing pipes intact (last pipe is the separator)', () => {
    const html = renderMarkdown('[draft|v2.pdf|3 KB](upload:x1)')
    expect(html).toContain('data-filename="draft|v2.pdf"')
    expect(html).toContain('data-size="3 KB"')
  })

  it('round-trips through the editor spelling (tiptap image node markdown)', () => {
    // tiptap serializes an image node as ![alt](src); the read side must
    // accept exactly that shape back. (Raw HTML never renders — the pipeline
    // drops it by design — so the editor spelling IS the markdown form.)
    const html = renderMarkdown('![shot.png|8 KB](upload:abc)')
    expect(html).toContain('data-kb-upload="abc"')
    expect(html).toContain('data-filename="shot.png"')
    expect(html).toContain('data-size="8 KB"')
  })

  it('leaves ordinary markdown images and links alone', () => {
    const html = renderMarkdown('![alt](/api/uploads/some-id) and [link](https://example.com)')
    expect(html).toContain('src="/api/uploads/some-id"')
    expect(html).toContain('href="https://example.com"')
    expect(html).not.toContain('data-kb-upload')
  })

  it('re-ordering the body keeps one chip per reference (no duplication)', () => {
    const body = 'intro\n\n[a.pdf|1 KB](upload:id-a)\n\noutro'
    const reordered = 'outro\n\n' + body.split('\n\n')[1] + '\n\nintro'
    const a = chip(renderMarkdown(body))
    const b = chip(renderMarkdown(reordered))
    expect(a).toBe('id-a')
    expect(b).toBe('id-a')
    const html = renderMarkdown(reordered)
    expect((html.match(/data-kb-upload/g) ?? []).length).toBe(1)
  })
})
