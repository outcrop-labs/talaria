import { prefersReducedMotion } from '@/lib/motion'

// The playground's "Lift", tuned: the clicked tab and its view rise out of the
// bar while the old view keeps rising and fades. Durations in ms.
export const LIFT = {
  ease: 'cubic-bezier(0.22, 1, 0.36, 1)',
  reveal: 280,
  exit: 160,
  tab: { rise: 8, scale: 0.92, opacity: 0.4 },
  view: { rise: 12, delay: 40, blur: 3 },
  sink: 8,
  reduced: 150,
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
    const fade = { duration: LIFT.reduced, easing: 'ease' }
    return {
      tab: { frames: [{ opacity: LIFT.tab.opacity }, { opacity: 1 }], options: fade },
      view: { frames: [{ opacity: 0 }, { opacity: 1 }], options: fade },
      exit: { frames: [{ opacity: 1 }, { opacity: 0 }], options: { ...fade, fill: 'forwards' } },
    }
  }
  const enter = { duration: LIFT.reveal, easing: LIFT.ease, fill: 'backwards' } as const
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
      options: { ...enter, delay: LIFT.view.delay },
    },
    exit: {
      frames: [
        { transform: 'none', opacity: 1 },
        { transform: `translateY(${-LIFT.sink}px)`, opacity: 0 },
      ],
      options: { duration: LIFT.exit, easing: LIFT.ease, fill: 'forwards' },
    },
  }
}

interface ExitView {
  clone: HTMLElement
  scrolled: Array<{ el: Element; top: number; left: number }>
}

// The old view is gone once the route swaps, so a copy is taken at the click.
// The copy loses its scroll offsets, which are carried over, and its embedded
// media is blanked so an iframe never loads twice.
function captureView(page: HTMLElement): ExitView {
  const clone = page.cloneNode(true) as HTMLElement
  const from = page.querySelectorAll('*')
  const to = clone.querySelectorAll('*')
  const scrolled: ExitView['scrolled'] = []
  from.forEach((el, i) => {
    if (el.scrollTop || el.scrollLeft) scrolled.push({ el: to[i]!, top: el.scrollTop, left: el.scrollLeft })
  })
  for (const media of clone.querySelectorAll('iframe, video, audio, object, embed')) {
    const blank = document.createElement('div')
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
  return { clone, scrolled }
}

let running: Animation[] = []
let leftover: HTMLElement | null = null

function play(tab: HTMLElement, page: HTMLElement | null, exit: ExitView | null) {
  for (const a of running) a.cancel()
  leftover?.remove()
  running = []
  leftover = null

  const plan = liftPlan(prefersReducedMotion())
  running.push(tab.animate(plan.tab.frames, plan.tab.options))
  if (!page) return
  running.push(page.animate(plan.view.frames, plan.view.options))
  if (!exit) return

  // Before the page in the DOM, so the arriving view paints over it.
  page.before(exit.clone)
  for (const { el, top, left } of exit.scrolled) el.scrollTo(left, top)
  const out = exit.clone.animate(plan.exit.frames, plan.exit.options)
  running.push(out)
  leftover = exit.clone
  const done = () => {
    exit.clone.remove()
    if (leftover === exit.clone) leftover = null
  }
  out.finished.then(done, done)
}

// The strip's switch hook. A plain left click on a closed tile arms the lift
// and copies the open view; the lift plays once the strip shows the clicked
// tile as the open tab. A route change nobody clicked here, such as a deep
// link or the back button, never arms it.
export function tabLift(strip: HTMLElement) {
  let armed: { from: Element | null; to: string; exit: ExitView | null } | null = null
  const openTab = () => strip.querySelector<HTMLElement>('a[aria-current="page"]')

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
    if (tab?.getAttribute('href') === to) play(tab, document.querySelector('[data-tab-page]'), exit)
  }

  const mo = new MutationObserver(onSwap)
  mo.observe(strip, { childList: true })
  strip.addEventListener('click', onClick)
  return {
    destroy() {
      mo.disconnect()
      strip.removeEventListener('click', onClick)
    },
  }
}
