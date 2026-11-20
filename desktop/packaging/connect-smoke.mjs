#!/usr/bin/env node
// The connect smoke: does the shell open an instance at all?
//
//   node desktop/packaging/connect-smoke.mjs <talaria-desktop binary> [--wait <s>] [--data-dir <dir>] [--force] [--drag-icon]
//
// WHY IT EXISTS. On Windows, every first open of an instance froze Talaria
// Desktop for good, and nothing in CI could see it. `activate_instance` was a
// sync command, so it ran inside the launcher's WebView2 IPC callback, and it
// created the instance's WebView2 there; WebView2 cannot finish creating a
// controller while one of its own handlers is on the stack, and wry waits for
// it in a message pump with no timeout. The process stays up and keeps
// repainting, so "is it still running after 20 s" (desktop-package.yml's
// installer smoke) passed on a frozen app. This one passes only when the
// instance page itself is requested, and its script runs.
//
// WHAT IT DRIVES. A fake instance on 127.0.0.1 (a random port) serves the
// beacon (`/api/well-known/talaria-instance`) and a page at `/` whose script
// fetches `/opened`. The shell's registry (`instances.json` in its data dir)
// is written with that instance as the last view, so the launcher reopens it
// at boot — the whole connect path: boot → `probe_instance` (the beacon) →
// `activate_instance` → a new instance webview → its navigation to `/` →
// `/opened`. Pass: both requests inside the wait (30 s by default), with the
// shell still running. Anything else fails, and says which step was the last
// one that happened.
//
// It writes into the shell's real data dir, so it refuses to replace an
// existing registry unless `--force` is passed: run it on a throwaway runner,
// or under a private HOME. Node stdlib only.
//
// `--drag-icon` (Linux, X11, GDK_SCALE of 2 or more): then, does a drag's
// ghost come out at the web size? WebKitGTK hands GTK its drag image in device
// pixels and never says so, so at a display scale s above 1 the ghost was s
// times the size a browser shows, until the shell's shim scaled it
// (desktop/src-tauri/src/drag_icon.rs; docs/DESKTOP.md, "Drag images on a
// HiDPI Linux display"). Once the page has run, this phase presses the
// mouse with XTest (xdotool) inside a 240×120 CSS px `draggable` box in the
// page, moves it 4 × 10 px, and waits for the page's `dragstart`. Then it
// finds GDK's drag window, the mapped root child whose
// `_NET_WM_WINDOW_TYPE` is `_NET_WM_WINDOW_TYPE_DND` (xprop, xwininfo),
// which is created at 100×100 and resized once the icon is set, so it is read
// until its geometry holds still. It passes when that window is the box's
// size × devicePixelRatio (±2 device px) and sits at the pointer minus the
// press point's offset in the box × devicePixelRatio (±2 px): the right size,
// with the cursor still on the point grabbed. The page reports
// devicePixelRatio, which must equal GDK_SCALE. The box is placed at whole CSS
// px with no border, shadow, outline or overflow, because WebKit snapshots the
// element's painted rect while the hotspot is measured from its border box:
// any of those would grow or shift the image by a CSS px (2 device px at
// scale 2). The page also sets the Talaria UI's shell hook, so the shell
// keeps it up instead of calling it "not Talaria" mid-drag. Missing DISPLAY,
// xdotool, xwininfo or xprop, or a GDK_SCALE below 2, exits 2 before
// anything is launched.

