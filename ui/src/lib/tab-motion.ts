import { prefersReducedMotion } from '@/lib/motion'
import { isUnder } from '@/lib/route-tabs'

export const LIFT = {
  ease: 'cubic-bezier(0.22, 1, 0.36, 1)',
  revealMs: 280,
  exitMs: 160,
  tab: { rise: 8, scale: 0.92, opacity: 0.4 },
  view: { rise: 12, delayMs: 40, blur: 3 },
  exitRise: 8,
  reducedMs: 150,
} as const

interface Track {
  frames: Keyframe[]
  options: KeyframeAnimationOptions
}

interface LiftPlan {
  tab: Track
  view: Track
  exit: Track
}

export function liftPlan(reduced: boolean): LiftPlan {
  if (reduced) {
    const fade = { duration: LIFT.reducedMs, easing: 'ease' }
    return {
      tab: { frames: [{ opacity: LIFT.tab.opacity }, { opacity: 1 }], options: fade },
      view: { frames: [{ opacity: 0 }, { opacity: 1 }], options: fade },
      exit: { frames: [{ opacity: 1 }, { opacity: 0 }], options: { ...fade, fill: 'forwards' } },
    }
  }
  const enter = { duration: LIFT.revealMs, easing: LIFT.ease, fill: 'backwards' } as const
  return {
    tab: {
      frames: [
        {
          transformOrigin: '50% 100%',
          transform: `translateY(${LIFT.tab.rise}px) scale(${LIFT.tab.scale})`,
          opacity: LIFT.tab.opacity,
        },
        { transformOrigin: '50% 100%', transform: 'none', opacity: 1 },
      ],
      options: enter,
    },
    view: {
      frames: [
        { transform: `translateY(${LIFT.view.rise}px)`, opacity: 0, filter: `blur(${LIFT.view.blur}px)` },
        { transform: 'none', opacity: 1, filter: 'none' },
      ],
      options: { ...enter, delay: LIFT.view.delayMs },
    },
    exit: {
      frames: [
        { transform: 'none', opacity: 1 },
        { transform: `translateY(${-LIFT.exitRise}px)`, opacity: 0 },
      ],
      options: { duration: LIFT.exitMs, easing: LIFT.ease, fill: 'forwards' },
    },
  }
}

interface ExitView {
  clone: HTMLElement
  scrolled: Array<{ el: Element; top: number; left: number }>
}

function captureView(page: HTMLElement): ExitView {
  // Copied inside a document with no browsing context, where nothing fetches,
  // and its media dropped before it joins the page: a copied video or iframe
  // would load again.
  const inert = document.implementation.createHTMLDocument('')
  const clone = inert.importNode(page, true)
  const from = page.querySelectorAll('*')
  const to = clone.querySelectorAll('*')
  const scrolled: ExitView['scrolled'] = []
  from.forEach((el, i) => {
    if (el.scrollTop || el.scrollLeft) scrolled.push({ el: to[i]!, top: el.scrollTop, left: el.scrollLeft })
  })
  for (const media of clone.querySelectorAll('iframe, video, audio, object, embed')) {
    const blank = inert.createElement('div')
    blank.className = media.className
    blank.style.cssText = (media as HTMLElement).style.cssText
    media.replaceWith(blank)
  }
  clone.removeAttribute('data-tab-page')
  clone.setAttribute('aria-hidden', 'true')
  clone.inert = true
  Object.assign(clone.style, {
    position: 'absolute',
    top: `${page.offsetTop}px`,
    left: `${page.offsetLeft}px`,
    width: `${page.offsetWidth}px`,
    height: `${page.offsetHeight}px`,
    margin: '0',
    pointerEvents: 'none',
  })
  return { clone: document.adoptNode(clone), scrolled }
}

export function tabLift(strip: HTMLElement, _pathname: string) {
  let armed: { from: Element | null; to: string; exit: ExitView | null } | null = null
  let lift: { to: string; running: Set<Animation>; leftover: HTMLElement | null } | null = null
  const openTab = () => strip.querySelector<HTMLElement>('a[aria-current="page"]')

  const stop = () => {
    if (!lift) return
    for (const a of lift.running) a.cancel()
    lift.leftover?.remove()
    lift = null
  }

  const play = (tab: HTMLElement, to: string, exit: ExitView | null) => {
    stop()
    const current = { to, running: new Set<Animation>(), leftover: null as HTMLElement | null }
    lift = current
    const track = (anim: Animation) => {
      current.running.add(anim)
      const drop = () => current.running.delete(anim)
      anim.finished.then(drop, drop)
      return anim
    }

    const plan = liftPlan(prefersReducedMotion())
    track(tab.animate(plan.tab.frames, plan.tab.options))
    const page = document.querySelector<HTMLElement>('[data-tab-page]')
    if (!page) return
    track(page.animate(plan.view.frames, plan.view.options))
    if (!exit) return

    // Before the page in the DOM, so the arriving view paints over it.
    page.before(exit.clone)
    for (const { el, top, left } of exit.scrolled) el.scrollTo(left, top)
    current.leftover = exit.clone
    const done = () => {
      exit.clone.remove()
      current.leftover = null
    }
    track(exit.clone.animate(plan.exit.frames, plan.exit.options)).finished.then(done, done)
  }

  const onClick = (event: MouseEvent) => {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    const tile = (event.target as Element).closest('a[href]')
    if (!tile || tile.hasAttribute('aria-current')) return
    const page = document.querySelector<HTMLElement>('[data-tab-page]')
    armed = { from: openTab(), to: tile.getAttribute('href')!, exit: page ? captureView(page) : null }
  }

  const onSwap = () => {
    if (!armed) return
    const tab = openTab()
    if (tab === armed.from) return
    const { to, exit } = armed
    armed = null
    if (tab?.getAttribute('href') === to) play(tab, to, exit)
  }

  const mo = new MutationObserver(onSwap)
  mo.observe(strip, { childList: true })
  strip.addEventListener('click', onClick)
  return {
    // A route change outside the clicked tile spends the click, since that
    // tile's navigation was superseded or refused. A route change outside the
    // lifted tab, such as Back mid-lift, stops the lift: the outlet is reused,
    // so the next view would otherwise finish the old view's motion.
    update(pathname: string) {
      if (armed && !isUnder(pathname, armed.to)) armed = null
      if (lift && !isUnder(pathname, lift.to)) stop()
    },
    destroy() {
      mo.disconnect()
      strip.removeEventListener('click', onClick)
      armed = null
      stop()
    },
  }
}
