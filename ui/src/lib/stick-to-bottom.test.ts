import assert from 'node:assert/strict'
import { test } from 'vitest'
import { bottomStick, distanceFromBottom, NEAR_BOTTOM_PX, type ScrollBox } from './stick-to-bottom'

// The behaviour is three rules — follow, hold, release — and every one of them
// is about WHEN state is read rather than what it computes. That makes it very
// easy to get subtly wrong and impossible to notice in review, so it gets a
// fake scroll box and a test per rule.

/** A scroll container with the browser's clamping, and nothing else. */
function box(clientHeight = 500, scrollHeight = 500): ScrollBox & { grow(by: number): void } {
  let top = 0
  const listeners = new Set<() => void>()
  const self = {
    clientHeight,
    scrollHeight,
    get scrollTop() {
      return top
    },
    set scrollTop(v: number) {
      // The browser clamps to the scrollable range; `follow` relies on it.
      const max = Math.max(0, self.scrollHeight - self.clientHeight)
      const next = Math.min(Math.max(0, v), max)
      if (next === top) return
      top = next
      for (const fn of listeners) fn()
    },
    addEventListener: (_t: 'scroll', fn: () => void) => void listeners.add(fn),
    removeEventListener: (_t: 'scroll', fn: () => void) => void listeners.delete(fn),
    /** Content arrives. Deliberately fires NO scroll event — browsers don't. */
    grow(by: number) {
      self.scrollHeight += by
    },
  }
  return self
}

test('follows new content when the reader is at the bottom', () => {
  const el = box()
  const stick = bottomStick()
  stick.attach(el)
  stick.jump()
  el.grow(300)
  stick.follow()
  assert.equal(distanceFromBottom(el), 0)
})

test('a tall message does not break the follow', () => {
  // THE REGRESSION. The old check measured after the content landed, so a
  // message taller than its threshold made a reader who had not moved look
  // like one who had — and the taller the message, the more certainly it
  // stopped following.
  const el = box()
  const stick = bottomStick()
  stick.attach(el)
  stick.jump()
  el.grow(4000) // one very long answer
  stick.follow()
  assert.equal(distanceFromBottom(el), 0, 'stopped following a tall message')
})

test('streaming keeps up flush after flush', () => {
  const el = box()
  const stick = bottomStick()
  stick.attach(el)
  stick.jump()
  for (let i = 0; i < 50; i++) {
    el.grow(90) // a token flush growing the last message in place
    stick.follow()
  }
  assert.equal(distanceFromBottom(el), 0)
})

test('holds position once the reader scrolls up', () => {
  const el = box(500, 2000)
  const stick = bottomStick()
  stick.attach(el)
  stick.jump()
  el.scrollTop = 200 // the reader goes back to read
  assert.equal(stick.held, true)

  const before = el.scrollTop
  el.grow(600)
  stick.follow()
  assert.equal(el.scrollTop, before, 'yanked a reader out of the history')
})

test('a nudge inside the tolerance is not a hold', () => {
  const el = box(500, 2000)
  const stick = bottomStick()
  stick.attach(el)
  stick.jump()
  el.scrollTop = el.scrollHeight - el.clientHeight - (NEAR_BOTTOM_PX - 1)
  assert.equal(stick.held, false, 'a hair off the bottom counted as intent')
  el.grow(400)
  stick.follow()
  assert.equal(distanceFromBottom(el), 0)
})

test('scrolling back to the bottom releases the hold', () => {
  // Without this the transcript stays frozen for the rest of the session after
  // one scroll up, which is the complaint that started this.
  const el = box(500, 2000)
  const stick = bottomStick()
  stick.attach(el)
  el.scrollTop = 100
  assert.equal(stick.held, true)
  el.scrollTop = el.scrollHeight // back to the end
  assert.equal(stick.held, false)
  el.grow(300)
  stick.follow()
  assert.equal(distanceFromBottom(el), 0)
})

test('jump overrides a hold — sending always shows your own message', () => {
  const el = box(500, 2000)
  const stick = bottomStick()
  stick.attach(el)
  el.scrollTop = 100
  assert.equal(stick.held, true)
  stick.jump()
  assert.equal(stick.held, false)
  assert.equal(distanceFromBottom(el), 0)
})

test('detaching stops listening, and a detached stick is inert', () => {
  const el = box(500, 2000)
  const stick = bottomStick()
  const off = stick.attach(el)!
  off()
  el.scrollTop = 100
  assert.equal(stick.held, false, 'still tracking a container it let go of')
  assert.doesNotThrow(() => {
    stick.follow()
    stick.jump()
  })
})

test('attach(null) is safe — the container may not be mounted yet', () => {
  const stick = bottomStick()
  assert.equal(stick.attach(null), undefined)
  assert.doesNotThrow(() => {
    stick.follow()
    stick.jump()
  })
})
