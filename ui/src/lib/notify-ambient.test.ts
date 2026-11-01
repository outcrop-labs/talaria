import { describe, expect, it, vi } from 'vitest'
import { startAmbient, titleWith, type AmbientDoc } from './notify-ambient'

class FakeLink {
  attrs = new Map<string, string>()
  constructor(href: string, type?: string) {
    this.attrs.set('rel', 'icon')
    this.attrs.set('href', href)
    if (type) this.attrs.set('type', type)
  }
  getAttribute(k: string) {
    return this.attrs.get(k) ?? null
  }
  setAttribute(k: string, v: string) {
    this.attrs.set(k, v)
  }
  removeAttribute(k: string) {
    this.attrs.delete(k)
  }
}

function fakeDoc(title = 'Talaria') {
  const link = new FakeLink('/favicon.svg', 'image/svg+xml')
  let observer: (() => void) | null = null
  const doc: AmbientDoc & { link: FakeLink; rewrite: (t: string) => void } = {
    title,
    head: {} as Node,
    querySelectorAll: () => [link] as unknown as ArrayLike<Element>,
    link,
    // A route rewrites the title (svelte:head), and the observer fires.
    rewrite: (t: string) => {
      doc.title = t
      observer?.()
    },
  }
  const observe = (_: Node, fn: () => void) => {
    observer = fn
    return () => {
      observer = null
    }
  }
  return { doc, observe }
}

const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

describe('titleWith', () => {
  it('prefixes, replaces its own prefix, and is plain at zero', () => {
    expect(titleWith(2, 'Talaria')).toBe('(2) Talaria')
    expect(titleWith(3, '(2) Talaria')).toBe('(3) Talaria')
    expect(titleWith(0, '(2) Talaria - Acme')).toBe('Talaria - Acme')
    expect(titleWith(150, 'Talaria')).toBe('(99+) Talaria')
    expect(titleWith(4, '(99+) Talaria')).toBe('(4) Talaria')
  })
})

describe('startAmbient', () => {
  it('"(2) Talaria" — kept when a route rewrites the title — and plain at 0', async () => {
    const { doc, observe } = fakeDoc()
    const a = startAmbient({ doc, observe, nav: null })
    a.set(2)
    expect(doc.title).toBe('(2) Talaria')
    doc.rewrite('Talaria - Acme')
    expect(doc.title).toBe('(2) Talaria - Acme')
    a.set(0)
    expect(doc.title).toBe('Talaria - Acme')
    a.stop()
  })

  it('the favicon dot is drawn at > 0 and the original restored at 0', async () => {
    const { doc, observe } = fakeDoc()
    const dot = vi.fn(async (href: string) => `data:image/png;base64,DOT(${href})`)
    const a = startAmbient({ doc, observe, nav: null, dot })
    a.set(1)
    await flush()
    expect(doc.link.getAttribute('href')).toBe('data:image/png;base64,DOT(/favicon.svg)')
    expect(doc.link.getAttribute('type')).toBe('image/png')
    a.set(5) // already dotted: not redrawn
    await flush()
    expect(dot).toHaveBeenCalledTimes(1)
    a.set(0)
    await flush()
    expect(doc.link.getAttribute('href')).toBe('/favicon.svg')
    expect(doc.link.getAttribute('type')).toBe('image/svg+xml')
    a.stop()
  })

  it('setAppBadge(2), then setAppBadge(0) — once per change', async () => {
    const { doc, observe } = fakeDoc()
    const setAppBadge = vi.fn(async (_n?: number) => {})
    const a = startAmbient({ doc, observe, nav: { setAppBadge } })
    a.set(2)
    a.set(2)
    a.set(0)
    expect(setAppBadge.mock.calls.map((c) => c[0])).toEqual([2, 0])
    a.stop()
  })

  it('stop (sign-out) puts everything back', async () => {
    const { doc, observe } = fakeDoc()
    const setAppBadge = vi.fn(async (_n?: number) => {})
    const a = startAmbient({ doc, observe, nav: { setAppBadge }, dot: async () => 'data:x' })
    a.set(3)
    await flush()
    a.stop()
    expect(doc.title).toBe('Talaria')
    expect(doc.link.getAttribute('href')).toBe('/favicon.svg')
    expect(setAppBadge.mock.calls.at(-1)?.[0]).toBe(0)
    doc.rewrite('Talaria')
    expect(doc.title).toBe('Talaria') // the observer is gone
  })

  it('a browser without badging, or one that rejects it, is fine', () => {
    const { doc, observe } = fakeDoc()
    const a = startAmbient({ doc, observe, nav: { setAppBadge: () => Promise.reject(new Error('not installed')) } })
    expect(() => a.set(1)).not.toThrow()
    a.stop()
  })
})
