// Boot-time environment validation. `server-entry.js` calls validateEnv() once
// before listen(), so a misconfigured deploy fails at start with EVERY problem
// listed at once — instead of at whichever request first happens to touch the
// missing value (getSql(), getRedis() and kekMaterial() each throw lazily, which
// surfaces as a random 500 hours later).
//
// Deliberately dependency-light: zod and node:fs only, no `@/` aliases and no
// app imports. The production entry is plain JS and imports this module
// directly, so anything pulled in here has to resolve without the Vite graph.

import { readFileSync } from 'node:fs'
import { z } from 'zod'

/** The bundled MinIO password published in docker/dev-compose.yml and
 *  ui/.env.example. Anyone who has read this repo knows it, so it is a local-dev
 *  convenience only — never a production credential. `storage.ts` refuses it at
 *  use-time and validateEnv() refuses it at boot. */
export const DEV_S3_SECRET = 'talaria-dev-secret'

// Empty-string env vars are "unset" as far as every consumer is concerned
// (`process.env.X || fallback` throughout the codebase), so normalise them
// before the schema sees them — otherwise `FOO=` sneaks past as a present value.
const blankToUndefined = (v: unknown) => (typeof v === 'string' && v.trim() === '' ? undefined : v)

// Every message below is written to read as a continuation of "KEY: ", which is
// how formatIssue() renders it.
const opt = z.preprocess(blankToUndefined, z.string().optional())

const optPort = z.preprocess(
  blankToUndefined,
  z
    .string()
    // One combined check rather than regex-then-range: two chained refinements
    // both fire on garbage like "abc" and report the same key twice.
    .refine((v) => /^\d+$/.test(v) && Number(v) > 0 && Number(v) < 65536, 'must be a port number between 1 and 65535')
    .optional(),
)

const optUrl = z.preprocess(
  blankToUndefined,
  z
    .string()
    .refine((v) => {
      try {
        new URL(v)
        return true
      } catch {
        return false
      }
    }, 'must be an absolute URL')
    .optional(),
)