import { spawn, spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { homedir } from 'node:os'
import { join, resolve } from 'node:path'

const IDENTIFIER = 'app.talaria.desktop'
const LOCAL_ID = '5e0c0de5-0000-4000-8000-00000000c0de'
const INSTANCE_ID = '5e0c0de5-0000-4000-8000-0000000be4c0'
const BEACON = '/api/well-known/talaria-instance'

const argv = process.argv.slice(2)
const flag = (name) => {
  const i = argv.indexOf(name)
  if (i === -1) return undefined
  const value = argv[i + 1]
  argv.splice(i, 2)
  return value
}
const takeSwitch = (name) => {
  const i = argv.indexOf(name)
  if (i !== -1) argv.splice(i, 1)
  return i !== -1
}
const force = takeSwitch('--force')
const dragIcon = takeSwitch('--drag-icon')
const waitSeconds = Number(flag('--wait') ?? 30)
const dataDirFlag = flag('--data-dir')
const binary = argv[0] && resolve(argv[0])
if (!binary || !existsSync(binary) || !(waitSeconds > 0)) {
  console.error('usage: connect-smoke.mjs <talaria-desktop binary> [--wait <s>] [--data-dir <dir>] [--force] [--drag-icon]')
  process.exit(2)
}

/** A short command whose output is the answer (xdotool, xwininfo, xprop). */
function tool(cmd, args) {
  const r = spawnSync(cmd, args, { encoding: 'utf8', timeout: 5000 })
  return { ok: r.status === 0, out: (r.stdout ?? '').trim(), err: ((r.stderr ?? '').trim() || r.error?.message) ?? '' }
}

const gdkScale = Number(process.env.GDK_SCALE)
if (dragIcon) {
  const missing = []
  if (process.platform !== 'linux') missing.push('Linux')
  if (!process.env.DISPLAY) missing.push('an X display (DISPLAY)')
  for (const cmd of ['xdotool', 'xwininfo', 'xprop']) {
    if (spawnSync(cmd, ['-version'], { stdio: 'ignore' }).error) missing.push(cmd)
  }
  if (!(Number.isInteger(gdkScale) && gdkScale >= 2)) missing.push('GDK_SCALE=2 or more (a scale of 1 cannot show the bug)')
  if (missing.length) {
    console.error(`--drag-icon needs ${missing.join(', ')}`)
    process.exit(2)
  }
}

/** Where tauri's app_data_dir and app_log_dir put the registry and the log. */
function platformDirs() {
  const home = homedir()
  if (process.platform === 'win32') {
    return {
      data: join(process.env.APPDATA ?? join(home, 'AppData', 'Roaming'), IDENTIFIER),
      log: join(process.env.LOCALAPPDATA ?? join(home, 'AppData', 'Local'), IDENTIFIER, 'logs'),
    }
  }
  if (process.platform === 'darwin') {
    return {
      data: join(home, 'Library', 'Application Support', IDENTIFIER),
      log: join(home, 'Library', 'Logs', IDENTIFIER),
    }
  }
  const share = process.env.XDG_DATA_HOME || join(home, '.local', 'share')
  return { data: join(share, IDENTIFIER), log: join(share, IDENTIFIER, 'logs') }
}
const dirs = platformDirs()
const dataDir = dataDirFlag ? resolve(dataDirFlag) : dirs.data
const registryPath = join(dataDir, 'instances.json')
if (existsSync(registryPath) && !force) {
  console.error(`${registryPath} exists: this would replace a real registry. Pass --force on a throwaway machine.`)
  process.exit(2)
}

const t0 = Date.now()
const at = () => `+${((Date.now() - t0) / 1000).toFixed(1)}s`
const seen = new Map() // path → seconds since launch, first request only
const requests = []
let launched = 0

const seenQuery = new Map() // path → the query of its latest request
// The box `--drag-icon` drags (the header says why it is placed and styled
// this way). Every page has it; only that phase touches it.
const BOX = { width: 240, height: 120 }
// With --drag-icon only: the page answers as a Talaria UI that is up
// (desktop/src-tauri/src/loading.rs, PAGE_JS). Otherwise the shell judges it
// "not Talaria" about 3 s after it loads and goes back to the launcher, which
// can land in the middle of the drag. The plain connect smoke keeps the page
// as it always was.
const TALARIA_UI_UP = '\n  window.__TALARIA_DESKTOP__ = {}'
const PAGE = `<!doctype html>
<meta charset="utf-8">
<title>Connect smoke</title>
<style>
  html, body { margin: 0; overflow: hidden; }
  #drag-box {
    position: fixed; box-sizing: border-box; width: ${BOX.width}px; height: ${BOX.height}px;
    margin: 0; padding: 0; border: 0; outline: none; box-shadow: none; overflow: hidden;
    background: #3a6ea5; color: #fff; font: 16px/${BOX.height}px sans-serif; text-align: center;
    user-select: none; -webkit-user-select: none;
  }
</style>
<p>The connect smoke's instance page.</p>
<div id="drag-box" draggable="true">drag smoke</div>
<script>${dragIcon ? TALARIA_UI_UP : ''}
  const report = (path, values) => fetch(path + '?' + new URLSearchParams(values)).catch(() => {})
  const box = document.getElementById('drag-box')
  // Centred at whole CSS px: never a transform, never a fraction.
  const place = () => {
    box.style.left = Math.round((innerWidth - ${BOX.width}) / 2) + 'px'
    box.style.top = Math.round((innerHeight - ${BOX.height}) / 2) + 'px'
  }
  const rect = () => {
    const r = box.getBoundingClientRect()
    return { left: r.left, top: r.top, width: r.width, height: r.height, dpr: devicePixelRatio }
  }
  place()
  addEventListener('resize', place)
  document.addEventListener('mousedown', (e) => {
    report('/press', { x: e.clientX, y: e.clientY, onBox: e.target === box, ...rect() })
  }, true)
  box.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData('text/plain', 'drag smoke')
    e.dataTransfer.effectAllowed = 'move'
    report('/dragstart', rect())
  })
  box.addEventListener('dragend', () => report('/dragend', {}))
  fetch('/opened?href=' + encodeURIComponent(location.href)).catch(() => {})
</script>
`

const server = createServer((req, res) => {
  const [path, query = ''] = (req.url ?? '/').split('?')
  requests.push(`${at()} ${req.method} ${req.url} (${(req.headers['user-agent'] ?? 'no user agent').slice(0, 60)})`)
  if (!seen.has(path)) seen.set(path, launched ? (Date.now() - launched) / 1000 : 0)
  seenQuery.set(path, new URLSearchParams(query))
  if (path === BEACON) {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ instance: INSTANCE_ID, companyName: 'Connect smoke' }))
  } else if (path === '/') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end(PAGE)
  } else if (['/opened', '/press', '/dragstart', '/dragend'].includes(path)) {
    res.writeHead(204)
    res.end()
  } else {
    res.writeHead(404)
    res.end()
  }
})
await new Promise((ok) => server.listen(0, '127.0.0.1', ok))
const origin = `http://127.0.0.1:${server.address().port}`

