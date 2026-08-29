// Secrets must never ride into git through any env file a dev workflow
// writes. The ignore list in .gitignore is policy; this pins it — the
// canonical secret-carrying paths resolve as ignored (bare patterns, so a
// devbox tree relocated INTO a checkout via TALARIA_DEVBOX_HOME is covered),
// and the one example env file stays trackable.

import { describe, expect, test } from 'bun:test'
import { execFileSync } from 'node:child_process'
import { repoRoot } from '../../paths'

describe('secrets stay out of git', () => {
  test('every secret-carrying env path is git-ignored (any depth)', () => {
    // The canonical carriers: primary env, fleet env, the deploy compose
    // channel (docker/.env — `deploy up` writes the generated shared secrets
    // there), the devbox compose channels (the override carries provider
    // tokens/--env values, compose.env carries S3 creds), and the rendered
    // searxng secret.
    const paths = [
      'ui/.env',
      'mcp/.env',
      '.env.local',
      'fleet/.env',
      'docker/.env',
      'compose.env',
      'compose.override.yml',
      'docker-compose.override.yml',
      'devboxes/demo/compose.env', // TALARIA_DEVBOX_HOME inside the repo
      'devboxes/demo/compose.override.yml',
      'docker/searxng/settings.local.yml',
    ]
    const cwd = repoRoot(import.meta.dir)
    const out = execFileSync('git', ['check-ignore', '--stdin'], {
      cwd,
      input: `${paths.join('\n')}\n`,
    }).toString()
    const ignored = new Set(out.split('\n').filter(Boolean))
    expect(paths.filter((p) => !ignored.has(p))).toEqual([])
    // the one env file that MUST stay trackable — check-ignore lists nothing
    // for it (and exits 1)
    let listed = ''
    try {
      listed = execFileSync('git', ['check-ignore', '--stdin'], { cwd, input: 'ui/.env.example\n' }).toString()
    } catch {
      /* exit 1 = nothing ignored, exactly right */
    }
    expect(listed.trim()).toBe('')
  })
})
