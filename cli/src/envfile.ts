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
//
// readEnvFile() (below) carries those differences as OPTIONS — quotes,
// envWins — so there is one reader per shape instead of one per caller.

import { chmodSync, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { isAbsolute, join } from 'node:path'
import type { Ctx } from './ctx'

export type EnvVars = Record<string, string>

/** The two ways the readers below differ — each one a real difference between
 *  the hand-rolled reads this replaces, so each is an option rather than a
 *  convention. */
export type ReadEnvFileOpts = {
  /** Strip ONE layer of matching quotes from every value. Off by default:
   *  the files this CLI writes are unquoted, and a base64 secret would lose
   *  real characters. On for files OTHER tools wrote by hand (ui/.env as the
   *  app's own loader would read it). */
  quotes?: boolean
  /** Let the process environment win over the file — the runtime view, with
   *  an empty-string export still winning (see envWins). Off by default: a
   *  caller that wants the file's own values gets them untouched. */
  envWins?: boolean
}

/** An env file's TEXT: `rel` under ctx.root, or `rel` itself when it is
 *  absolute (the devboxes live outside the checkout). '' when the file is not
 *  there — "missing" and "empty" mean the same thing to every reader here.
 *  Modules that need the lines themselves (a strip-list, a block appended to
 *  the existing text) take the text and parse it with parseEnv. */
export function envFileText(ctx: Ctx, rel: string): string {
  const path = isAbsolute(rel) ? rel : join(ctx.root, rel)
  return existsSync(path) ? readFileSync(path, 'utf8') : ''
}

/** The parsed view of an env file: FIRST matching line per key, value
 *  everything after the first `=`, raw unless `quotes`, and — with `envWins`
 *  — the environment shadowing the file exactly as it does at runtime. */
export function readEnvFile(ctx: Ctx, rel: string, opts: ReadEnvFileOpts = {}): EnvVars {
  const vars = parseEnv(envFileText(ctx, rel))
  if (opts.quotes) {
    for (const key of Object.keys(vars)) vars[key] = stripQuotes(vars[key]!)
  }
  return opts.envWins ? envWins(vars, ctx.env) : vars
}

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