mkdirSync(dataDir, { recursive: true })
writeFileSync(
  registryPath,
  JSON.stringify(
    {
      instances: [
        {
          id: LOCAL_ID,
          instanceId: INSTANCE_ID,
          label: 'Connect smoke',
          url: origin,
          createdAt: Date.now(),
          lastOpenedAt: null,
        },
      ],
      lastView: LOCAL_ID,
    },
    null,
    2,
  ),
)
console.log(`fake instance at ${origin}; registry written to ${registryPath}`)

const output = []
launched = Date.now()
const child = spawn(binary, [], {
  stdio: ['ignore', 'pipe', 'pipe'],
  // Its own process group on POSIX, so the kill below takes the webview
  // processes with it. Windows: taskkill /T does that.
  detached: process.platform !== 'win32',
})
let exited = null
child.on('exit', (code, signal) => {
  exited = { code, signal, at: at() }
})
for (const stream of [child.stdout, child.stderr]) {
  let rest = ''
  stream.on('data', (chunk) => {
    const lines = (rest + chunk).split(/\r?\n/)
    rest = lines.pop()
    for (const line of lines) output.push(`${at()} ${line}`)
  })
}
console.log(`launched ${binary} (pid ${child.pid}); waiting up to ${waitSeconds} s for ${origin}/ and /opened`)