export const envSchema = z
  .object({
    // ── Required: the three things nothing works without ──────────────────────
    DATABASE_URL: z.preprocess(
      blankToUndefined,
      z
        .string({ error: 'is required' })
        .refine((v) => /^postgres(ql)?:\/\//.test(v), 'must be a postgres:// connection string'),
    ),
    REDIS_URL: z.preprocess(
      blankToUndefined,
      z.string({ error: 'is required' }).refine((v) => /^rediss?:\/\//.test(v), 'must be a redis:// or rediss:// URL'),
    ),

    // ── The encryption root (one of the three; checked as a group below) ──────
    TALARIA_SECRET_KEY: opt,
    TALARIA_SECRET_KEY_FILE: opt,
    AUTH_SECRET: opt,

    // ── Runtime ───────────────────────────────────────────────────────────────
    // Unset means "development" to every consumer (session.ts only special-cases
    // the literal 'production'), so it stays optional — but a typo like
    // NODE_ENV=prod silently drops the Secure flag from session cookies, which
    // is exactly the kind of thing worth failing a boot over.
    NODE_ENV: z.preprocess(blankToUndefined, z.enum(['development', 'test', 'production']).optional()),
    PORT: optPort,

    // ── Auth ──────────────────────────────────────────────────────────────────
    // Optional in the schema, but an empty value yields an instance with zero
    // admins and no other signal — collectWarnings() shouts about that case.
    AUTH_ADMIN_EMAILS: opt,
    // Raw AUTH_USERS pairs — parsed by auth/config.ts; here only so
    // collectWarnings() can nag about plaintext entries (#244).
    AUTH_USERS: opt,

    // ── Built-in object storage (Admin → Storage, mode 'internal') ────────────
    TALARIA_S3_URL: optUrl,
    TALARIA_S3_BUCKET: opt,
    TALARIA_S3_ACCESS_KEY: opt,
    TALARIA_S3_SECRET_KEY: opt,
    TALARIA_MINIO_PORT: optPort,

    // ── Outbound-fetch policy (server/safe-fetch.ts) ──────────────────────────
    // Every config-derived URL — MCP servers, OAuth discovery, app catalogs,
    // favicons — is resolved and refused if it lands on a private/loopback/
    // link-local address. These two are the deliberate escape hatches; both are
    // comma-separated and both WIDEN what the server may reach, so they belong
    // in review.
    TALARIA_FETCH_ALLOW_HOSTS: opt, // hosts, *.suffix, bare IPs, or CIDRs
    TALARIA_MCP_OAUTH_TRUSTED_DOMAINS: opt, // extra cross-domain OAuth pairs
  })
  .superRefine((env, ctx) => {
    // The KEK root. secretbox.ts tries these three in order and throws if all
    // are empty; without one, every stored provider key and OAuth refresh token
    // is unreadable — so this is boot-blocking, not a warning.
    let fileMaterial = ''
    const keyFile = env.TALARIA_SECRET_KEY_FILE
    if (keyFile) {
      try {
        fileMaterial = readFileSync(keyFile, 'utf8').trim()
        if (!fileMaterial) {
          ctx.addIssue({ code: 'custom', path: ['TALARIA_SECRET_KEY_FILE'], message: `points at an empty file (${keyFile})` })
        }
      } catch (err) {
        ctx.addIssue({
          code: 'custom',
          path: ['TALARIA_SECRET_KEY_FILE'],
          message: `cannot read ${keyFile} — ${err instanceof Error ? err.message : String(err)}`,
        })
      }
    }
    if (!env.TALARIA_SECRET_KEY && !fileMaterial && !env.AUTH_SECRET) {
      ctx.addIssue({
        code: 'custom',
        path: ['TALARIA_SECRET_KEY'],
        message:
          'the encryption root is unset — set one of TALARIA_SECRET_KEY, TALARIA_SECRET_KEY_FILE, or AUTH_SECRET ' +
          '(generate with: openssl rand -base64 48). Whichever you pick must NEVER change afterwards, or every stored ' +
          'secret becomes permanently unrecoverable.',
      })
    }

    // The published dev password must never reach production. storage.ts carries
    // the same guard at use-time; this one refuses the boot so it cannot linger.
    if (env.NODE_ENV === 'production' && env.TALARIA_S3_SECRET_KEY === DEV_S3_SECRET) {
      ctx.addIssue({
        code: 'custom',
        path: ['TALARIA_S3_SECRET_KEY'],
        message:
          `is still the published dev default "${DEV_S3_SECRET}" — it is committed to this repo, so it is public. ` +
          'Set a real secret (openssl rand -hex 24) and update the minio container to match.',
      })
    }
  })

export type Env = z.infer<typeof envSchema>

/** A `scrypt$…` AUTH_USERS entry (see auth/password.ts's hashPassword — the
 *  format node and bun BOTH verify dependency-free). Lives HERE, not in
 *  auth/password.ts, because this module is the app-import-free leaf the
 *  plain-JS entry resolves — the boot warning and the verify path share one
 *  definition by password.ts importing it from here. */
export function isHashedCredential(password: string): boolean {
  return password.startsWith('scrypt$')
}

// What each key is for, appended to its failure so the message stands alone: an
// operator reading a crash log does not have .env.example in front of them.
const HINTS: Record<string, string> = {
  DATABASE_URL: 'durable state (users, conversations, tasks). Example: postgres://talaria:talaria@127.0.0.1:5544/talaria',
  REDIS_URL: 'sessions + realtime fan-out. Example: redis://127.0.0.1:6399',
  TALARIA_SECRET_KEY: 'root of the envelope encryption described in docs/ENCRYPTION.md',
  TALARIA_SECRET_KEY_FILE: 'alternative to TALARIA_SECRET_KEY — a file whose contents are the root secret',
  NODE_ENV: 'one of development | test | production; anything else is treated as non-production (no Secure cookies)',
  PORT: 'HTTP port for the server (default 3000)',
  TALARIA_S3_SECRET_KEY: 'password for the bundled MinIO container; must match docker/dev-compose.yml',
}

function formatIssue(issue: { path: ReadonlyArray<PropertyKey>; message: string }): string {
  const key = issue.path.map(String).join('.')
  const head = key ? `${key}: ${issue.message}` : issue.message
  const hint = key ? HINTS[key] : undefined
  return hint ? `${head}\n      ↳ ${hint}` : head
}

/** Non-fatal misconfigurations: an instance that boots fine but that nobody can
 *  administer, or whose secrets sit on a footing the operator doesn't know about. */
function collectWarnings(env: Env): string[] {
  const warnings: string[] = []

  if (!env.AUTH_ADMIN_EMAILS) {
    warnings.push(
      'AUTH_ADMIN_EMAILS is unset — this instance has ZERO admins. Everyone who signs in becomes a member, and ' +
        'nothing in the UI can grant the first admin role. Set it to a comma-separated list of emails and restart.',
    )
  }

  if (!env.TALARIA_SECRET_KEY && !env.TALARIA_SECRET_KEY_FILE && env.AUTH_SECRET) {
    warnings.push(
      'no TALARIA_SECRET_KEY / TALARIA_SECRET_KEY_FILE — AUTH_SECRET is doing double duty as the encryption root. ' +
        'That works, but rotating AUTH_SECRET (a routine thing to do to a cookie-signing key) permanently destroys ' +
        'access to every stored secret. Set TALARIA_SECRET_KEY explicitly.',
    )
  }

  if (env.NODE_ENV === 'production' && !env.TALARIA_S3_SECRET_KEY) {
    warnings.push(
      'TALARIA_S3_SECRET_KEY is unset in production — the built-in ("internal") storage mode will refuse to run ' +
        'rather than fall back to the published dev password. Harmless if you use local disk or an external bucket.',
    )
  }

  // Plaintext AUTH_USERS entries keep verifying (#244 transition), but anything
  // that can read this process's env — a crash dump, a dotfile backup, a
  // docker inspect — sees a working password. talaria setup has written the
  // scrypt hash since the transition; only hand-rolled or upgraded envs hit this.
  const plaintext = (env.AUTH_USERS ?? '')
    .split(',')
    .map((pair) => pair.trim())
    .filter(Boolean)
    .map((pair) => pair.slice(pair.indexOf(':') + 1))
    .filter((pass) => pass && !isHashedCredential(pass))
  if (plaintext.length > 0) {
    warnings.push(
      `AUTH_USERS carries ${plaintext.length} plaintext password${plaintext.length === 1 ? '' : 's'} — readable by anything that can see this process's env. ` +
        `Hash them (from the repo root: bun -e "console.log(await (await import('./ui/src/server/auth/password')).hashPassword('the-password'))") ` +
        `and swap the entries in ui/.env; hashed entries verify unchanged.`,
    )
  }

  return warnings
}

/**
 * Validate `process.env` and return the parsed values.
 *
 * Throws a single aggregated Error listing every problem — one round trip for
 * the operator instead of fix / restart / fix / restart. Warnings go to stderr
 * rather than throwing: they describe an instance that runs, but is broken in a
 * way nothing else would ever report.
 */
export function validateEnv(env: NodeJS.ProcessEnv = process.env): Env {
  const result = envSchema.safeParse(env)

  if (!result.success) {
    const { issues } = result.error
    throw new Error(
      `Environment validation failed (${issues.length} problem${issues.length === 1 ? '' : 's'}):\n\n` +
        `${issues.map((i) => `  • ${formatIssue(i)}`).join('\n')}\n\n` +
        'See ui/.env.example for the full list, and docs/ENCRYPTION.md for the secret-key rules.',
    )
  }

  for (const w of collectWarnings(result.data)) console.warn(`[talaria-ui] WARNING: ${w}`)

  return result.data
}
