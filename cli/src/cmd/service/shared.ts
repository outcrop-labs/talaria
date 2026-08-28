// Shared plumbing for the service commands — where the unit lives, how a
// privileged command is spelled, and the probes install/status lean on.
//
// The stack this group supervises is the SAME docker compose project `talaria
// deploy` drives (docker/compose.yml); the unit is only its boot/stop handle
// (see unit.ts). Everything here is injectable — HostPaths and euid so tests
// point at a tmp tree instead of the real /etc and /run, env so PATH walks
// find planted stubs.

import { accessSync, constants } from 'node:fs'
import { delimiter, join } from 'node:path'
import type { Ctx } from '../../ctx'

export const UNIT_NAME = 'talaria.service'
export const COMPOSE_FILE = 'docker/compose.yml'

/** The host paths the guards and privileged steps touch. */
export type HostPaths = { systemdDir: string; runSystemd: string }

export const HOST: HostPaths = { systemdDir: '/etc/systemd/system', runSystemd: '/run/systemd/system' }

export const unitPath = (h: HostPaths): string => join(h.systemdDir, UNIT_NAME)

/** One privileged command, spelled for who is asking: bare when already
 *  root, sudo-wrapped otherwise (run through ctx.run so sudo can prompt on
 *  the inherited tty). */
export function privileged(euid: number, cmd: string, args: string[]): [string, string[]] {
  return euid === 0 ? [cmd, args] : ['sudo', [cmd, ...args]]
}

/** `command -v` without a shell: the first executable `bin` along PATH, or
 *  null. A unit's ExecStart needs an absolute program, and systemd's PATH is
 *  not the shell's (snap and desktop installs live outside it). */
export function resolveBin(bin: string, env: Record<string, string | undefined>): string | null {
  for (const dir of (env.PATH ?? '').split(delimiter)) {
    if (!dir) continue
    const p = join(dir, bin)
    try {
      accessSync(p, constants.X_OK)
      return p
    } catch {
      // not there / not executable — keep walking
    }
  }
  return null
}

/** Parse `systemctl show` output: one Key=Value per line. */
export function parseShow(text: string): Record<string, string> {
  const out: Record<string, string> = {}
  for (const line of text.split('\n')) {
    const at = line.indexOf('=')
    if (at > 0) out[line.slice(0, at)] = line.slice(at + 1)
  }
  return out
}

/** One unprivileged probe for everything systemd. `systemctl show` exits 0
 *  in every state — unlike is-enabled/is-active, which exit 1 for `disabled`
 *  AND `masked` (indistinguishable through exec, which rejects non-zero). */
export async function unitState(ctx: Ctx, unit: string, props: string[]): Promise<Record<string, string>> {
  const { stdout } = await ctx.exec('systemctl', ['show', unit, ...props.flatMap((p) => ['-p', p])])
  return parseShow(stdout)
}

/** The first semver in `docker compose version --short` output, or null —
 *  spellings vary (`5.4.0`, `v2.29.7-desktop.2`). */
export function parseComposeVersion(out: string): string | null {
  return out.match(/v?(\d+\.\d+\.\d+)/)?.[1] ?? null
}

/** The unit's up argv. `--wait` gates the unit's start on the stack's
 *  healthchecks — but builds older than 2.24 hang on it with a one-shot
 *  service in the project (searxng-config), and an unparseable version gets
 *  the same conservative treatment: a hung unit is worse than an ungated
 *  one, the containers' own restart policies carry the steady state. */
export function upArgsFor(version: string | null): string[] {
  if (version === null) return ['up', '-d']
  const [maj, min] = version.split('.').map(Number)
  const wait = Number.isFinite(maj) && Number.isFinite(min) && (maj > 2 || (maj === 2 && min >= 24))
  return wait ? ['up', '-d', '--wait'] : ['up', '-d']
}
