// Does the desktop shell's drop guard do what it says? It runs the shipped
// script — `DROP_GUARD` in desktop/src-tauri/src/commands.rs, read out of the
// Rust source — against a stub DOM, one drag at a time.
//
// WHY. Instance webviews have no Tauri drag-drop handler, so the engine's
// browser-tab default applies to a drop the page did not take: it loads the
// dropped file or link in place of the instance. The guard cancels such a
// drag ("no drop here") — but a first version also cancelled drags over
// native fields, which take drops by the engine's default with nobody calling
// `preventDefault`: text into an <input>, a selection moved in a <textarea>,
// a file onto an <input type=file>, anything contenteditable. This holds both
// halves. Real drags in WebKitGTK are the rig's job (PR #652); this is the
// cheap, every-commit check that the script's branches stay right.
//
// THE STUB. Only what the guard touches: `window.addEventListener`, `Element`
// with `closest()` over tag names and an inherited `isContentEditable`, a text
// node's `parentElement`, and an event with `target`, `defaultPrevented`,
// `preventDefault()` and `dataTransfer.dropEffect`. A guard that starts using
// more of the DOM fails here loudly (a TypeError is reported, not swallowed),
// and this stub grows with it. Stdlib only.

/** The script's text, out of the Rust raw string. */
export function dropGuardSource(rust) {
  const m = /const DROP_GUARD: &str = r#"([\s\S]*?)"#;/.exec(rust)
  return m ? m[1] : null
}

class Node {
  constructor(parent = null) {
    this.parentElement = parent
  }
}

class Element extends Node {
  constructor(tag, parent = null, attrs = {}) {
    super(parent)
    this.tagName = tag.toUpperCase()
    this.attrs = attrs
  }
  closest(selector) {
    const tags = selector.split(',').map((s) => s.trim().toUpperCase())
    if (tags.some((t) => !/^[A-Z]+$/.test(t))) throw new Error(`the stub's closest() takes tag names only, not "${selector}"`)
    for (let el = this; el; el = el.parentElement) if (tags.includes(el.tagName)) return el
    return null
  }
  get isContentEditable() {
    for (let el = this; el; el = el.parentElement) {
      const v = el.attrs.contenteditable
      if (v === 'false') return false
      if (v === '' || v === 'true') return true
    }
    return false
  }
}

/** What the guard got wrong, as sentences; empty when it is right. */
export function checkDropGuard(rust) {
  const src = dropGuardSource(rust)
  if (src === null) return ['no `const DROP_GUARD: &str = r#"…"#;` in commands.rs']
  const listeners = {}
  const window = { addEventListener: (type, fn) => (listeners[type] ??= []).push(fn) }
  try {
    new Function('window', 'Element', src)(window, Element)
  } catch (e) {
    return [`the script does not run: ${e.message}`]
  }
  const fire = (type, target, { zone = false, dataTransfer = { dropEffect: 'copy' } } = {}) => {
    const e = {
      type,
      target,
      defaultPrevented: zone,
      dataTransfer,
      preventDefault() {
        this.defaultPrevented = true
      },
    }
    for (const fn of listeners[type] ?? []) fn(e)
    return e
  }
  const body = new Element('body', new Element('html'))
  const wrong = []
  const expect = (what, ok) => {
    if (!ok) wrong.push(what)
  }
  try {
    expect('it listens to dragover and drop on the window', listeners.dragover?.length && listeners.drop?.length)

    const div = new Element('div', body)
    let e = fire('dragover', div)
    expect('a drag no zone took is cancelled', e.defaultPrevented)
    expect('… with "no drop here"', e.dataTransfer.dropEffect === 'none')
    expect('a drop no zone took is cancelled (no navigation)', fire('drop', div).defaultPrevented)

    e = fire('dragover', div, { zone: true })
    expect("a zone's own dragover keeps its drop effect", e.dataTransfer.dropEffect === 'copy')

    for (const [name, el] of [
      ['a text <input>', new Element('input', body, { type: 'text' })],
      ['an <input type=file>', new Element('input', body, { type: 'file' })],
      ['a <textarea>', new Element('textarea', body)],
      ['a <select>', new Element('select', body)],
      ['a node inside a contenteditable', new Element('span', new Element('div', body, { contenteditable: 'true' }))],
      ['a text node inside a <textarea>', new Node(new Element('textarea', body))],
    ]) {
      e = fire('dragover', el)
      expect(`a drag over ${name} is left to the engine`, !e.defaultPrevented && e.dataTransfer.dropEffect === 'copy')
      expect(`a drop on ${name} is left to the engine`, !fire('drop', el).defaultPrevented)
    }

    const off = new Element('span', new Element('div', body, { contenteditable: 'false' }))
    expect('contenteditable="false" is no field: the guard applies', fire('dragover', off).defaultPrevented)
    expect('a drag with no dataTransfer does not throw', fire('dragover', div, { dataTransfer: null }).defaultPrevented)
  } catch (e) {
    wrong.push(`the guard threw against the stub DOM: ${e.message}`)
  }
  return wrong
}
