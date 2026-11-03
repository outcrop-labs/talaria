// Does the desktop shell's OS-link script hand over only what the person
// clicked? This runs the shipped script, `OS_LINKS` in
// desktop/src-tauri/src/links.rs (read out of the Rust source), against a stub
// window, one click at a time.
//
// WHY. The script is the only way a link reaches the OS (mail, the dialer):
// the shell's navigation and new-window handlers refuse every OS scheme,
// because none of the three engines tells them whether a click or a script
// asked, or whether the page or a frame in it did. A microsite artifact is
// agent-written HTML running scripts in a sandboxed frame. So the script must
// take a trusted primary-button click on a `mailto:`, `tel:` or `sms:` link
// that the page did not handle itself, and nothing else: no `a.click()` from
// a script, no other scheme, no http link (that is the new-window handler's).
// It runs only in the main frame, as initialization scripts do. The rig shows
// that a frame's clicks never reach it; this is the cheap check on every
// commit that its branches stay right.
//
// THE STUB. Only what the script touches: `window.__TAURI_INTERNALS__.invoke`,
// `window.addEventListener`, `Element` with `localName` and `hasAttribute`,
// `URL`, and a click event with `isTrusted`, `defaultPrevented`, `button`,
// `composedPath()` and `preventDefault()`. If the script starts using more,
// this fails loudly and the stub grows with it. Stdlib only.

/** The script's text, out of the Rust raw string. */
export function osLinksSource(rust) {
  const m = /pub const OS_LINKS: &str = r#"([\s\S]*?)"#;/.exec(rust)
  return m ? m[1] : null
}

class Element {
  constructor(localName, attrs = {}, parent = null) {
    this.localName = localName
    this.attrs = attrs
    this.parent = parent
  }
  hasAttribute(name) {
    return name in this.attrs
  }
  get href() {
    // An anchor's href resolves against the page; an SVG <a> has no string href.
    if (this.localName !== 'a' || !('href' in this.attrs)) return undefined
    if (this.attrs.svg) return { baseVal: this.attrs.href }
    return new URL(this.attrs.href, 'https://talaria.example.com/comms').href
  }
}

/** What the script got wrong, as sentences; empty when it is right. */
export function checkOsLinks(rust) {
  const src = osLinksSource(rust)
  if (src === null) return ['no `pub const OS_LINKS: &str = r#"…"#;` in links.rs']
  const listeners = []
  const calls = []
  const window = {
    __TAURI_INTERNALS__: {
      invoke(cmd, args) {
        calls.push({ cmd, args })
        return Promise.resolve(null)
      },
    },
    addEventListener(type, fn, capture) {
      listeners.push({ type, fn, capture })
    },
  }
  try {
    new Function('window', 'Element', 'URL', src)(window, Element, URL)
  } catch (e) {
    return [`the script does not run: ${e.message}`]
  }
  const wrong = []
  const expect = (what, ok) => {
    if (!ok) wrong.push(what)
  }
  const click = (target, { trusted = true, prevented = false, button = 0 } = {}) => {
    const path = []
    for (let n = target; n; n = n.parent) path.push(n)
    const e = {
      type: 'click',
      isTrusted: trusted,
      defaultPrevented: prevented,
      button,
      composedPath: () => path,
      preventDefault() {
        this.defaultPrevented = true
      },
    }
    const before = calls.length
    for (const l of listeners) if (l.type === 'click') l.fn(e)
    return { prevented: e.defaultPrevented && !prevented, calls: calls.slice(before) }
  }
  const opens = (desc, target, url, opts) => {
    const r = click(target, opts)
    const ok = r.prevented && r.calls.length === 1 && r.calls[0].cmd === 'desktop_open_link' && r.calls[0].args?.url === url
    expect(`${desc} goes to the OS`, ok)
  }
  const ignores = (desc, target, opts) => {
    const r = click(target, opts)
    expect(`${desc} is left to the page`, !r.prevented && r.calls.length === 0)
  }
  try {
    expect(
      'one click listener, in the bubble phase (after the page has had the click)',
      listeners.length === 1 && listeners[0].type === 'click' && !listeners[0].capture,
    )
    const body = new Element('body')
    const mail = new Element('a', { href: 'mailto:someone@example.com?subject=hi' }, body)
    opens('a clicked mailto: link', mail, 'mailto:someone@example.com?subject=hi')
    opens('a click on a span inside a mailto: link', new Element('span', {}, mail), 'mailto:someone@example.com?subject=hi')
    opens('a clicked tel: link', new Element('a', { href: 'tel:+15551234567' }, body), 'tel:+15551234567')
    opens('a clicked sms: link', new Element('a', { href: 'sms:+15551234567' }, body), 'sms:+15551234567')
    opens('a clicked MAILTO: link (any case)', new Element('a', { href: 'MAILTO:x@example.com' }, body), 'mailto:x@example.com')

    ignores('a mailto: link a script clicked (a.click(), untrusted)', mail, { trusted: false })
    ignores('a mailto: link the page handled itself', mail, { prevented: true })
    ignores('a middle-button click', mail, { button: 1 })
    for (const href of [
      'vscode://file/etc/passwd',
      'ms-officecmd:{}',
      'jnlp://evil.example/x.jnlp',
      'zoommtg://zoom.us/join?confno=1',
      'file:///etc/hostname',
      'javascript:void(0)',
      'https://drive.google.com/file/d/x',
      '/settings',
    ]) {
      ignores(`a clicked ${href.split(':')[0]}: link`, new Element('a', { href }, body))
    }
    ignores('an <a> without href', new Element('a', {}, body))
    ignores('an SVG <a> (no string href)', new Element('a', { href: 'mailto:x@example.com', svg: true }, body))
    ignores('a click outside any link', new Element('div', {}, body))

    // Outside the shell the script does nothing, and never throws.
    let threw = null
    try {
      new Function('window', 'Element', 'URL', src)({}, Element, URL)
    } catch (e) {
      threw = e
    }
    expect('the script does nothing outside the shell', threw === null)
  } catch (e) {
    wrong.push(`the script threw against the stub: ${e.message}`)
  }
  return wrong
}
