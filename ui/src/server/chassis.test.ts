// THE CHASSIS, ASSERTED — the file that took the whole managed fleet down.
//
// `cap_drop: ALL` landed with no `cap_add`. s6-overlay starts as root and drops
// to the hermes uid itself, so removing the capabilities removed its ability to
// do the dropping: `s6-setuidgid`'s `setgroups()` failed, both cont-init hooks
// exited 111, the /opt/data bootstrap never ran, and every agent crash-looped.
// Five capabilities back and it boots clean.
//
// The bug is not the interesting part. NOTHING IN THE CODEBASE READ THIS FILE.
// A one-line config change took down every agent and the repository had no
// opinion about it — no test parsed the template, let alone booted what it
// renders. This is the cheap half of closing that: the shape of the rendered
// service, checked on every commit, with no Docker required.
//
// The expensive half is a boot smoke test (`scripts/chassis-boot-smoke.sh`),
// which starts a real container and is gated on the paths that can break it.
// This file is what runs on every push; that one is what proves the container
// actually comes up.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parse as parseYaml } from 'yaml'

const REPO = join(new URL('.', import.meta.url).pathname, '..', '..', '..')
const TEMPLATE = join(REPO, 'scripts', 'chassis.template.yml')

interface Service {
  image?: string
  cap_drop?: string[]
  cap_add?: string[]
  security_opt?: string[]
  pids_limit?: unknown
  mem_limit?: unknown
  cpus?: unknown
  user?: string
  healthcheck?: { test?: unknown }
  extra_hosts?: string[]
}

const chassis = parseYaml(readFileSync(TEMPLATE, 'utf8'), { merge: true, version: '1.1' }) as {
  service?: Service
  [k: string]: unknown
}
const service = chassis.service ?? ({} as Service)

/** The five s6-overlay needs to boot and to stop cleanly. Each is here because
 *  removing it was OBSERVED to break something, not because it looked prudent:
 *  the first four for the uid drop and the /opt/data bootstrap, KILL so s6 can
 *  signal its own children (without it `docker stop` falls through to SIGKILL
 *  and every roll hard-kills the agent at the timeout). */
const REQUIRED_CAPS = ['CHOWN', 'DAC_OVERRIDE', 'SETUID', 'SETGID', 'KILL']

describe('the chassis template', () => {
  it('parses', () => {
    expect(chassis).toBeTruthy()
    expect(service, 'no `service:` block — the renderer reads exactly this key').toBeTruthy()
  })

  it('does not drop ALL capabilities without adding back the ones s6 needs', () => {
    // THE REGRESSION, stated as the shape rather than as a list: dropping
    // everything and adding nothing is the exact configuration that crash-looped
    // the fleet, and it is the one a well-meaning hardening pass writes.
    const drops = service.cap_drop ?? []
    if (!drops.map(String).map((c) => c.toUpperCase()).includes('ALL')) return
    const adds = (service.cap_add ?? []).map(String).map((c) => c.toUpperCase())
    expect(
      adds.length,
      'cap_drop: ALL with no cap_add — s6-overlay cannot drop to the hermes uid, ' +
        'cont-init exits 111 and every agent crash-loops. Add back: ' + REQUIRED_CAPS.join(', '),
    ).toBeGreaterThan(0)
    for (const cap of REQUIRED_CAPS) {
      expect(adds, `cap_add is missing ${cap} — see the incident table in the chassis comments`).toContain(cap)
    }
  })

  it('keeps no-new-privileges, which costs nothing and was never the problem', () => {
    const opts = (service.security_opt ?? []).map(String)
    expect(opts.some((o) => o.replace(/\s/g, '') === 'no-new-privileges:true')).toBe(true)
  })

  it('bounds pids, memory and cpu — one fork bomb must not take the host', () => {
    // Without pids_limit a single runaway agent takes down the database, the
    // app, and every other agent with it.
    for (const key of ['pids_limit', 'mem_limit', 'cpus'] as const) {
      expect(service[key], `${key} is unset — an agent is unbounded`).toBeDefined()
    }
  })

  it('does NOT pin a `user:`, which is the fix that looks right and is not', () => {
    // s6-overlay expects to start as root and drop privileges itself (fix-attrs
    // chowns the state volume on boot). Pinning a uid breaks init AND orphans
    // the /opt/data volume of every already-deployed agent. Rootless agents need
    // an image built for it, not a compose override — asserted so the next
    // hardening pass reads this instead of rediscovering it.
    expect(service.user).toBeUndefined()
  })

  it('has a healthcheck, and it is the cheap one', () => {
    // Deliberately shallow: this attests the gateway process answers. Whether
    // the agent can actually reach the LLM and MCP gateways is a DEEPER question
    // and belongs to the roster, not to Docker's restart policy.
    expect(service.healthcheck?.test, 'no healthcheck — a dead agent looks identical to a live one').toBeDefined()
  })
})
