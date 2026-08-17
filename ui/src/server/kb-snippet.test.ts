// The KB search snippet is the one string in this codebase that is rendered as
// raw HTML (`{@html h.snippet}` in KbSearch.svelte) and built from a document
// body anyone — including a prompt-injected agent — can write. `ts_headline`
// does not escape what it excerpts, so without safeSnippet the audit's live
// payload ran script in any searcher's browser, admins included.
//
// The design under test: highlight with STX-delimited sentinels no real
// document can carry, escape EVERYTHING, then turn only the sentinels into <b>.
import { describe, it, expect } from 'vitest'
import { safeSnippet } from './kb'

const HL_START = '\u0002hl\u0002'
const HL_STOP = '\u0002/hl\u0002'

describe('safeSnippet', () => {
  it('escapes the audit’s verified payload', () => {
    const out = safeSnippet('harmless intro <img src=x onerror=alert(1)> budget text')
    expect(out).not.toContain('<img')
    expect(out).toContain('&lt;img')
    expect(out).toContain('onerror=alert(1)&gt;') // inert text, not an attribute
  })

  it('escapes a script tag', () => {
    expect(safeSnippet('<script>alert(1)</script>')).toBe(
      '&lt;script&gt;alert(1)&lt;/script&gt;',
    )
  })

  it('escapes quotes, so a payload cannot break out of an attribute', () => {
    expect(safeSnippet(`" onmouseover="alert(1)`)).toBe('&quot; onmouseover=&quot;alert(1)')
    expect(safeSnippet("' onfocus='alert(1)")).toBe('&#39; onfocus=&#39;alert(1)')
  })

  it('escapes ampersands first, so escaping is not double-applied or undone', () => {
    // A body containing a literal `&lt;` must survive as visible text, not
    // decay into a real `<` on the client.
    expect(safeSnippet('&lt;script&gt;')).toBe('&amp;lt;script&amp;gt;')
  })

  it('turns the sentinels into <b>, and only the sentinels', () => {
    expect(safeSnippet(`the ${HL_START}budget${HL_STOP} doc`)).toBe('the <b>budget</b> doc')
  })

  it('still highlights when the surrounding text is hostile', () => {
    const out = safeSnippet(`<img src=x> ${HL_START}budget${HL_STOP} </script>`)
    expect(out).toBe('&lt;img src=x&gt; <b>budget</b> &lt;/script&gt;')
  })

  it('does not let a document forge a highlight by typing the sentinel text', () => {
    // The literal words are harmless; only the STX-delimited form is a sentinel.
    const out = safeSnippet('hl /hl <hl> </hl>')
    expect(out).not.toContain('<b>')
    expect(out).toBe('hl /hl &lt;hl&gt; &lt;/hl&gt;')
  })

  it('handles a null snippet', () => {
    expect(safeSnippet(null)).toBe('')
  })

  it('leaves ordinary prose untouched', () => {
    expect(safeSnippet('Q3 budget planning notes')).toBe('Q3 budget planning notes')
  })
})
