// CAN AN AGENT REACH TALARIA? Asked from where the AGENT stands.
//
// THE INCIDENT THIS EXISTS FOR. Every managed agent was reported healthy — the
// container was up, `/health` answered 200, the roster was green — while not a
// single one could call a tool. Its log said so once, at startup, and nothing
// read it:
//
//     WARNING tools.mcp_tool: Failed to connect to MCP server 'talaria'
//
// The host firewall (`ufw`, default-deny INPUT) had no rule admitting the
// docker bridges to the app's port. Container→host traffic is INPUT, not
// FORWARD, so Docker's own rules said nothing about it, and a DROP (rather than
// a REJECT) turns the failure into a timeout: the agent hangs rather than
// erroring, and every tool call quietly does nothing.
//
// WHY THE EXISTING ALERT MISSED IT, which is the part worth internalising.
// `alerts.ts` probes the toolkit at `127.0.0.1:5280` and it answered perfectly,
// all day, because the APP can always reach it — the app is the thing listening.
// Reachability is not a property of a service, it is a property of a PATH, and
// the only way to test the agent's path is to stand where the agent stands.
//
// So this runs a throwaway container ON THE FLEET NETWORK and asks it. That is
// expensive — a container start, ~1s — so it is deliberately NOT on the alerts
// poll. It runs when the fleet changes (render, up) and its verdict is cached
// for the alerts panel to read.
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { getSetting, setSetting } from './audit'
import { fleetNetworkName } from './fleet-render'
import { MCP_PORT } from './mcp-service'

const exec = promisify(execFile)

const KEY = 'fleet_preflight'
/** Small, always present locally (the fleet pulls far larger images), and it has
 *  a shell with a TCP-capable builtin — no python, no curl assumed. */
const PROBE_IMAGE = 'busybox:latest'

export interface PreflightResult {
  ok: boolean
  /** The URL as an AGENT would write it, not as the app would. */
  target: string
  detail: string
  at: string
}

/** The host:port an agent is configured to reach Talaria on. `host.docker.internal`
 *  is what the chassis maps via `extra_hosts`, so the probe uses the same name a
 *  rendered agent does rather than an address that only works from here. */
const appTarget = () => `host.docker.internal:${process.env.PORT ?? 5273}`
const mcpTarget = () => `host.docker.internal:${MCP_PORT()}`

/** THE REMEDY, in the syntax of whatever this host actually runs.
 *
 *  Deliberately not hardcoded to ufw: this ships to any distro, and a Fedora
 *  operator handed a `ufw` command reasonably concludes the diagnosis is wrong.
 *  Detection is by binary presence + service state, and the fallback names the
 *  PROPERTY to satisfy rather than a command, because an operator running a
 *  hand-rolled nftables ruleset knows their own syntax better than we do.
 *
 *  The durable answer is the last sentence: an app on the fleet network is
 *  container→container traffic, which Docker DOES manage, and no host firewall
 *  is in the path at all. `TALARIA_GATEWAY_SELF_URL` exists for exactly that. */
async function firewallRemedy(): Promise<string> {
  const port = process.env.PORT ?? 5273
  const has = async (bin: string, args: string[]): Promise<boolean> => {
    try {
      await exec(bin, args, { timeout: 3000 })
      return true
    } catch {
      return false
    }
  }
  const bridges = '172.16.0.0/12'
  let cmd: string
  if (await has('ufw', ['status'])) {
    cmd = `sudo ufw allow from ${bridges} to any port ${port} proto tcp`
  } else if (await has('firewall-cmd', ['--state'])) {
    cmd = `sudo firewall-cmd --permanent --add-rich-rule='rule family=ipv4 source address=${bridges} port port=${port} protocol=tcp accept' && sudo firewall-cmd --reload`
  } else if (await has('nft', ['list', 'ruleset'])) {
    cmd = `sudo nft add rule inet filter input ip saddr ${bridges} tcp dport ${port} accept`
  } else {
    cmd = `admit ${bridges} to tcp/${port} on the INPUT chain, however this host manages it`
  }
  return (
    `Admit the docker bridges to the host: ${cmd} — and the same for the toolkit port. ` +
    `Or remove the host from the path entirely by running Talaria on the fleet network and setting ` +
    `TALARIA_GATEWAY_SELF_URL to its service DNS, which is container→container and needs no firewall rule.`
  )
}

/** One TCP connect from inside a container on the fleet network.
 *
 *  `nc -z` rather than an HTTP request on purpose: the question is whether the
 *  packets arrive at all. An HTTP 401 or 405 would prove reachability just as
 *  well as a 200, and conflating "refused" with "unauthorised" is how a firewall
 *  problem gets misread as a credentials problem. */
async function reaches(network: string, target: string, timeoutSec = 5): Promise<boolean> {
  const [host, port] = target.split(':')
  try {
    await exec(
      'docker',
      [
        'run', '--rm', '--network', network,
        '--add-host', 'host.docker.internal:host-gateway',
        PROBE_IMAGE, 'sh', '-c', `nc -z -w ${timeoutSec} ${host} ${port}`,
      ],
      { timeout: (timeoutSec + 20) * 1000 },
    )
    return true
  } catch {
    return false
  }
}

/** Ask, from the fleet network, whether Talaria is reachable. Never throws:
 *  a docker that will not run is its own alert elsewhere, and a preflight that
 *  takes the caller down with it is worse than no preflight. */
export async function runFleetPreflight(): Promise<PreflightResult> {
  const at = new Date().toISOString()
  let result: PreflightResult
  try {
    const network = await fleetNetworkName()
    const app = appTarget()
    const mcp = mcpTarget()
    const [appOk, mcpOk] = await Promise.all([reaches(network, app), reaches(network, mcp)])

    if (appOk && mcpOk) {
      result = { ok: true, target: app, detail: 'agents can reach the app and the toolkit', at }
    } else {
      const dead = [!appOk && `the app (${app})`, !mcpOk && `the toolkit (${mcp})`].filter(Boolean).join(' and ')
      const remedy = await firewallRemedy()
      result = {
        ok: false,
        target: app,
        // Named concretely, because the generic version of this sentence is what
        // cost a day: the failure is on the HOST, in a place Docker's own rules
        // do not cover, and the fix is a firewall rule rather than anything in
        // Talaria.
        detail:
          `a container on the "${network}" network cannot reach ${dead}. Agents are running but cannot call a single tool. ` +
          `Container→host traffic goes through the host's INPUT chain, which Docker does NOT manage — so a default-deny ` +
          `firewall blocks it while every Docker rule still looks correct. ${remedy}`,
        at,
      }
    }
  } catch (e) {
    result = { ok: false, target: appTarget(), detail: `preflight could not run: ${(e as Error).message}`, at }
  }
  await setSetting(KEY, result).catch(() => {})
  return result
}

/** The last verdict, for surfaces that must not start a container to render.
 *  Null when the probe has never run — which is NOT the same as a failure and
 *  must not be reported as one. */
export async function lastFleetPreflight(): Promise<PreflightResult | null> {
  return (await getSetting<PreflightResult | null>(KEY, null).catch(() => null)) ?? null
}
