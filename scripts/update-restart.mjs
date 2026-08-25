// The updater's restart half. Spawned DETACHED by ui/src/server/updater.ts
// right before the old server SIGTERMs itself, and it outlives that server:
// its whole job is to be the pair of hands on the other side of the restart.
//
//   1. wait for the old server to let go of the port (it drains its
//      scheduler jobs on the way out, so this can take a minute),
//   2. start `bun server-entry.js` from the ui/ directory, detached, with
//      the environment we inherited (which carries everything the dying
//      server had resolved at boot, ui/.env included),
//   3. wait for the port to answer again, say which way it went, exit.
//
// Deliberately dependency-free plain script: it runs when half the app is
// gone, so it may not import anything from ui/src. It speaks into
// logs/updater.log next to the server log it creates.
//
// Nothing here marks the update done in the database. Only the NEW server
// can do that (its first updater read reconciles the running state), which
// is the honest order: "done" means the replacement actually came up.
import net from 'node:net'
import { spawn } from 'node:child_process'
import { mkdirSync, openSync } from 'node:fs'
import { join } from 'node:path'

const uiDir = process.env.TALARIA_UPDATE_UI_DIR
const port = Number(process.env.TALARIA_UPDATE_PORT || 3000)
// Poll the address the server will actually bind, derived exactly the way
// server-entry.js derives it: HOST set to a real interface means 127.0.0.1
// never answers even though the server is fine.
const host = process.env.HOST && process.env.HOST !== '0.0.0.0' && process.env.HOST !== '::' ? process.env.HOST : '127.0.0.1'

const log = (message) => console.log(`[update-restart ${new Date().toISOString()}] ${message}`)
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

/** True the moment something is listening on the port. */
const portUp = () =>
  new Promise((resolve) => {
    const socket = net.connect(port, host)
    socket.once('connect', () => {
      socket.destroy()
      resolve(true)
    })
    socket.once('error', () => resolve(false))
  })

const waitUntil = async (wanted, timeoutMs, label) => {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if ((await portUp()) === wanted) return true
    await sleep(1000)
  }
  return false
}

if (!uiDir) {
  log('TALARIA_UPDATE_UI_DIR is not set; nothing to restart. This script is spawned by the updater, not run by hand.')
  process.exit(1)
}

// 1. The old server is draining. Give it up to 3 minutes; a heavy job set
//    takes longer than a light one, and giving up early just collides with
//    a port that is still held.
if (!(await waitUntil(false, 3 * 60_000, 'old server exit'))) {
  log(`The old server never let go of port ${port}. Not starting a second copy; the port is occupied. (Is something else supervised restarting it?)`)
  process.exit(1)
}

// 2. Start the replacement. Its stdout/stderr land in logs/talaria.log,
//    which is where every subsequent manual restart should look too.
const logDir = join(uiDir, '..', 'logs')
mkdirSync(logDir, { recursive: true })
const out = openSync(join(logDir, 'talaria.log'), 'a')
log(`starting bun server-entry.js in ${uiDir} (pid follows)`)
const child = spawn('bun', ['server-entry.js'], {
  cwd: uiDir,
  detached: true,
  stdio: ['ignore', out, out],
  env: process.env,
})
child.unref()
log(`started pid ${child.pid}`)

// 3. The new server applies any new migrations before it listens, so the
//    wait here is minutes, not seconds, on a release that touches the DB.
if (!(await waitUntil(true, 5 * 60_000, 'new server boot'))) {
  log(`The new server did not come up on port ${port} within 5 minutes. Read logs/talaria.log; the update is still marked running and the panel will say so.`)
  process.exit(1)
}

log(`new server is up on port ${port}`)
