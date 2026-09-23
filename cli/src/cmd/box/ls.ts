// `talaria box ls` — the registry: every box with box.env, its branch, its
// host port, and whether the container is up. Port of scripts/devbox `ls`.

import { existsSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { containerState } from '../../containers'
import { readEnvFile } from '../../envfile'
import { devboxes } from './shared'

export async function runLs(ctx: Ctx): Promise<number> {
  const home = devboxes(ctx)
  const rows: string[][] = []
  if (existsSync(home)) {
    for (const e of readdirSync(home, { withFileTypes: true })) {
      if (!e.isDirectory()) continue
      const envPath = join(home, e.name, 'box.env')
      if (!existsSync(envPath)) continue
      const env = readEnvFile(ctx, envPath)
      const name = env.BOX_NAME ?? e.name
      const port = env.APP_PORT ?? '?'
      let branch = '?'
      try {
        branch = (await ctx.exec('git', ['-C', join(home, e.name, 'talaria'), 'branch', '--show-current'])).stdout.trim() || '?'
      } catch {
        /* detached or gone — '?' */
      }
      // not created / daemon off → null → 'gone'
      const status = (await containerState(ctx, `devbox-${name}`)) ?? 'gone'
      rows.push([name, branch, port, status])
    }
  }
  if (rows.length === 0) {
    ctx.log.raw('  (no devboxes yet — bun talaria box new <name>)')
    return 0
  }
  const line = (r: string[]): string => `  ${r[0]!.padEnd(18)} ${r[1]!.padEnd(28)} ${r[2]!.padEnd(7)} ${r[3]}`
  const table = [line(['BOX', 'BRANCH', 'APP', 'STATUS']), ...rows.map(line)].join('\n')
  ctx.log.raw(`${table}\n`)
  return 0
}

export const lsCommand: Leaf = {
  kind: 'leaf',
  name: 'ls',
  summary: 'list devboxes (branch, port, container status)',
  usage: 'talaria box ls',
  run: runLs,
}
