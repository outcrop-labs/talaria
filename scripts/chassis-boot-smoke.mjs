#!/usr/bin/env node
// BOOT THE CHASSIS FOR REAL, then stop it and check HOW it stopped.
//
// `chassis.test.ts` reads the file and asserts its shape. That catches the
// configuration we already know is wrong. This catches the one we don't: it
// starts a container from the chassis and asks the two questions a rendered
// service has to answer before it reaches anybody.
//
//   1. DOES ITS INIT SURVIVE? `cap_drop: ALL` crash-looped the whole fleet and
//      shipped, because no test in this repository had ever started a
//      container. Compose accepting an `up` means nothing — the failure was
//      s6's init hooks exiting 111 sixty seconds later, and the container
//      dying with it.
//
//   2. DOES IT STOP CLEANLY? This is the half a health check cannot see. Drop
//      CAP_KILL and the agent still boots, still reports healthy, still serves
//      /health — and `docker stop` falls through to SIGKILL, so every
//      fleetStop, every restart and every slot roll hard-kills the agent at the
//      timeout instead of shutting it down. Exit 137 is the tell, and only
//      stopping it on purpose reveals it.
//
// WHAT THIS DELIBERATELY DOES NOT ASSERT: that the agent is HEALTHY. Hermes
// only starts its API server — the thing behind `/health` — once it has a
// reachable model, so a health assertion would make this test require a live
// LLM, an API key and network egress. That is a functional test of an agent,
// not a test of the chassis, and CI cannot hold those.
//
// The two things it does assert need none of that, and they are exactly the two
// the incident produced: init hooks that exit 111, and a stop that falls
// through to SIGKILL. Both are capability failures, both are invisible to a
// health check, and one of them (the SIGKILL) happens on a container that
// reports healthy.
//
// Deliberately NOT part of `npm test`: it needs Docker and pulls a large image,
// so it is a separate CI job gated on the paths that can break it. Run it
// locally the same way CI does:
//
//     node scripts/chassis-boot-smoke.mjs
//
// Exit 0 = the chassis boots and stops cleanly. Any other exit prints what
// failed and why it matters.
import { execFile } from 'node:child_process'
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { createRequire } from 'node:module'

const exec = promisify(execFile)
const HERE = dirname(fileURLToPath(import.meta.url))
const REPO = join(HERE, '..')
const TEMPLATE = join(REPO, 'scripts', 'chassis.template.yml')

// The one dependency, and it is already installed for the app. Parsing YAML by
// hand to keep this dependency-free would be its own source of bugs.
const require = createRequire(join(REPO, 'ui', 'package.json'))
const { parse: parseYaml, stringify: stringifyYaml } = require('yaml')

const PROJECT = 'talaria-chassis-smoke'
const SERVICE = 'smoke'
const CONTAINER = `${PROJECT}-${SERVICE}-1`
/** The chassis gives itself 60s before the healthcheck counts, and the image is
 *  an s6 supervision tree with several init hooks. 180s is generous enough that
 *  a failure here is a real failure rather than a slow CI runner. */
const BOOT_SETTLE_MS = 180_000
/** The line stage2 prints once every init hook has run. Waiting for THIS rather
 *  than for a fixed sleep is what makes the test honest on a slow runner. */
const INIT_DONE = /\[stage2\] Setup complete/
/** The failure this whole file exists for, in every spelling the image has been
 *  observed to produce. All three mean the same thing — s6 could not drop
 *  privileges, so the /opt/data bootstrap never ran.
 *
 *  `could not chown` is the one that actually fires on the current image;
 *  the `exited 111` and `s6-applyuidgid` forms are kept because they are what
 *  older tags print and what the incident was originally diagnosed from. Match
 *  all of them rather than betting on one: a signature that stops matching
 *  downgrades this test to the generic "did not stay up", which is still a
 *  failure but no longer a diagnosis. */
const INIT_FAILED = /cont-init:.*exited 111|s6-applyuidgid: fatal|could not chown/
/** Compose's default stop timeout is 10s; the chassis stops in ~4s when it can
 *  signal its children and hits the timeout when it cannot. 20s separates those
 *  two cases without waiting on a hang. */
const STOP_TIMEOUT_S = 20

const log = (m) => console.log(`[chassis-smoke] ${m}`)
const fail = (m) => {
  console.error(`\n[chassis-smoke] FAILED: ${m}\n`)
  process.exitCode = 1
}

async function docker(args, opts = {}) {
  return exec('docker', args, { timeout: 300_000, ...opts })
}

/** A SYNTHETIC AGENT, minimal but shaped like a rendered one — a rendered agent
 *  gets a config.yaml and a SOUL.md mounted, so a chassis booted without them is
 *  a configuration Talaria never actually produces.
 *
 *  No LLM is reachable from here and none is needed. The model block points at a
 *  base_url that will never answer, which is why the gateway never starts its API
 *  server and the container never reports healthy — see the header. Init and
 *  shutdown, the two things under test, do not depend on it. */
function writeSyntheticAgent(dir) {
  const config = {
    model: { model: 'smoke-test-model', api_key: 'unused', provider: 'custom', base_url: 'http://127.0.0.1:1/v1' },
    model_aliases: {},
    fallback_providers: [],
    skills: { external_dirs: [] },
  }
  writeFileSync(join(dir, 'config.yaml'), stringifyYaml(config))
  writeFileSync(join(dir, 'SOUL.md'), '# Smoke test agent\n\nExists to prove the container boots and stops.\n')
}

/** The chassis service block, as compose, with the smoke test's own changes:
 *  a throwaway volume and NO restart policy. `restart: unless-stopped` would
 *  turn a crash-loop into something that merely looks slow to start, which is
 *  exactly how the original bug hid. */