const deadline = launched + waitSeconds * 1000
while (Date.now() < deadline && !exited && !(seen.has('/') && seen.has('/opened'))) {
  await new Promise((ok) => setTimeout(ok, 250))
}
// Still running a moment after its page answered: an open that crashes the
// shell right after is no pass either.
if (seen.has('/opened') && !exited) await new Promise((ok) => setTimeout(ok, 2000))
// The drag, while the shell still runs.
const drag = dragIcon && seen.has('/opened') && !exited ? await dragPhase() : null
// How it ended, read before the kill below ends it anyway.
const ended = exited
const alive = !ended

async function waitFor(path, ms) {
  const until = Date.now() + ms
  while (Date.now() < until && !exited && !seen.has(path)) await new Promise((ok) => setTimeout(ok, 50))
  return seen.has(path)
}

/** xwininfo's view of one X window: root position, size, map state. */
function geometry(id) {
  const r = tool('xwininfo', ['-id', String(id), '-int'])
  if (!r.ok) return null
  const num = (label) => Number(r.out.match(new RegExp(`${label}:\\s+(-?\\d+)`))?.[1])
  return {
    id,
    x: num('Absolute upper-left X'),
    y: num('Absolute upper-left Y'),
    w: num('Width'),
    h: num('Height'),
    viewable: /Map State:\s+IsViewable/.test(r.out),
  }
}
// Declarations, not consts: the phase runs from a top-level await above them.
function describe(g) {
  return `${g.id} ${g.w}x${g.h}+${g.x}+${g.y}${g.viewable ? '' : ' (unmapped)'}`
}
function byArea(a, b) {
  return b.w * b.h - a.w * a.h
}

/** The ids of the root window's children (xvfb runs no window manager, so a
 *  toplevel is one, unreparented). */
function rootChildren() {
  const r = tool('xwininfo', ['-root', '-children', '-int'])
  const list = r.out.split(/\n\s*\d+ child(?:ren)?:\n/)[1] ?? ''
  return [...list.matchAll(/^\s*(\d+)\s/gm)].map((m) => Number(m[1]))
}

/** Every root child typed `_NET_WM_WINDOW_TYPE_DND`: GDK's drag window. */
function dndWindows() {
  const found = []
  for (const id of rootChildren()) {
    const type = tool('xprop', ['-id', String(id), '_NET_WM_WINDOW_TYPE'])
    if (!type.ok || !/_NET_WM_WINDOW_TYPE_DND\b/.test(type.out)) continue
    const g = geometry(id)
    if (g) found.push(g)
  }
  return found
}

