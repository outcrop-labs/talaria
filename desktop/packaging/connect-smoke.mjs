#!/usr/bin/env node
// The connect smoke: does the shell open an instance at all?
//
//   node desktop/packaging/connect-smoke.mjs <talaria-desktop binary> [--wait <s>] [--data-dir <dir>] [--force]
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
const force = argv.includes('--force')
if (force) argv.splice(argv.indexOf('--force'), 1)
const waitSeconds = Number(flag('--wait') ?? 30)
const dataDirFlag = flag('--data-dir')
const binary = argv[0] && resolve(argv[0])
if (!binary || !existsSync(binary) || !(waitSeconds > 0)) {
  console.error('usage: connect-smoke.mjs <talaria-desktop binary> [--wait <s>] [--data-dir <dir>] [--force]')
  process.exit(2)
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

const PAGE = `<!doctype html>
<meta charset="utf-8">
<title>Connect smoke</title>
<p>The connect smoke's instance page.</p>
<script>fetch('/opened?href=' + encodeURIComponent(location.href)).catch(() => {})</script>
`

const server = createServer((req, res) => {
  const path = (req.url ?? '/').split('?')[0]
  requests.push(`${at()} ${req.method} ${req.url} (${(req.headers['user-agent'] ?? 'no user agent').slice(0, 60)})`)
  if (!seen.has(path)) seen.set(path, launched ? (Date.now() - launched) / 1000 : 0)
  if (path === BEACON) {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ instance: INSTANCE_ID, companyName: 'Connect smoke' }))
  } else if (path === '/') {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end(PAGE)
  } else if (path === '/opened') {
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
// How it ended, read before the kill below ends it anyway.
const ended = exited
const alive = !ended

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

const pass = alive && seen.has('/') && seen.has('/opened')
if (pass) {
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
