// `talaria app new` — scaffold a TypeScript app into apps/<slug>.
import { existsSync, mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { APP_SLUG_RE, displayName, skeletonFiles } from './skeleton'

export function appsDir(ctx: Ctx): string {
  return ctx.env.TALARIA_APPS_DIR || join(ctx.root, 'apps')
}

export function runAppNew(
  ctx: Ctx,
  slug: string,
  opts: { name?: string; icon?: string } = {},
): number {
  if (!APP_SLUG_RE.test(slug)) {
    ctx.log.die(`"${slug}" is not a usable app slug (lowercase letters, digits, dashes; ≤64 chars)`)
  }
  const name = (opts.name ?? displayName(slug)).trim()
  if (!name) ctx.log.die('--name cannot be empty')
  const icon = (opts.icon ?? '⬡').trim() || '⬡'
  const dest = join(appsDir(ctx), slug)
  if (existsSync(dest)) ctx.log.die(`${dest} already exists`)

  mkdirSync(dest, { recursive: true })
  const files = skeletonFiles({ slug, name, icon })
  for (const [rel, body] of Object.entries(files)) {
    writeFileSync(join(dest, rel), body)
  }

  const shown = dest.startsWith(ctx.root + '/') ? dest.slice(ctx.root.length + 1) : dest
  ctx.log.say(`created ${shown}`)
  ctx.log.ok('work surface, server, MCP starter')
  ctx.log.ok('enable it in Manage → Apps')
  return 0
}

export const newCommand: Leaf = {
  kind: 'leaf',
  name: 'new',
  summary: 'scaffold a TypeScript app into apps/<slug>',
  usage: 'talaria app new <slug>',
  positionals: { name: 'slug', required: true, desc: 'lowercase kebab, same rule as install' },
  flags: [
    { name: 'name', kind: 'value', desc: 'display name (default: title-cased slug)' },
    { name: 'icon', kind: 'value', desc: 'nav icon (default: ⬡)' },
  ],
  run: (ctx, args) =>
    runAppNew(ctx, args.positionals[0]!, {
      name: typeof args.flags.name === 'string' ? args.flags.name : undefined,
      icon: typeof args.flags.icon === 'string' ? args.flags.icon : undefined,
    }),
}