function composeFile(dir) {
  const chassis = parseYaml(readFileSync(TEMPLATE, 'utf8'), { merge: true, version: '1.1' })
  const service = chassis?.service
  if (!service) throw new Error(`no "service:" block in ${TEMPLATE}`)

  service.restart = 'no'
  // Mounted read-only exactly as fleet-render.ts mounts them, so the smoke test
  // exercises the same shape a real agent gets.
  service.volumes = [
    `${PROJECT}-state:/opt/data`,
    `${join(dir, 'config.yaml')}:/opt/data/config.yaml:ro`,
    `${join(dir, 'SOUL.md')}:/opt/data/SOUL.md:ro`,
  ]
  // The two the renderer sets per agent; without them the gateway has no
  // identity to serve its API under.
  service.environment = {
    ...(service.environment ?? {}),
    API_SERVER_KEY: 'smoke-test-key',
    API_SERVER_MODEL_NAME: 'smoke-test-agent',
  }
  // The chassis joins the fleet's external network; the smoke test owns its own
  // so it never depends on (or disturbs) a running fleet.
  delete service.networks

  return stringifyYaml({
    services: { [SERVICE]: service },
    volumes: { [`${PROJECT}-state`]: {} },
  })
}

/** Tear down anything a previous run left behind. Split from removing the temp
 *  directory because the pre-run sweep has to leave the directory in place. */
async function composeDown(file) {
  await docker(['compose', '-p', PROJECT, '-f', file, 'down', '-v', '--remove-orphans']).catch(() => {})
}

async function main() {
  await docker(['version']).catch(() => {
    console.error('[chassis-smoke] docker is not available — this job needs it')
    process.exit(1)
  })

  const dir = mkdtempSync(join(tmpdir(), 'chassis-smoke-'))
  const file = join(dir, 'compose.yml')
  writeSyntheticAgent(dir)
  writeFileSync(file, composeFile(dir))
  const compose = (...args) => docker(['compose', '-p', PROJECT, '-f', file, ...args])

  try {
    // A previous run that died mid-test leaves a container and a volume; the
    // smoke test must start from nothing or it is testing the wrong state.
    await composeDown(file)

    log('starting one agent from the chassis…')
    await compose('up', '-d')

    // ── 1. does its init survive, and does it stay up ─────────────────────
    // "Started" proves nothing: the capability failure killed the container
    // roughly a minute in, after compose had already returned success. So wait
    // for the init sequence to finish, then check it is still running.
    const deadline = Date.now() + BOOT_SETTLE_MS
    let state = 'unknown'
    while (Date.now() < deadline) {
      const { stdout } = await docker(['inspect', '-f', '{{.State.Status}}', CONTAINER]).catch(() => ({ stdout: 'gone' }))
      state = stdout.trim()
      if (state === 'exited' || state === 'gone' || state === 'restarting') break
      const { stdout: sofar } = await docker(['logs', CONTAINER]).catch(() => ({ stdout: '' }))
      if (INIT_DONE.test(sofar)) break
      await new Promise((r) => setTimeout(r, 3000))
    }

    const { stdout: logs } = await docker(['logs', CONTAINER]).catch(() => ({ stdout: '' }))
    const initFailed = INIT_FAILED.test(logs)

    if (initFailed) {
      // Name the signature, not just the state — that turns a red CI job into a
      // diagnosis rather than a starting point.
      fail(
        `the container's INIT HOOKS FAILED — it could not chown its state tree or drop privileges. That is the ` +
        `CAPABILITY signature: s6-overlay starts as root and drops to the hermes uid itself, so it needs CHOWN, ` +
        `DAC_OVERRIDE, SETUID and SETGID to boot (and KILL to stop cleanly). Check cap_add in ` +
        `scripts/chassis.template.yml — dropping ALL without adding these back crash-loops every agent.`,
      )
      console.error(logs.split('\n').slice(-40).join('\n'))
      return
    }
    if (state !== 'running') {
      fail(`the container did not stay up (state: ${state}) — it started and then died`)
      console.error(logs.split('\n').slice(-40).join('\n'))
      return
    }
    log('init clean, still running ✓')

    // ── 2. does it stop cleanly ───────────────────────────────────────────
    log(`stopping (timeout ${STOP_TIMEOUT_S}s)…`)
    const started = Date.now()
    await docker(['stop', '-t', String(STOP_TIMEOUT_S), CONTAINER])
    const took = Math.round((Date.now() - started) / 1000)

    const { stdout: codeOut } = await docker(['inspect', '-f', '{{.State.ExitCode}}', CONTAINER])
    const code = Number(codeOut.trim())

    if (code === 137) {
      fail(
        `the container was SIGKILLed on stop (exit 137 after ${took}s). It booted cleanly and would report ` +
        `healthy, so NOTHING observed at startup can see this — but every fleetStop, fleetRestart and slot ` +
        `roll now hard-kills the agent at the timeout instead of shutting it down, losing whatever it had ` +
        `in flight. The cause is a missing CAP_KILL: s6 cannot signal its own hermes-uid children.`,
      )
      return
    }
    if (code !== 0) {
      fail(`the container exited ${code} on stop after ${took}s (expected 0)`)
      return
    }
    log(`stopped cleanly in ${took}s, exit 0 ✓`)
    log('PASS — the chassis boots and stops cleanly')
  } catch (e) {
    fail(e?.message ?? String(e))
  } finally {
    await composeDown(file)
    rmSync(dir, { recursive: true, force: true })
  }
}

await main()
