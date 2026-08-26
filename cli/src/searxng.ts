// Render docker/searxng/settings.local.yml from the template with the
// per-install secret. Absorbs scripts/render-searxng.sh (used by dev and by
// box bring-up; docker/entrypoint.sh performs the same substitution for the
// production stack). SearXNG reads its secret from settings.yml, not the
// environment, and the image's own substitution would need the container to
// rewrite a mounted file — so the render happens host-side, into the
// gitignored file compose mounts. Skipped when the render is already current
// (template older than the local file).

import { existsSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { randomBytes } from 'node:crypto'
import { join } from 'node:path'
import type { Ctx } from './ctx'
import { envValue } from './envfile'

export function renderSearxng(ctx: Ctx): void {
  const uiEnv = (() => {
    try {
      return readFileSync(join(ctx.root, 'ui/.env'), 'utf8')
    } catch {
      return ''
    }
  })()
  let secret = envValue(uiEnv, 'SEARXNG_SECRET')
  if (!secret) secret = `talaria-${randomBytes(24).toString('hex')}`

  const tpl = join(ctx.root, 'docker/searxng/settings.template.yml')
  const out = join(ctx.root, 'docker/searxng/settings.local.yml')
  const stale = () => {
    if (!existsSync(out)) return true
    return statSync(tpl).mtimeMs > statSync(out).mtimeMs
  }
  if (stale()) {
    writeFileSync(out, readFileSync(tpl, 'utf8').split('__SEARXNG_SECRET__').join(secret))
    ctx.log.say('rendered docker/searxng/settings.local.yml')
  }
}