/** The `--drag-icon` phase (the header says what it proves). */
async function dragPhase() {
  const log = []
  const step = (line) => log.push(`${at()} ${line}`)
  const fail = (why) => ({ pass: false, why, log })
  const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms))
  const ids = tool('xdotool', ['search', '--onlyvisible', '--pid', String(child.pid)]).out.split(/\s+/).filter(Boolean)
  const win = (ids.length ? ids : rootChildren()).map(geometry).filter((g) => g?.viewable).sort(byArea)[0]
  if (!win) return fail("the shell's window is not on the X display")
  step(`the shell's window: ${describe(win)}`)
  const cx = win.x + Math.round(win.w / 2)
  const cy = win.y + Math.round(win.h / 2)
  tool('xdotool', ['mousemove', '--sync', String(cx), String(cy)])
  await sleep(300)
  tool('xdotool', ['mousedown', '1'])
  step(`pressed at ${cx},${cy}`)
  try {
    if (!(await waitFor('/press', 5000))) return fail('the page never saw the press (no mousedown)')
    const q = seenQuery.get('/press')
    const n = (key) => Number(q.get(key))
    const box = { left: n('left'), top: n('top'), width: n('width'), height: n('height') }
    const grab = { x: n('x') - box.left, y: n('y') - box.top }
    const dpr = n('dpr')
    step(`press: page point ${n('x')},${n('y')}; box ${box.width}×${box.height} at ${box.left},${box.top}; devicePixelRatio ${dpr}`)
    if (q.get('onBox') !== 'true') return fail(`the press at ${cx},${cy} landed outside the box`)
    if (dpr !== gdkScale) return fail(`the page's devicePixelRatio is ${dpr}, not GDK_SCALE (${gdkScale})`)
    for (let i = 0; i < 4; i++) {
      tool('xdotool', ['mousemove_relative', '--sync', '0', '10'])
      await sleep(100)
    }
    if (!(await waitFor('/dragstart', 5000))) return fail('the page never started a drag (no dragstart)')
    step('dragstart')
    // GDK makes the drag window 100×100 (logical) at drag start and resizes it
    // to the icon; motion moves it. Read it until it holds still at a size
    // that is not the initial one.
    const initial = 100 * gdkScale
    let icon = null
    let last = ''
    let still = 0
    const until = Date.now() + 10_000
    while (Date.now() < until && !exited) {
      const all = dndWindows()
      const mapped = all.filter((g) => g.viewable).sort(byArea)
      const top = mapped[0]
      const key = top ? describe(top) : `none mapped (${all.map(describe).join(', ') || 'no DND window'})`
      if (key === last) still++
      else {
        still = 0
        step(`drag window: ${key}${mapped.length > 1 ? `, the biggest of ${mapped.length}` : ''}`)
      }
      last = key
      if (top && !(top.w === initial && top.h === initial) && still >= 3) {
        icon = top
        break
      }
      await sleep(150)
    }
    if (!icon) return fail(`no drag window settled at an icon's size (last: ${last})`)
    const loc = tool('xdotool', ['getmouselocation', '--shell']).out
    const coord = (k) => Number(loc.match(new RegExp(`^${k}=(-?\\d+)`, 'm'))?.[1])
    const pointer = { x: coord('X'), y: coord('Y') }
    const want = {
      w: box.width * dpr,
      h: box.height * dpr,
      x: pointer.x - grab.x * dpr,
      y: pointer.y - grab.y * dpr,
    }
    const ratio = icon.w / want.w
    step(
      `pointer ${pointer.x},${pointer.y}, grabbed ${grab.x},${grab.y} CSS px into the box: ` +
        `want ${want.w}x${want.h}+${want.x}+${want.y}, found ${icon.w}x${icon.h}+${icon.x}+${icon.y} (size ratio ${ratio.toFixed(2)})`,
    )
    const near = (a, b) => Math.abs(a - b) <= 2
    if (!near(icon.w, want.w) || !near(icon.h, want.h)) {
      let cause = 'an unexpected size'
      if (Math.abs(ratio - dpr) < 0.05)
        cause =
          "WebKitGTK's device-pixel image shown as logical px: the shim did not run " +
          '(the "drag icon shim" step checks its export and its binding)'
      else if (Math.abs(ratio - 1 / dpr) < 0.05)
        cause =
          'scaled down twice: this WebKitGTK already sizes the image itself in a way the shim cannot see, ' +
          'so the shim must stand down for it (docs/DESKTOP.md, "Drag images on a HiDPI Linux display")'
      return { pass: false, why: `the drag ghost is ${ratio.toFixed(2)}× the web size: ${cause}`, ratio, log }
    }
    if (!near(icon.x, want.x) || !near(icon.y, want.y))
      return {
        pass: false,
        why:
          `the drag ghost is the web size but sits at ${icon.x},${icon.y}, not ${want.x},${want.y}: ` +
          'the cursor is off the point grabbed (a hotspot offset not scaled with the image)',
        ratio,
        log,
      }
    return { pass: true, why: `the drag ghost is the web size (${icon.w}×${icon.h} device px for a ${box.width}×${box.height} CSS px box at scale ${dpr}) under the point grabbed`, ratio, log }
  } finally {
    tool('xdotool', ['mouseup', '1'])
    await waitFor('/dragend', 2000)
  }
}

