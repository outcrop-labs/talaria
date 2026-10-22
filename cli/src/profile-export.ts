// Where an operator's COMPOSE_FILE export lives, and the one edit the CLI
// makes to it. compose.ts repairs a fragment-less COMPOSE_FILE for the run
// in hand (composeFileEnv), but the export it came from stays wrong, so the
// repair — and its warning — came back on every single update (a customer
// VM, 2026-10-22, after a month of them). The durable repair is the line
// itself: an exact-value rewrite of the export the operator already has,
// nothing else in the file touched, announced once.

import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

/** The files a login or interactive shell reads an export from — the user's
 *  own first, then the system's. Empty without HOME: the CLI only ever edits
 *  where a real shell would look, and a test context (no HOME) never reaches
 *  a real home directory. */
export function profileFiles(env: Record<string, string | undefined>): string[] {
  const home = env.HOME
  if (!home) return []
  const own = [
    '.profile',
    '.bash_profile',
    '.bash_login',
    '.bashrc',
    '.zshenv',
    '.zprofile',
    '.zshrc',
    '.config/fish/config.fish',
  ].map((f) => join(home, f))
  let profileD: string[] = []
  try {
    profileD = readdirSync('/etc/profile.d')
      .filter((f) => f.endsWith('.sh'))
      .sort()
      .map((f) => join('/etc/profile.d', f))
  } catch {
    // no profile.d — nothing to scan there
  }
  return [...own, '/etc/environment', '/etc/profile', '/etc/profile.local', '/etc/bash.bashrc', '/etc/zshenv', ...profileD]
}

/** One COMPOSE_FILE assignment line, in the shapes a profile spells it:
 *  `export COMPOSE_FILE=…`, a bare `COMPOSE_FILE=…` (/etc/environment), and
 *  fish's `set -gx COMPOSE_FILE …` — quoted or not, with a trailing comment
 *  or not. A value built from other variables never matches the exact-value
 *  test below, so it is never rewritten. */
const ASSIGN =
  /^(?<lead>\s*(?:export\s+)?COMPOSE_FILE=|\s*set\s+-[a-zA-Z]+\s+COMPOSE_FILE\s+)(?<q>["']?)(?<value>[^"'\s#]+)\k<q>(?<tail>\s*(?:#.*)?)$/

export type ExportRepair = {
  file: string
  /** fixed: the line now carries the repaired list. already: the file was
   *  fixed before and this shell still holds the old value. readonly: the
   *  export is there but this user cannot write it. */
  status: 'fixed' | 'already' | 'readonly'
}

/** Rewrite every export of exactly `broken` to `repaired`, across `files`.
 *  Only a line whose whole value is `broken` changes; quotes, `export`, the
 *  comment and every other line are kept byte for byte. A file whose line
 *  already reads `repaired` is reported as `already`, so the caller can say
 *  "this shell predates the fix" instead of warning again. */
export function repairComposeFileExport(files: string[], broken: string, repaired: string): ExportRepair[] {
  const out: ExportRepair[] = []
  for (const file of files) {
    let text: string
    try {
      if (!existsSync(file)) continue
      text = readFileSync(file, 'utf8')
    } catch {
      continue
    }
    if (!text.includes('COMPOSE_FILE')) continue
    const lines = text.split('\n')
    let changed = false
    let already = false
    for (let i = 0; i < lines.length; i++) {
      const m = ASSIGN.exec(lines[i]!)
      if (!m?.groups) continue
      const { lead, q, value, tail } = m.groups as { lead: string; q: string; value: string; tail: string }
      if (value === broken) {
        lines[i] = `${lead}${q}${repaired}${q}${tail}`
        changed = true
      } else if (value === repaired) {
        already = true
      }
    }
    if (changed) {
      try {
        writeFileSync(file, lines.join('\n'))
        out.push({ file, status: 'fixed' })
      } catch {
        out.push({ file, status: 'readonly' })
      }
    } else if (already) {
      out.push({ file, status: 'already' })
    }
  }
  return out
}

/** A path as the operator would type it: `~/.bashrc`, not the expanded home. */
export function tildePath(file: string, home: string | undefined): string {
  return home && file.startsWith(`${home}/`) ? `~${file.slice(home.length)}` : file
}
