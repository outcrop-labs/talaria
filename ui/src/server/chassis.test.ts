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
  stop_grace_period?: string
  cap_drop?: string[]
  cap_add?: string[]
  security_opt?: string[]
  pids_limit?: unknown
  mem_reservation?: unknown
  mem_limit?: unknown
  cpu_shares?: unknown
  oom_score_adj?: unknown
  user?: string
  healthcheck?: { test?: unknown }
  extra_hosts?: string[]
  dns?: string[]
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
    for (const key of ['pids_limit', 'mem_reservation', 'mem_limit', 'cpu_shares'] as const) {
      expect(service[key], `${key} is unset — an agent is unbounded`).toBeDefined()
    }
    expect(service.oom_score_adj, 'agents must die before Postgres').toBe(500)
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

  it('sets explicit external DNS — the host resolver cannot be trusted from a bridge', () => {
    // THE SILENT ONE: docker points containers at whatever the host's
    // /etc/resolv.conf named when the network was created. A loopback stub on
    // the host becomes a bridge IP where nothing listens; plenty of LAN/router
    // resolvers refuse queries from docker subnets. The fleet stays green and
    // every external name dies with EAI_AGAIN/SERVFAIL — the built-in browser
    // (which fetches its engine from npm on first use) shipped dead for exactly
    // this reason while every agent reported healthy. Explicit resolvers route
    // around the host entirely; AGENT_DNS_1/_2 override for locked-down nets.
    const dns = (service.dns ?? []).map(String).filter(Boolean)
    expect(
      dns.length,
      'no explicit dns: entries — external name resolution silently depends on ' +
        'the host resolver the day the network was created',
    ).toBeGreaterThanOrEqual(1)
  })

  it('pins the Hermes image to a tag and a digest, overridable by HERMES_IMAGE', () => {
    // A floating `:latest` meant every host ran whatever it cached first and
    // CI booted a different image than production. The default is the pin.
    const m = /^\$\{HERMES_IMAGE:-(nousresearch\/hermes-agent:[^@}]+@sha256:[0-9a-f]{64})\}$/.exec(service.image ?? '')
    expect(m, `image must be \${HERMES_IMAGE:-nousresearch/hermes-agent:<tag>@sha256:<digest>}, got ${service.image}`).toBeTruthy()
  })

  it('builds the workbench image FROM the same Hermes pin', () => {
    // A Developer Agent must run the Hermes every other agent runs — a second
    // floating base would be a second version axis.
    const pin = /^\$\{HERMES_IMAGE:-(.+)\}$/.exec(service.image ?? '')?.[1]
    expect(pin).toBeTruthy()
    const dockerfile = readFileSync(join(REPO, 'scripts', 'workbench.Dockerfile'), 'utf8')
    expect(dockerfile).toContain(`ARG HERMES_IMAGE=${pin}\n`)
    const build = readFileSync(join(REPO, 'scripts', 'build-workbench-image.sh'), 'utf8')
    expect(build).toContain(`\${HERMES_IMAGE:-${pin}}`)
  })

  it('gives a stop long enough for Hermes to drain cron and /v1 runs', () => {
    // A roll recreates the container in place; docker's default 10s grace
    // SIGKILLs a Hermes still inside its 30s cron drain (+10s cleanup).
    const grace = /^(\d+)s$/.exec(service.stop_grace_period ?? '')
    expect(grace, `stop_grace_period must be set in seconds, got ${service.stop_grace_period}`).toBeTruthy()
    expect(Number(grace?.[1])).toBeGreaterThanOrEqual(45)
  })
})
