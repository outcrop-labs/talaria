// THE PAGE SAYS "YOU HAVE SOMETHING" WITHOUT A BANNER (BT-9, EB-11 web) —
// the signals every enterprise app keeps on its tab: a count in the title
// ("(2) Talaria"), a dot on the favicon, and the OS app badge where the
// browser has one (an installed web app's dock or taskbar icon). All three
// follow the person's For-you unread count (`unreadForYou`, I-12: Needs you +
// Direct; O-10 — Following does not badge), and all three go back to plain at
// zero.
//
// THE TITLE IS NOT OURS: the app's TabTitle and the routes write it. So the
// prefix is applied over whatever title is there, and a MutationObserver on
// <head> puts it back whenever a route change rewrites the title. A count we
// wrote is recognised and replaced, never stacked ("(3) (2) Talaria").
//
// THE FAVICON DOT is drawn on a canvas over the page's own icon, and the
// original href is restored at zero (and when the runtime stops, at
// sign-out).
//
// Framework-free over injected edges (notify-ambient.test.ts).

/** "(n) " — ours, recognised so it is replaced rather than stacked. */
const PREFIX = /^\(\d+\+?\) /

/** The title with the count in front (or without it at zero). */
export function titleWith(count: number, title: string): string {
  const base = title.replace(PREFIX, '')
  if (!(count > 0)) return base
  return `(${count > 99 ? '99+' : Math.floor(count)}) ${base}`
}

export interface AmbientDoc {
  title: string
  head: Node | null
  querySelectorAll(sel: string): ArrayLike<Element>
}

export interface AmbientEnv {
  doc: AmbientDoc
  /** `navigator`, for `setAppBadge` where it exists. */
  nav?: { setAppBadge?: (n?: number) => Promise<void> } | null
  /** Watch <head> for title rewrites; returns the stop. Default: a
   *  MutationObserver. */
  observe?: (target: Node, fn: () => void) => () => void
  /** Draw the dot over an icon; answers a data URL (or null: leave it). */
  dot?: (href: string) => Promise<string | null>
}

export interface Ambient {
  set(count: number): void
  count(): number
  stop(): void
}

export function startAmbient(env: AmbientEnv): Ambient {
  let count = 0
  let stopped = false
  let badged: number | null = null
  let dotFor = 0
  // The icons as the page had them, keyed by element; restored at zero.
  const originals = new Map<Element, { href: string; type: string | null }>()

  const applyTitle = () => {
    if (stopped) return
    const want = titleWith(count, env.doc.title)
    if (env.doc.title !== want) env.doc.title = want
  }

  const icons = (): Element[] => Array.from(env.doc.querySelectorAll('link[rel~="icon"]'))

  const applyIcon = async () => {
    const n = count
    if (n > 0) {
      if (dotFor > 0) return // already dotted; the dot does not carry the number
      dotFor = n
      for (const link of icons()) {
        const href = link.getAttribute('href') ?? ''
        if (!originals.has(link)) originals.set(link, { href, type: link.getAttribute('type') })
        const src = originals.get(link)?.href ?? href
        const url = env.dot ? await env.dot(src).catch(() => null) : null
        if (stopped || count === 0) return
        if (url) {
          link.setAttribute('href', url)
          // A data URL is a PNG whatever the original's type was.
          if (link.getAttribute('type')) link.setAttribute('type', 'image/png')
        }
      }
      return
    }
    dotFor = 0
    restoreIcons()
  }

  const restoreIcons = () => {
    for (const [link, was] of originals) {
      link.setAttribute('href', was.href)
      if (was.type) link.setAttribute('type', was.type)
      else link.removeAttribute('type')
    }
    originals.clear()
  }

  const applyBadge = () => {
    const n = count > 0 ? Math.floor(count) : 0
    if (badged === n) return
    badged = n
    try {
      void env.nav?.setAppBadge?.(n)?.catch?.(() => {})
    } catch {
      /* no badging here */
    }
  }

  const observe =
    env.observe ??
    ((target: Node, fn: () => void) => {
      if (typeof MutationObserver === 'undefined') return () => {}
      const mo = new MutationObserver(fn)
      mo.observe(target, { subtree: true, childList: true, characterData: true })
      return () => mo.disconnect()
    })
  const unobserve = env.doc.head ? observe(env.doc.head, applyTitle) : () => {}

  return {
    set: (n) => {
      if (stopped) return
      const next = Number.isFinite(n) && n > 0 ? Math.floor(n) : 0
      if (next === count && badged !== null) return
      count = next
      applyTitle()
      void applyIcon()
      applyBadge()
    },
    count: () => count,
    stop: () => {
      if (stopped) return
      count = 0
      applyTitle()
      applyBadge()
      stopped = true
      unobserve()
      restoreIcons()
    },
  }
}

/** The browser's dot: the icon drawn on a 64 px canvas, a red dot in its
 *  top-right corner with a ring in the page's ground colour. */
export async function canvasDot(href: string): Promise<string | null> {
  if (typeof document === 'undefined' || typeof Image === 'undefined') return null
  const img = new Image()
  img.decoding = 'async'
  const loaded = new Promise<boolean>((resolve) => {
    img.onload = () => resolve(true)
    img.onerror = () => resolve(false)
  })
  img.src = href
  if (!(await loaded)) return null
  const size = 64
  const canvas = document.createElement('canvas')
  canvas.width = size
  canvas.height = size
  const ctx = canvas.getContext('2d')
  if (!ctx) return null
  ctx.drawImage(img, 0, 0, size, size)
  const r = size * 0.2
  const cx = size - r - 2
  const cy = r + 2
  ctx.beginPath()
  ctx.arc(cx, cy, r + 3, 0, Math.PI * 2)
  ctx.fillStyle = '#0b0b0d'
  ctx.fill()
  ctx.beginPath()
  ctx.arc(cx, cy, r, 0, Math.PI * 2)
  ctx.fillStyle = '#e5484d'
  ctx.fill()
  try {
    return canvas.toDataURL('image/png')
  } catch {
    return null
  }
}
