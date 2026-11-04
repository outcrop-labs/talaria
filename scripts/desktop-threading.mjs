// Can an IPC command of the desktop shell create a webview inline?
//
// THE BUG THIS KEEPS FROM RETURNING. `activate_instance` was a sync
// `#[tauri::command]`. A sync command runs inside the calling webview's IPC
// callback, which on Windows is a WebView2 event handler, and it created the
// instance's WebView2 right there. WebView2 cannot hand over a new controller
// while one of its own handlers is on the stack, and wry waits for it in a
// message pump with no timeout: every first open of an instance froze Talaria
// Desktop on Windows for good. Linux and macOS create synchronously, so no
// review, no test and no rig run there could see it (docs/DESKTOP.md,
// "Threading").
//
// THE RULE, as desktop/src-tauri/src/commands.rs states it: a webview is made
// only in a main-thread turn of its own. A sync command may not reach webview
// creation (or a wait on WebView2, like a cookie read) at all; an async command
// may reach it only through a hop to the main thread (`on_main_turn`), because
// creating from a tokio worker splits hide → create → show across turns, which
// WebKitGTK's geometry depends on. A CALLBACK may not reach it either: a closure
// handed to `.on_*(…)` (window, webview, page-load, navigation, menu and tray
// events), to `.run(…)`, `.with_webview(…)` or a GTK `connect_*` signal, and any
// function marked `// webview-threading: callback` (the notification-click entry
// points the OS calls). On Windows a window event fires synchronously inside a
// sync command's window operation — inside a WebView2 callback — so a restore
// there must queue its turn (`queue_main_turn`), as every such path does now.
//
// HOW IT JUDGES. A crude call graph over desktop/src-tauri/src, by function
// name: comments and literals blanked, `#[cfg(test)]` modules skipped. A
// function CREATES when its body holds a creation (`CREATES`) or calls a
// function that creates. Code passed to a HOP runs elsewhere or later, so the
// caller does not reach it: tokio and thread spawns, and the shell's two
// main-turn helpers. `run_on_main_thread` is NOT a hop for code already on the
// main thread — Tauri runs the closure inline there — so a sync command that
// wraps the creation in it still reaches it. `block_on(on_main_turn(…))` on the
// main thread waits for a turn that cannot run until it returns: that counts as
// a creation too. Names are matched as free calls (`name(`, `path::name(`),
// never method calls (`.name(`).
//
// KNOWN LIMITS. Method-call edges are not followed (moving `activate` into an
// `impl` and calling it as `x.activate()` would drop the edge), macro bodies
// are read as plain text, and names are merged across modules. The misses are
// false negatives on a refactor of exactly these paths, which review and the
// connect smoke in CI back up; every false positive falls on the safe side.
//
// Stdlib only, like everything `bun run check` runs.

