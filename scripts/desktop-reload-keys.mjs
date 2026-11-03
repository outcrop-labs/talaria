// Do the desktop shell's reload keys do what they say? This runs the shipped
// script, `RELOAD_KEYS` in desktop/src-tauri/src/reload.rs (read out of the
// Rust source), against a stub window: one key at a time, on macOS and
// elsewhere.
//
// WHY. An instance webview has no browser chrome, so these keys are the way to
// reload a page that broke. That includes a page whose Talaria UI never
// loaded. The mapping is per OS:
// - Windows and Linux: Ctrl+R and F5 reload, and Ctrl+Shift+R, Shift+F5 and
//   Ctrl+F5 skip the cache.
// - macOS: Cmd+R reloads and Cmd+Shift+R skips the cache. Ctrl+R and F5 are
//   left alone there.
// The script must also leave other keys alone: a Cmd+R on Linux, an Alt
// chord, a held key's repeats, Ctrl+P on Dvorak (whose KeyR position types p).
// When the shell refuses the command (an origin with no grant), the script
// still reloads, by itself. Real key presses in WebKitGTK are the rig's job;
// this is the cheap check on every commit that the branches stay right.
//
// THE STUB. Only what the script touches: `window.__TAURI_INTERNALS__.invoke`,
// `window.addEventListener`, `navigator.userAgent`, `location.reload`, and a
// keydown event with `key`, `code`, the four modifiers, `repeat` and
// `preventDefault()`. If the script starts using more, this fails loudly and
// the stub grows with it. Stdlib only.

/** The script's text, out of the Rust raw string. */
export function reloadKeysSource(rust) {
  const m = /pub const RELOAD_KEYS: &str = r#"([\s\S]*?)"#;/.exec(rust)
  return m ? m[1] : null
}

const MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)'
const LINUX = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)'
const WINDOWS = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0'

/** Load the script in a stub window. `refuse`: the shell answers every
 *  invoke with an error, as it does for an origin with no grant. */
function load(src, userAgent, { refuse = false } = {}) {
  const listeners = []
  const calls = []
  let reloads = 0
  const window = {
    __TAURI_INTERNALS__: {
      invoke(cmd, args) {
        calls.push({ cmd, args })
        return refuse ? Promise.reject(new Error(`${cmd} not allowed`)) : Promise.resolve(null)
      },
    },
    addEventListener(type, fn, capture) {
      listeners.push({ type, fn, capture })
    },
  }
  const location = {
    reload() {
      reloads++
    },
  }
  new Function('window', 'navigator', 'location', src)(window, { userAgent }, location)
  const press = (key, { code, ctrl = false, meta = false, shift = false, alt = false, repeat = false } = {}) => {
    const e = {
      type: 'keydown',
      key,
      code: code ?? (key.length === 1 ? `Key${key.toUpperCase()}` : key),
      ctrlKey: ctrl,
      metaKey: meta,
      shiftKey: shift,
      altKey: alt,
      repeat,
      defaultPrevented: false,
      preventDefault() {
        this.defaultPrevented = true
      },
    }
    const before = calls.length
    for (const l of listeners) if (l.type === 'keydown') l.fn(e)
    const call = calls.slice(before)
    return { prevented: e.defaultPrevented, calls: call }
  }
  return { listeners, press, reloads: () => reloads }
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

/** What the keys got wrong, as sentences; empty when they are right. */
export async function checkReloadKeys(rust) {
  const src = reloadKeysSource(rust)
  if (src === null) return ['no `pub const RELOAD_KEYS: &str = r#"…"#;` in reload.rs']
  const wrong = []
  const expect = (what, ok) => {
    if (!ok) wrong.push(what)
  }
  // One key on one OS: reloads (hard or not), or is left alone.
  const reloads = (os, page, desc, key, mods, hard) => {
    const r = page.press(key, mods)
    const ok = r.prevented && r.calls.length === 1 && r.calls[0].cmd === 'desktop_reload' && r.calls[0].args?.hard === hard
    expect(`${os}: ${desc} ${hard ? 'reloads skipping the cache' : 'reloads'}`, ok)
  }
  const ignores = (os, page, desc, key, mods) => {
    const r = page.press(key, mods)
    expect(`${os}: ${desc} is left to the page`, !r.prevented && r.calls.length === 0)
  }
  try {
    for (const [os, ua] of [
      ['Linux', LINUX],
      ['Windows', WINDOWS],
    ]) {
      const page = load(src, ua)
      expect(`${os}: one capturing keydown listener`, page.listeners.length === 1 && page.listeners[0].capture === true)
      reloads(os, page, 'Ctrl+R', 'r', { ctrl: true }, false)
      reloads(os, page, 'F5', 'F5', {}, false)
      reloads(os, page, 'Ctrl+Shift+R', 'R', { ctrl: true, shift: true }, true)
      reloads(os, page, 'Shift+F5', 'F5', { shift: true }, true)
      reloads(os, page, 'Ctrl+F5', 'F5', { ctrl: true }, true)
      reloads(os, page, 'Ctrl+R on a Cyrillic layout', 'к', { code: 'KeyR', ctrl: true }, false)
      ignores(os, page, 'Cmd+R (the Super key here)', 'r', { meta: true })
      ignores(os, page, 'Ctrl+Super+R', 'r', { ctrl: true, meta: true })
      ignores(os, page, 'Ctrl+Alt+R', 'r', { ctrl: true, alt: true })
      ignores(os, page, 'a plain r', 'r', {})
      ignores(os, page, 'a held Ctrl+R repeating', 'r', { ctrl: true, repeat: true })
      ignores(os, page, 'Ctrl+P on Dvorak (the KeyR position)', 'p', { code: 'KeyR', ctrl: true })
      ignores(os, page, 'Ctrl+Shift+H (the escape hatch)', 'H', { ctrl: true, shift: true })
    }
    const mac = load(src, MAC)
    reloads('macOS', mac, 'Cmd+R', 'r', { meta: true }, false)
    reloads('macOS', mac, 'Cmd+Shift+R', 'R', { meta: true, shift: true }, true)
    ignores('macOS', mac, 'Ctrl+R', 'r', { ctrl: true })
    ignores('macOS', mac, 'F5', 'F5', {})
    ignores('macOS', mac, 'Cmd+Ctrl+R', 'r', { meta: true, ctrl: true })
    ignores('macOS', mac, 'Cmd+Option+R', 'r', { meta: true, alt: true })

    // An origin the shell gives no grant (an error page from another host):
    // the command is refused and the page reloads by itself.
    const refused = load(src, LINUX, { refuse: true })
    const r = refused.press('F5')
    await settle()
    expect('a refused desktop_reload falls back to location.reload()', r.prevented && refused.reloads() === 1)
    const granted = load(src, LINUX)
    granted.press('F5')
    await settle()
    expect('a granted desktop_reload does not also reload the page itself', granted.reloads() === 0)

    // A webview without the shell's internals (never the case in the shell,
    // but the script must not throw there).
    let threw = null
    try {
      new Function('window', 'navigator', 'location', src)({}, { userAgent: LINUX }, { reload() {} })
    } catch (e) {
      threw = e
    }
    expect('the script does nothing outside the shell', threw === null)
  } catch (e) {
    wrong.push(`the script threw against the stub: ${e.message}`)
  }
  return wrong
}
