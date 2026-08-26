// .env reading and secret-file writing, with the exact semantics the bash
// scripts relied on:
//
//   - value = everything after the FIRST `=` (`cut -d= -f2-`): a base64 value
//     containing `=` must survive intact
//   - FIRST matching line wins (`grep -m1` / `head -1`)
//   - raw values, no quote stripping — the files the scripts wrote were
//     unquoted; `stripQuotes` exists for reading files OTHER tools wrote
//   - env wins over file at runtime (the loadEnvFile rule; the CLI only reads
//     files the shell env does not override)

import { chmodSync, writeFileSync } from 'node:fs'

export type EnvVars = Record<string, string>

/** Parse KEY=VALUE lines. Comments and blanks are skipped; a line without an
 *  `=` after a valid key start is left alone (matches grep's selectivity). */
export function parseEnv(text: string): EnvVars {
  const out: EnvVars = {}
  for (const line of text.split('\n')) {
    const m = /^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/.exec(line)
    if (m && !(m[1] in out)) out[m[1]] = m[2]
  }
  return out
}

/** First `^KEY=` value, raw (no quote handling) — `grep -m1 … | cut -d= -f2-`. */
export function envValue(text: string, key: string): string | undefined {
  for (const line of text.split('\n')) {
    if (line.startsWith(`${key}=`)) return line.slice(key.length + 1)
  }
  return undefined
}

/** Strip ONE layer of matching quotes, if the whole value is quoted. */
export function stripQuotes(v: string): string {
  if (v.length >= 2 && ((v[0] === '"' && v.endsWith('"')) || (v[0] === "'" && v.endsWith("'")))) {
    return v.slice(1, -1)
  }
  return v
}

/** The runtime view: environment variables win over the file. Only keys the
 *  env actually SET shadow the file — an empty-string export still wins
 *  (that is the footgun the backup/restore docs warn about, so keep it). */
export function envWins(file: EnvVars, env: Record<string, string | undefined>): EnvVars {
  const out = { ...file }
  for (const [k, v] of Object.entries(env)) {
    if (v !== undefined) out[k] = v
  }
  return out
}

/** Write a file nobody else should read: compose.env, compose.override.yml,
 *  a seeded fleet/.env — the `umask 077` writes of the bash era. */
export function writeSecret(path: string, content: string): void {
  writeFileSync(path, content, { mode: 0o600 })
  // chmod again: if the file already existed, writeFileSync keeps its old mode.
  chmodSync(path, 0o600)
}