/** What counts as creating a webview, or waiting on WebView2 to answer. */
export const CREATES = [
  { re: /\.add_child\s*\(/g, what: '.add_child(' },
  { re: /\bWebviewBuilder::new\s*\(/g, what: 'WebviewBuilder::new(' },
  { re: /\bWebviewWindowBuilder::new\s*\(/g, what: 'WebviewWindowBuilder::new(' },
  { re: /\.cookies_for_url\s*\(/g, what: '.cookies_for_url(' },
  { re: /\.cookies\s*\(\s*\)/g, what: '.cookies()' },
  { re: /\bWebviewWindow::builder\s*\(/g, what: 'WebviewWindow::builder(' },
  { re: /\bWebview(?:Window)?Builder::from_config\s*\(/g, what: 'WebviewBuilder::from_config(' },
  { re: /\bblock_on\s*\(\s*on_main_turn\s*\(/g, what: 'block_on(on_main_turn(' },
]

/** Calls that hand a closure to the platform, to run later as a callback:
 *  a `.on_*(|…|` event handler, `.run(|…|`, `.with_webview(|…|`, a listener,
 *  a GTK `connect_*(|…|` signal or a glib idle. The closure is a root. */
const CALLBACKS = [
  /\.(on_[a-z_]+|run|with_webview)\s*\(\s*(?:move\s*)?\|/g,
  // A listener names its event first: `.listen("shell-state", |e| …)`.
  /\.(listen(?:_any)?|once(?:_any)?)\s*\(\s*"[^"]*"\s*,\s*(?:move\s*)?\|/g,
  /\b(connect_[a-z_]+|idle_add(?:_local)?(?:_once)?)\s*\(\s*(?:move\s*)?\|/g,
]

/** A function the OS or a platform library calls back, marked in the source. */
const MARKER = /\/\/\s*webview-threading:\s*callback\b/

/** Calls whose closure argument runs on another thread, or in a later
 *  main-thread turn — never inline in the caller. */
export const HOPS = ['spawn', 'spawn_blocking', 'queue_main_turn', 'on_main_turn']

/** Inline on the main thread, queued from anywhere else. A hop only for an
 *  async command's own body, which runs on a tokio worker. */
const MAIN_THREAD = 'run_on_main_thread'

/** `src` with comments and the contents of string and char literals replaced
 *  by spaces, newlines kept: offsets and line numbers stay true, and a brace
 *  or a pattern inside a string or a doc comment no longer counts. */
export function blankRust(src) {
  const out = src.split('')
  const blank = (from, to) => {
    for (let k = from; k < to && k < out.length; k++) if (out[k] !== '\n') out[k] = ' '
  }
  const word = (k) => k >= 0 && /\w/.test(src[k])
  const n = src.length
  let i = 0
  while (i < n) {
    const c = src[i]
    const d = src[i + 1]
    if (c === '/' && d === '/') {
      const end = src.indexOf('\n', i)
      const j = end === -1 ? n : end
      blank(i, j)
      i = j
      continue
    }
    if (c === '/' && d === '*') {
      let depth = 1
      let j = i + 2
      while (j < n && depth > 0) {
        if (src[j] === '/' && src[j + 1] === '*') (depth++, (j += 2))
        else if (src[j] === '*' && src[j + 1] === '/') (depth--, (j += 2))
        else j++
      }
      blank(i, j)
      i = j
      continue
    }
    const raw = word(i - 1) ? null : /^[bc]?r(#*)"/.exec(src.slice(i, i + 64))
    if (raw) {
      const close = '"' + raw[1]
      const start = i + raw[0].length
      const end = src.indexOf(close, start)
      const j = end === -1 ? n : end
      blank(start, j)
      i = j + close.length
      continue
    }
    if (c === '"' || ((c === 'b' || c === 'c') && d === '"' && !word(i - 1))) {
      const start = c === '"' ? i + 1 : i + 2
      let j = start
      while (j < n && src[j] !== '"') j += src[j] === '\\' ? 2 : 1
      blank(start, j)
      i = j + 1
      continue
    }
    if (c === "'") {
      // A char literal ('x', '\n', '\'', '\u{..}', a surrogate pair), or a
      // lifetime ('a, 'static), which is left alone.
      if (d === '\\') {
        let j = i + 3
        while (j < n && src[j] !== "'" && src[j] !== '\n') j++
        blank(i + 1, j)
        i = j + 1
        continue
      }
      const close = src[i + 2] === "'" ? i + 2 : src[i + 3] === "'" && /[\ud800-\udbff]/.test(d) ? i + 3 : -1
      if (close !== -1) {
        blank(i + 1, close)
        i = close + 1
        continue
      }
    }
    i++
  }
  return out.join('')
}

/** The index just past the bracket matching the one at `open`. */
function matching(text, open) {
  const pairs = { '(': ')', '{': '}', '[': ']' }
  const want = pairs[text[open]]
  let depth = 0
  for (let k = open; k < text.length; k++) {
    if (text[k] === text[open]) depth++
    else if (text[k] === want && --depth === 0) return k + 1
  }
  return text.length
}

/** Spaces over the argument list of every call to one of `names`. */
function blankCalls(text, names) {
  const re = new RegExp(String.raw`\b(?:${names.join('|')})\s*\(`, 'g')
  let out = text
  let m
  while ((m = re.exec(text)) !== null) {
    const open = m.index + m[0].length - 1
    const close = matching(text, open)
    out = out.slice(0, open + 1) + out.slice(open + 1, close - 1).replace(/[^\n]/g, ' ') + out.slice(close - 1)
  }
  return out
}

const lineAt = (text, index) => text.slice(0, index).split('\n').length

/** Every function with a body: name, async, whether it is a Tauri command,
 *  and its body text. */
export function rustFns(path, blanked, raw = blanked) {
  const skip = []
  for (const m of blanked.matchAll(/#\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{/g)) {
    const open = m.index + m[0].length - 1
    skip.push([open, matching(blanked, open)])
  }
  const fns = []
  for (const m of blanked.matchAll(/\bfn\s+([A-Za-z_]\w*)/g)) {
    if (skip.some(([s, e]) => m.index > s && m.index < e)) continue
    let k = m.index + m[0].length
    let depth = 0 // parens and brackets: `-> [u8; 4]` is still a signature
    for (; k < blanked.length; k++) {
      const ch = blanked[k]
      if (ch === '(' || ch === '[') depth++
      else if (ch === ')' || ch === ']') depth--
      else if (depth === 0 && (ch === '{' || ch === ';')) break
    }
    if (blanked[k] !== '{') continue // a declaration with no body
    const before = blanked.slice(0, m.index)
    const from = Math.max(before.lastIndexOf('}'), before.lastIndexOf(';'), before.lastIndexOf('{')) + 1
    const prefix = before.slice(from)
    const bodyStart = k + 1
    const body = blanked.slice(bodyStart, matching(blanked, k) - 1)
    // The closures this function hands to the platform, each a root of its own.
    const callbacks = []
    for (const re of CALLBACKS) {
      re.lastIndex = 0
      for (const c of body.matchAll(re)) {
        const open = c.index + c[0].indexOf('(')
        callbacks.push({
          via: c[0].startsWith('.') ? `.${c[1]}(` : `${c[1]}(`,
          line: lineAt(blanked, bodyStart + c.index),
          text: body.slice(open + 1, matching(body, open) - 1),
        })
      }
    }
    fns.push({
      path,
      name: m[1],
      line: lineAt(blanked, m.index),
      async: /\basync\s+(?:unsafe\s+)?$/.test(prefix),
      command: /#\[\s*tauri\s*::\s*command\b/.test(prefix),
      marked: MARKER.test(raw.slice(from, m.index)),
      body,
      callbacks,
    })
  }
  return fns
}

/**
 * The commands that can create a webview where they must not.
 *
 * `files` maps a repo-relative path to its raw Rust source. Returns
 * `{ violations, commands, creators }`: each violation names the command, where
 * it is, and the chain of calls down to the creation.
 */
export function scanWebviewThreading(files) {
  const fns = []
  for (const [path, src] of files) fns.push(...rustFns(path, blankRust(src), src))
  const names = [...new Set(fns.map((f) => f.name))]
  const callRe = new RegExp(String.raw`(?<![\w.])(?<!\bfn\s+)(${names.join('|')})\s*(?:::<[^>]*>\s*)?\(`, 'g')
  const sinkIn = (text) => {
    for (const c of CREATES) {
      c.re.lastIndex = 0
      if (c.re.test(text)) return c.what
    }
    return null
  }
  const callsIn = (text) => (names.length ? [...new Set([...text.matchAll(callRe)].map((m) => m[1]))] : [])

  // What each function reaches when it runs on the main thread.
  const main = new Map() // name → { sink, calls } over every definition of it
  for (const f of fns) {
    const text = blankCalls(f.body, HOPS)
    const entry = main.get(f.name) ?? { sink: null, calls: new Set() }
    entry.sink ??= sinkIn(text)
    for (const c of callsIn(text)) entry.calls.add(c)
    main.set(f.name, entry)
  }
  // Fixed point: via[name] = the creation itself, or the callee that reaches it.
  const via = new Map()
  for (const [name, e] of main) if (e.sink) via.set(name, { sink: e.sink })
  for (let changed = true; changed; ) {
    changed = false
    for (const [name, e] of main) {
      if (via.has(name)) continue
      const callee = [...e.calls].find((c) => via.has(c))
      if (callee) (via.set(name, { callee }), (changed = true))
    }
  }
  const chain = (name) => {
    const out = [name]
    const seen = new Set([name])
    let at = via.get(name)
    while (at && at.callee && !seen.has(at.callee)) {
      out.push(at.callee)
      seen.add(at.callee)
      at = via.get(at.callee)
    }
    if (at?.sink) out.push(at.sink)
    return out
  }

  const violations = []
  // A callback runs on the main thread (or wherever the platform calls it),
  // inline: judged as a sync command is.
  const reachesInline = (text) => {
    const body = blankCalls(text, HOPS)
    const sink = sinkIn(body)
    if (sink) return [sink]
    const callee = callsIn(body).find((c) => via.has(c))
    return callee ? chain(callee) : null
  }
  let callbacks = 0
  for (const f of fns) {
    if (f.marked) {
      callbacks++
      const reach = reachesInline(f.body)
      if (reach) violations.push({ path: f.path, line: f.line, command: f.name, kind: 'callback', chain: [f.name, ...reach] })
    }
    for (const c of f.callbacks) {
      callbacks++
      const reach = reachesInline(c.text)
      if (reach) {
        violations.push({ path: f.path, line: c.line, command: `${f.name}/${c.via}`, kind: 'callback', chain: [`${f.name}: ${c.via}…)`, ...reach] })
      }
    }
  }
  const commands = fns.filter((f) => f.command)
  for (const f of commands) {
    if (!f.async) {
      if (via.has(f.name)) {
        violations.push({ path: f.path, line: f.line, command: f.name, kind: 'sync', chain: chain(f.name) })
      }
      continue
    }
    // An async command's own body runs on a tokio worker: there,
    // run_on_main_thread queues, but a direct creation is a worker creating.
    const text = blankCalls(f.body, [...HOPS, MAIN_THREAD])
    const sink = sinkIn(text)
    const callee = callsIn(text).find((c) => via.has(c))
    if (sink || callee) {
      violations.push({
        path: f.path,
        line: f.line,
        command: f.name,
        kind: 'worker',
        chain: sink ? [f.name, sink] : [f.name, ...chain(callee)],
      })
    }
  }
  return { violations, commands: commands.length, callbacks, creators: [...via.keys()].filter((n) => main.get(n)?.sink) }
}

/** The tripwire run against a fixture it must catch, so a scanner that has
 *  quietly stopped seeing anything fails instead of passing. Returns what it
 *  got wrong (empty: it works). */
export function selfTest() {
  const fixture = `
    /// A doc comment that says .add_child( must not count.
    #[tauri::command]
    pub fn sync_direct(app: AppHandle) -> Result<(), String> { make(&app) }
    fn make(app: &AppHandle) -> Result<(), String> {
        let s = "a } brace in a string";
        let _ = window.add_child(builder, rect.position, rect.size);
        Ok(())
    }
    #[tauri::command]
    pub async fn async_hop(app: AppHandle) -> Result<(), String> {
        on_main_turn(&app, move |app| make(app)).await?
    }
    #[tauri::command]
    pub async fn async_worker(app: AppHandle) -> Result<(), String> { make(&app) }
    #[tauri::command]
    pub fn sync_spawned(app: AppHandle) {
        tauri::async_runtime::spawn(async move { let _ = app.run_on_main_thread(move || { let _ = make(&app); }); });
    }
    #[tauri::command]
    pub fn sync_inline_main(app: AppHandle) { let _ = app.run_on_main_thread(move || { let _ = make(&app); }); }
    #[tauri::command]
    pub fn sync_queued(app: AppHandle) { queue_main_turn(&app, move |app| { let _ = make(app); }); }
    #[tauri::command]
    pub fn sync_cookies(webview: Webview) { let _ = webview.cookies(); }
    #[tauri::command]
    pub fn sync_block_on(app: AppHandle) { let _ = tauri::async_runtime::block_on(on_main_turn(&app, |_| ())); }
    fn wire_window(builder: Builder, app: AppHandle) -> [u8; 4] {
        builder.on_window_event(move |window, event| { let _ = make(&app); });
        [0; 4]
    }
    fn wire_tray(builder: TrayIconBuilder) {
        builder.on_menu_event(move |app, event| queue_main_turn(app, |app| { let _ = make(app); }));
        let _ = settings.connect_timeout(CONNECT_TIMEOUT);
    }
    fn wire_more(app: AppHandle, webview: Webview, application: Application) {
        webview.with_webview(move |platform| { let _ = make(&app); });
        app.listen("shell-state", move |event| { let _ = make(&app); });
        application.connect_activate(move |_| { let _ = make(&app); });
        glib::idle_add_local_once(move || { let _ = make(&app); });
        app.run(|handle, event| { queue_main_turn(handle, |app| { let _ = make(app); }); });
    }
    #[tauri::command]
    pub fn sync_window_builder(app: AppHandle) { let _ = WebviewWindow::builder(&app, "x", url).build(); }
    #[tauri::command]
    pub fn sync_from_config(app: AppHandle) { let _ = WebviewBuilder::from_config(&config); }
    // webview-threading: callback
    pub(crate) fn clicked(app: &AppHandle) { let _ = make(app); }
    // webview-threading: callback
    pub(crate) fn clicked_queued(app: &AppHandle) { queue_main_turn(app, |app| { let _ = make(app); }); }
    #[cfg(test)]
    mod tests {
        #[tauri::command]
        pub fn in_a_test(app: AppHandle) { make(&app); }
    }
  `
  const got = scanWebviewThreading(new Map([['fixture.rs', fixture]]))
  const flagged = got.violations.map((v) => `${v.command}:${v.kind}`).sort()
  const want = [
    'async_worker:worker',
    'clicked:callback',
    'sync_block_on:sync',
    'sync_cookies:sync',
    'sync_direct:sync',
    'sync_from_config:sync',
    'sync_inline_main:sync',
    'sync_window_builder:sync',
    'wire_more/.listen(:callback',
    'wire_more/.with_webview(:callback',
    'wire_more/connect_activate(:callback',
    'wire_more/idle_add_local_once(:callback',
    'wire_window/.on_window_event(:callback',
  ]
  const wrong = []
  if (flagged.join() !== want.join()) wrong.push(`flagged ${flagged.join(', ') || 'nothing'}; want ${want.join(', ')}`)
  const direct = got.violations.find((v) => v.command === 'sync_direct')
  if (direct && direct.chain.join(' → ') !== 'sync_direct → make → .add_child(') {
    wrong.push(`the chain reads ${direct.chain.join(' → ')}`)
  }
  return wrong
}