function kill() {
  if (ended) return
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' })
  } else {
    try {
      process.kill(-child.pid, 'SIGKILL')
    } catch {
      child.kill('SIGKILL')
    }
  }
}
kill()
await new Promise((ok) => setTimeout(ok, 1000))
server.close()

const when = (path) => (seen.has(path) ? `${seen.get(path).toFixed(1)} s after launch` : 'never')
console.log('\nrequests to the fake instance:')
for (const line of requests) console.log(`  ${line}`)
if (!requests.length) console.log('  (none)')
console.log(`\nbeacon probed: ${when(BEACON)}`)
console.log(`instance page requested: ${when('/')}`)
console.log(`page script ran (/opened): ${when('/opened')}`)
console.log(`shell ${alive ? 'still running at the end' : `exited ${ended.at} (code ${ended.code}, signal ${ended.signal})`}`)

// The disk evidence a frozen open leaves: the instance's data dir exists (it
// is made before the webview), while lastOpenedAt stays null (written after).
const webviewDir = join(dataDir, 'instances', LOCAL_ID, 'webview')
console.log(`\n${webviewDir}: ${existsSync(webviewDir) ? 'exists' : 'absent'}`)
try {
  const after = JSON.parse(readFileSync(registryPath, 'utf8'))
  const entry = (after.instances ?? []).find((i) => i.id === LOCAL_ID)
  console.log(`registry after: lastView ${JSON.stringify(after.lastView)}, lastOpenedAt ${JSON.stringify(entry?.lastOpenedAt ?? null)}`)
} catch (e) {
  console.log(`registry after: unreadable (${e.message})`)
}

console.log(`\nthe shell's own output (${output.length} lines):`)
for (const line of output.slice(-80)) console.log(`  ${line}`)
const logFile = join(dirs.log, 'talaria-desktop.log')
if (existsSync(logFile)) {
  const lines = readFileSync(logFile, 'utf8').split(/\r?\n/).filter(Boolean)
  console.log(`\nthe shell's log file, ${logFile} (last ${Math.min(lines.length, 40)} of ${lines.length} lines):`)
  for (const line of lines.slice(-40)) console.log(`  ${line}`)
} else {
  console.log(`\nno log file at ${logFile}`)
}

if (drag) {
  console.log('\nthe drag (--drag-icon):')
  for (const line of drag.log) console.log(`  ${line}`)
  const said = output.filter((line) => line.includes('drag images:'))
  console.log(`the shell's drag-image line: ${said.length ? said.join(' | ') : 'none'}`)
}

const connected = alive && seen.has('/') && seen.has('/opened')
if (connected && dragIcon) {
  if (drag?.pass) {
    console.log(`\nPASS: the shell opened the instance and ${drag.why}`)
    process.exit(0)
  }
  console.log(`\nFAIL: the shell opened the instance, but ${drag?.why ?? 'the drag never ran'}`)
  process.exit(1)
}
if (connected) {
  console.log(`\nPASS: the shell opened the instance (page ${when('/')}, script ${when('/opened')}) and kept running`)
  process.exit(0)
}
let why
if (ended && !seen.has('/opened')) why = 'the shell exited before the instance page ran'
else if (ended) why = 'the shell exited right after opening the instance'
else if (!seen.has(BEACON)) why = 'the launcher never probed the instance: it did not boot, or did not read the registry'
else if (!seen.has('/'))
  why =
    'the launcher probed the instance, but its page was never requested: the shell hung creating the instance ' +
    'webview (on Windows, a webview created inside an IPC callback deadlocks — docs/DESKTOP.md, "Threading")'
else why = 'the instance page was requested but its script never ran'
console.log(`\nFAIL: ${why}`)
process.exit(1)
