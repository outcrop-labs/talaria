import { describe, expect, it } from 'vitest'
import { summarize } from './change-summary'

describe('summarize', () => {
  it('counts additions and removals', () => {
    expect(summarize('a\nb\nc', 'a\nB\nc\nd')).toEqual({ adds: 2, dels: 1, text: '2 added · 1 removed lines', oversized: false })
  })

  it('singularizes when only one line changed', () => {
    expect(summarize('a\nb', 'a\nB').text).toBe('1 added · 1 removed lines')
    expect(summarize('a\nb', 'a\nb\nc').text).toBe('1 added · 0 removed line')
  })

  it('reports the no-op honestly', () => {
    expect(summarize('same', 'same').text).toBe('no text changes')
  })

  it('degrades to "changed" when the document outgrows the client diff', () => {
    const big = Array.from({ length: 2000 }, (_, i) => `line ${i}`).join('\n')
    expect(summarize(big, `${big}\nmore`).oversized).toBe(true)
    expect(summarize(big, `${big}\nmore`).text).toBe('changed')
  })
})
