// An open conversation is read only while the person looks at it. A read
// also clears the conversation's bell rows (POST /api/channels/{id}/read), so
// a page nobody looks at — a background tab, a minimized window, an instance
// behind the desktop shell's launcher — that posted one would mark a DM read
// ~100 ms after it landed, before the page's own notification read could
// alert it: found by the #634 + #635 integration run (a DM to a conversation
// open behind the launcher posted no banner, and no badge).
import { describe, expect, it } from 'vitest'
import { lookingNow, readCursorToPost, watchLooking } from './looking'

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

  it('posts nothing for an empty conversation', () => {
    expect(readCursorToPost({ channelId: 'c1', latest: 0, posted: { id: '', seq: 0 }, looking: true })).toBeNull()
  })
})
