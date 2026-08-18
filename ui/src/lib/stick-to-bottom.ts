/**
 * Follow the newest message, unless the reader has scrolled away to read.
 *
 * Three surfaces had their own copy of this (ChatView, ChannelView,
 * ThreadPanel) and all three shared one bug, which is the reason this is now a
 * module with a test rather than a fourth copy.
 *
 * THE BUG: they decided whether to follow by measuring the scroll position at
 * FOLLOW time — after the new content was already in the DOM:
 *
 *     const pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 120
 *     if (loaded || pinned) el.scrollTop = el.scrollHeight
 *
 * A reader parked exactly at the bottom is 0px from it until a message
 * arrives; at that instant they are (message height)px from it, because the
 * content grew underneath them and nothing moved them. So any message taller
 * than the threshold read as "the user has scrolled up" and the transcript
 * stopped following — and it failed harder the longer the message, which is
 * exactly backwards. Streaming made it worse: the first flush that crossed
 * 120px froze the view for the rest of the turn.
 *
 * THE FIX is to stop inferring intent from geometry after the fact. `held` is
 * decided only when a SCROLL EVENT fires, which is the moment the reader
 * actually moved — content growing does not fire one. So the flag still says
 * what was true before the message landed, and the follow is correct however
 * tall it is.
 *
 * It also means "intentionally scrolled up" needs no heuristic: scrolling away
 * holds, scrolling back to the bottom releases, and nothing else touches it.
 * There is deliberately no timer that returns a reader to the bottom on its
 * own — yanking someone out of the history they are reading is the one failure
 * worse than not following.
 */

/**
 * How close to the bottom still counts as "at the bottom", in px.
 *
 * Not zero: sub-pixel layout, zoom, and a trackpad's last inertial pixel all
 * leave a reader a hair short of the end, and they did not mean to leave. Not
 * the old 120 either — that is far enough to be a deliberate nudge upward.
 */
export const NEAR_BOTTOM_PX = 64

/** Distance from the bottom of the scroll range, in px. */
export const distanceFromBottom = (el: ScrollBox): number =>
  el.scrollHeight - el.scrollTop - el.clientHeight

/** The part of an element this needs — so a test can supply a plain object. */
export interface ScrollBox {
  scrollHeight: number
  scrollTop: number
  clientHeight: number
  addEventListener(type: 'scroll', fn: () => void, opts?: { passive?: boolean }): void
  removeEventListener(type: 'scroll', fn: () => void): void
}

export interface BottomStick {
  /**
   * Bind the scroll container. Returns a teardown, so the call site is
   * `$effect(() => stick.attach(scrollEl))` and Svelte owns the lifecycle.
   */
  attach(node: ScrollBox | null): (() => void) | undefined
  /** Content changed: follow, unless the reader is holding position. */
  follow(): void
  /** Go to the newest message and release any hold — a send, or a new thread. */
  jump(): void
  /** Whether the reader has scrolled away. Exposed for tests and future affordances. */
  readonly held: boolean
}

export function bottomStick(): BottomStick {
  let el: ScrollBox | null = null
  let held = false

  return {
    attach(node) {
      el = node
      if (!node) return
      // The ONLY writer of `held`. A scroll event means the reader moved;
      // content arriving does not fire one, which is the whole point.
      const onScroll = (): void => {
        held = distanceFromBottom(node) > NEAR_BOTTOM_PX
      }
      node.addEventListener('scroll', onScroll, { passive: true })
      return () => {
        node.removeEventListener('scroll', onScroll)
        if (el === node) el = null
      }
    },
    follow() {
      // Assigning past the maximum is clamped by the browser, so this lands at
      // the end without needing to compute where the end is.
      if (!el || held) return
      const node = el
      node.scrollTop = node.scrollHeight
      // AND AGAIN NEXT FRAME, because `scrollHeight` is only as current as the
      // DOM. A streamed reply renders through markdown, and a code block, an
      // image or a table settles its own layout in a later update — so the
      // height read here can still be the one from before the delta landed, and
      // the view ends up a line short of the newest text. Every delta, that
      // reads as the transcript creeping upward while the answer writes itself
      // below the fold.
      //
      // Re-reading next frame costs one clamped assignment and fixes it for
      // every caller, which is why it is here and not at four call sites.
      // `held` is re-checked: a reader who scrolled away in between wins.
      if (typeof requestAnimationFrame !== 'function') return
      requestAnimationFrame(() => {
        if (el === node && !held) node.scrollTop = node.scrollHeight
      })
    },
    jump() {
      held = false
      if (el) el.scrollTop = el.scrollHeight
    },
    get held() {
      return held
    },
  }
}
