// An open conversation is read only while the person looks at it. A read
// also clears the conversation's bell rows (POST /api/channels/{id}/read), so
// a page nobody looks at — a background tab, a minimized window, an instance
// behind the desktop shell's launcher — that posted one would mark a DM read
// ~100 ms after it landed, before the page's own notification read could
// alert it: found by the #634 + #635 integration run (a DM to a conversation
// open behind the launcher posted no banner, and no badge).
import { describe, expect, it } from 'vitest'
import { LOOKING_INPUT_MS, lastInputAt, lookingNow, lookingWithInput, noteInput, onInput, readCursorToPost, resetInputForTests, trackInput, watchLooking } from './looking'

class FakeDoc extends EventTarget {
  visibilityState: DocumentVisibilityState = 'visible'
  focused = true
  hasFocus() {
    return this.focused
  }
}

describe('lookingNow', () => {
  it('is shown AND focused', () => {
    const doc = new FakeDoc()
    expect(lookingNow(doc)).toBe(true)
    doc.focused = false // another window has the focus; a minimized WebKitGTK window reads like this
    expect(lookingNow(doc)).toBe(false)
    doc.focused = true
    doc.visibilityState = 'hidden' // a background tab, or a webview behind the shell's launcher
    expect(lookingNow(doc)).toBe(false)
  })

  it('is false with no document', () => {
    expect(lookingNow(undefined)).toBe(false)
  })
})

describe('watchLooking', () => {
  it('reports each change once, from focus, blur and visibility, and stops when asked', () => {
    const win = new EventTarget()
    const doc = new FakeDoc()
    const seen: boolean[] = []
    const stop = watchLooking((v) => seen.push(v), win, doc)
    doc.focused = false
    win.dispatchEvent(new Event('blur'))
    win.dispatchEvent(new Event('blur')) // no change, no report
    doc.focused = true
    win.dispatchEvent(new Event('focus'))
    doc.visibilityState = 'hidden'
    doc.dispatchEvent(new Event('visibilitychange'))
    doc.visibilityState = 'visible'
    win.dispatchEvent(new Event('pageshow'))
    expect(seen).toEqual([false, true, false, true])
    stop()
    doc.focused = false
    win.dispatchEvent(new Event('blur'))
    expect(seen).toEqual([false, true, false, true])
  })
})

describe('readCursorToPost', () => {
  const posted = { id: 'c1', seq: 4 }

  it('posts nothing while the person is not looking, however new the message', () => {
    expect(readCursorToPost({ channelId: 'c1', latest: 9, posted, looking: false })).toBeNull()
    expect(readCursorToPost({ channelId: 'c2', latest: 9, posted, looking: false })).toBeNull()
  })

  it('posts the newest seq once they look', () => {
    expect(readCursorToPost({ channelId: 'c1', latest: 9, posted, looking: true })).toBe(9)
  })

  it('posts nothing already posted for this channel, and the newest for another', () => {
    expect(readCursorToPost({ channelId: 'c1', latest: 4, posted, looking: true })).toBeNull()
    expect(readCursorToPost({ channelId: 'c1', latest: 3, posted, looking: true })).toBeNull()
    expect(readCursorToPost({ channelId: 'c2', latest: 2, posted, looking: true })).toBe(2)
  })

  it("posts a second thread whose newest reply is older than the first thread's", () => {
    // ThreadPanel keys by root and is not remounted between threads; reply
    // seqs are channel-wide, so the older thread's newest sits below the
    // first thread's cursor and must still post.
    const first = { id: 'root-a', seq: 50 }
    expect(readCursorToPost({ channelId: 'root-b', latest: 30, posted: first, looking: true })).toBe(30)
    expect(readCursorToPost({ channelId: 'root-a', latest: 50, posted: first, looking: true })).toBeNull()
  })

  it('posts nothing for an empty conversation', () => {
    expect(readCursorToPost({ channelId: 'c1', latest: 0, posted: { id: '', seq: 0 }, looking: true })).toBeNull()
  })
})

describe('input: the half of looking a document cannot answer', () => {
  it('trackInput records pointer, key, wheel and touch, samples moves once a second, and releases when the last holder does', () => {
    resetInputForTests()
    const win = Object.assign(new EventTarget(), { document: Object.assign(new FakeDoc(), { visibilityState: 'hidden' as DocumentVisibilityState }) })
    let t = 1000
    const a = trackInput(win, () => t)
    const b = trackInput(win, () => t)
    expect(lastInputAt()).toBe(0) // a hidden page's load is not input
    for (const k of ['pointerdown', 'keydown', 'wheel', 'touchstart']) {
      t += 10
      win.dispatchEvent(new Event(k))
      expect(lastInputAt()).toBe(t)
    }
    const at0 = lastInputAt()
    t += 1500
    win.dispatchEvent(new Event('pointermove'))
    expect(lastInputAt()).toBe(t)
    t += 200
    win.dispatchEvent(new Event('pointermove'))
    expect(lastInputAt()).toBe(t - 200) // sampled
    expect(at0).toBeLessThan(lastInputAt())
    a()
    t += 5000
    win.dispatchEvent(new Event('keydown'))
    expect(lastInputAt()).toBe(t) // b still holds it
    b()
    t += 5000
    win.dispatchEvent(new Event('keydown'))
    expect(lastInputAt()).toBe(t - 5000) // released
    resetInputForTests()
  })

  it('a page that opens shown and focused counts its load as input', () => {
    resetInputForTests()
    const win = Object.assign(new EventTarget(), { document: new FakeDoc() })
    const stop = trackInput(win, () => 777)
    expect(lastInputAt()).toBe(777)
    stop()
    resetInputForTests()
  })

  it('lookingWithInput is shown, focused AND touched within 60 s', () => {
    const doc = new FakeDoc()
    expect(lookingWithInput(doc, 100_000, 100_000 - LOOKING_INPUT_MS)).toBe(true)
    expect(lookingWithInput(doc, 100_000, 100_000 - LOOKING_INPUT_MS - 1)).toBe(false)
    expect(lookingWithInput(doc, 100_000, 0)).toBe(false)
    doc.focused = false
    expect(lookingWithInput(doc, 100_000, 99_000)).toBe(false)
  })

  it('onInput hears each new input; an older timestamp is ignored', () => {
    resetInputForTests()
    const seen: number[] = []
    const off = onInput((at) => seen.push(at))
    noteInput(10)
    noteInput(5)
    noteInput(20)
    off()
    noteInput(30)
    expect(seen).toEqual([10, 20])
    resetInputForTests()
  })
})

