// The omp auth bridge: Oh My Pi's own OAuth flows, driven headlessly, over a
// loopback HTTP API the Rust api calls.
//
// WHY A BRIDGE AT ALL. Talaria holds the credentials (sealed in Postgres, per
// agent) and serves them to each agent's omp over omp's auth-broker protocol
// — which makes Talaria the canonical refresher. Performing those flows means
// knowing 23 providers' client ids, PKCE quirks, device-code endpoints, token
// exchanges, userinfo enrichment and per-provider refresh hooks. omp already
// knows all of it, declaratively, and ships it on npm. So we do not
// reimplement a single provider: we run upstream's own engine and keep the
// result. Nothing here mirrors a provider parameter, which is the point —
// upstream publishes several versions a week.
//
// WHAT THIS PROCESS IS. A single-purpose child of the api (spawned like
// talaria-mcp's), bound to loopback, bearer-gated, holding login sessions in
// memory for as long as a person takes to finish one. It is deliberately
// stateless about credentials: a finished login hands the credential to the
// api and forgets it. Nothing is written to disk — no omp agent dir, no
// sqlite, no token file.
//
// THE THREE FLOW SHAPES, all of which upstream supports headlessly:
//   - authorization code + loopback callback (anthropic, openai-codex,
//     google-gemini-cli, …): we surface the authorize URL. When the person's
//     browser can reach this host's loopback — a dev stack on the same
//     machine — the callback completes it. When it cannot (every deployed
//     instance), the same providers all declare a paste-code fallback and we
//     take the pasted code or redirect URL instead. That is the provider's
//     own instruction text, not our invention.
//   - device code (xai-oauth, muse-code, kimi-code, factory-droid): a URL and
//     a user code; upstream polls. No callback, nothing to paste.
//   - custom (github-copilot, cursor, perplexity, …): prompts and/or polling,
//     driven through the same controller callbacks.

import { getProviderDefinition } from '@oh-my-pi/pi-ai/registry'
import { authPolicyFor } from '@oh-my-pi/pi-catalog/compat/auth'
import {
  getOAuthProviders,
  normalizeOAuthCredentialExpiry,
  refreshOAuthToken,
} from '@oh-my-pi/pi-ai/registry/oauth'
import type { OAuthCredentials, OAuthPrompt } from '@oh-my-pi/pi-ai/registry/oauth/types'
import { resolveCredentialIdentityKey } from '@oh-my-pi/pi-ai/auth/sqlite-credential-store'
import { getBundledModels, isGeneratedProvider } from '@oh-my-pi/pi-catalog'

// ── Credential wire shapes (the api's, matching omp's broker protocol) ──────

type OAuthCredential = {
  type: 'oauth'
  access: string
  refresh: string
  expires: number
  apiEndpoint?: string
  enterpriseUrl?: string
  projectId?: string
  email?: string
  accountId?: string
  orgId?: string
  orgName?: string
  authorizedAt?: number
}
type ApiKeyCredential = { type: 'api_key'; key: string; source?: 'login' }
type AuthCredential = OAuthCredential | ApiKeyCredential

/** Only the keys omp's broker schema accepts — it rejects unknown members. */
function oauthCredential(c: OAuthCredentials, authorizedAt: number): OAuthCredential {
  const out: OAuthCredential = {
    type: 'oauth',
    access: c.access,
    refresh: c.refresh,
    expires: c.expires,
    authorizedAt: c.authorizedAt ?? authorizedAt,
  }
  for (const k of [
    'apiEndpoint',
    'enterpriseUrl',
    'projectId',
    'email',
    'accountId',
    'orgId',
    'orgName',
  ] as const) {
    const v = c[k]
    if (typeof v === 'string' && v.length > 0) out[k] = v
  }
  return out
}

// ── The provider roster ─────────────────────────────────────────────────────

type RosterEntry = {
  id: string
  name: string
  /** Where the credential is stored when it differs (openai-codex-device ⇒ openai-codex). */
  storeAs: string
  available: boolean
  /** The flow shape, for the UI's copy: what the person is about to be asked for. */
  flow: 'oauth-code' | 'device-code' | 'custom' | 'api-key'
  /** Whether a pasted code or redirect URL can finish it (needed off-host). */
  pasteCode: boolean
}

function roster(): RosterEntry[] {
  const out: RosterEntry[] = []
  for (const info of getOAuthProviders()) {
    const id = String(info.id)
    // The flow shape comes from the COMPILED policy (rules/auth/<id>.kdl);
    // the built registry definition only carries the login function. An
    // `api-key` login is a key paste, not a sign-in — it is not an OAuth
    // method and does not belong on this roster.
    const kind = authPolicyFor(id)?.login?.kind
    if (!kind || kind === 'api-key') continue
    out.push({
      id,
      name: info.name,
      storeAs: info.storeCredentialsAs ?? id,
      available: info.available,
      flow: kind,
      pasteCode: getProviderDefinition(id)?.pasteCodeFlow === true,
    })
  }
  return out
}

// ── Login sessions ──────────────────────────────────────────────────────────

type Phase = 'starting' | 'authorize' | 'waiting' | 'input' | 'done' | 'error'

type Pending = {
  /** `code` is a pasted authorization code or redirect URL; `prompt` is the provider's own question. */
  kind: 'prompt' | 'code'
  message: string
  placeholder?: string
  secret: boolean
  allowEmpty: boolean
  resolve: (value: string) => void
  reject: (err: Error) => void
}

type Session = {
  id: string
  provider: string
  storeAs: string
  phase: Phase
  createdAt: number
  touchedAt: number
  url?: string
  launchUrl?: string
  instructions?: string
  progress?: string
  pending?: Pending
  credential?: AuthCredential
  identityKey?: string | null
  error?: string
  abort: AbortController
}

const sessions = new Map<string, Session>()
/** A person finishing a login in a browser; generous, swept on a timer. */
const SESSION_TTL_MS = 15 * 60_000

function sweep(): void {
  const now = Date.now()
  for (const [id, s] of sessions) {
    if (now - s.touchedAt < SESSION_TTL_MS) continue
    sessions.delete(id)
    s.pending?.reject(new Error('login session expired'))
    if (s.phase !== 'done' && s.phase !== 'error') s.abort.abort()
  }
}

/** The session as the api reads it — never the resolvers, never a stale secret. */
function wire(s: Session) {
  return {
    id: s.id,
    provider: s.provider,
    storeAs: s.storeAs,
    phase: s.phase,
    url: s.url ?? null,
    launchUrl: s.launchUrl ?? null,
    instructions: s.instructions ?? null,
    progress: s.progress ?? null,
    prompt: s.pending
      ? {
          kind: s.pending.kind,
          message: s.pending.message,
          placeholder: s.pending.placeholder ?? null,
          secret: s.pending.secret,
          allowEmpty: s.pending.allowEmpty,
        }
      : null,
    credential: s.credential ?? null,
    identityKey: s.identityKey ?? null,
    error: s.error ?? null,
  }
}

function ask(s: Session, kind: Pending['kind'], prompt: OAuthPrompt): Promise<string> {
  return new Promise<string>((resolve, reject) => {
    // One question at a time: a second ask while one is open is a provider
    // sequencing we do not model, and answering the wrong one silently would
    // be worse than failing.
    if (s.pending) {
      reject(new Error('a prompt is already awaiting an answer'))
      return
    }
    s.pending = {
      kind,
      message: prompt.message,
      placeholder: prompt.placeholder,
      secret: prompt.secret === true,
      allowEmpty: prompt.allowEmpty === true,
      resolve: value => {
        s.pending = undefined
        s.phase = s.url ? 'waiting' : 'starting'
        resolve(value)
      },
      reject: err => {
        s.pending = undefined
        reject(err)
      },
    }
    s.phase = 'input'
    s.touchedAt = Date.now()
  })
}

function startLogin(providerId: string): Session {
  const entry = roster().find(p => p.id === providerId)
  if (!entry) throw new Error(`unknown oauth provider: ${providerId}`)
  if (!entry.available) throw new Error(`${entry.name} is not available in this omp build`)
  const def = getProviderDefinition(providerId)
  if (!def?.login) throw new Error(`${providerId} declares no login flow`)

  const s: Session = {
    id: crypto.randomUUID(),
    provider: providerId,
    storeAs: entry.storeAs,
    phase: 'starting',
    createdAt: Date.now(),
    touchedAt: Date.now(),
    abort: new AbortController(),
  }
  sessions.set(s.id, s)

  const authorizedAt = Date.now()
  const login = def.login(
    {
      signal: s.abort.signal,
      onAuth(info) {
        s.url = info.url
        s.launchUrl = info.launchUrl
        s.instructions = info.instructions
        // `input` wins: a provider that asked something before handing over
        // the URL is still waiting on the person.
        if (s.phase !== 'input') s.phase = 'authorize'
        s.touchedAt = Date.now()
      },
      onProgress(message) {
        s.progress = message
        s.touchedAt = Date.now()
      },
      onPrompt: prompt => ask(s, 'prompt', prompt),
      // Only where the provider declares it. For a loopback-callback flow on
      // a host the person's browser CAN reach, the callback finishes first and
      // this is never asked; where it cannot, this is the only way through.
      ...(entry.pasteCode
        ? {
            onManualCodeInput: (signal?: AbortSignal) => {
              const p = ask(s, 'code', {
                message: 'Paste the authorization code, or the full redirect URL',
              })
              signal?.addEventListener('abort', () => s.pending?.reject(new Error('cancelled')), {
                once: true,
              })
              return p
            },
          }
        : undefined),
      // A cookie out of an isolated browser is not something a headless
      // bridge can produce. Fail with the reason rather than hang.
      async onBrowserSession() {
        throw new Error(
          `${entry.name} needs an interactive browser session, which this instance cannot drive`,
        )
      },
    },
    // The controller is the only argument; the cast keeps the optional-callback
    // union from widening the object literal above.
  ) as Promise<OAuthCredentials | string>

  login
    .then(result => {
      const credential: AuthCredential =
        typeof result === 'string'
          ? { type: 'api_key', key: result, source: 'login' }
          : oauthCredential(
              normalizeOAuthCredentialExpiry(entry.storeAs, result),
              authorizedAt,
            )
      s.credential = credential
      // The same identity omp's own store would key this row by, so a
      // re-login replaces the account instead of doubling it.
      s.identityKey = resolveCredentialIdentityKey(entry.storeAs, credential as never)
      s.phase = 'done'
      s.touchedAt = Date.now()
    })
    .catch((err: unknown) => {
      s.error = err instanceof Error ? err.message : String(err)
      s.phase = 'error'
      s.touchedAt = Date.now()
    })

  return s
}

// ── HTTP ────────────────────────────────────────────────────────────────────

const PORT = Number.parseInt(process.env.OMP_AUTH_BRIDGE_PORT ?? '5276', 10)
const TOKEN = process.env.OMP_AUTH_BRIDGE_TOKEN ?? ''

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}
const fail = (status: number, error: string) => json({ error }, status)

function authorized(req: Request): boolean {
  // Loopback-bound already; the bearer is the second lock, so another local
  // process cannot drive a login or read a credential out of a session.
  if (!TOKEN) return false
  const header = req.headers.get('authorization') ?? ''
  return header.startsWith('Bearer ') && header.slice(7).trim() === TOKEN
}

async function body(req: Request): Promise<Record<string, unknown>> {
  try {
    const parsed = await req.json()
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}

const server = Bun.serve({
  hostname: '127.0.0.1',
  port: PORT,
  idleTimeout: 60,
  async fetch(req) {
    const url = new URL(req.url)
    const path = url.pathname
    if (path === '/healthz') return json({ ok: true })
    if (!authorized(req)) return fail(401, 'unauthorized')

    if (path === '/v1/providers' && req.method === 'GET') {
      return json({ providers: roster() })
    }

    if (path === '/v1/models' && req.method === 'GET') {
      const provider = url.searchParams.get('provider') ?? ''
      if (!isGeneratedProvider(provider)) return json({ models: [] })
      const models = getBundledModels(provider).map(m => ({
        id: m.id,
        name: 'name' in m && typeof m.name === 'string' ? m.name : m.id,
      }))
      return json({ models })
    }

    if (path === '/v1/login' && req.method === 'POST') {
      const provider = String((await body(req)).provider ?? '')
      try {
        return json(wire(startLogin(provider)))
      } catch (err) {
        return fail(400, err instanceof Error ? err.message : String(err))
      }
    }

    const loginMatch = /^\/v1\/login\/([0-9a-f-]{36})(\/input)?$/.exec(path)
    if (loginMatch) {
      const s = sessions.get(loginMatch[1])
      if (!s) return fail(404, 'unknown login session')
      s.touchedAt = Date.now()
      if (req.method === 'GET' && !loginMatch[2]) {
        const out = wire(s)
        // A credential is read exactly once: the api seals it, and nothing is
        // served a second time out of this process's memory.
        if (s.phase === 'done') sessions.delete(s.id)
        return json(out)
      }
      if (req.method === 'POST' && loginMatch[2]) {
        const value = String((await body(req)).value ?? '')
        const pending = s.pending
        if (!pending) return fail(409, 'nothing is awaiting an answer')
        if (!value && !pending.allowEmpty) return fail(400, 'a value is required')
        pending.resolve(value)
        return json(wire(s))
      }
      if (req.method === 'DELETE') {
        s.abort.abort()
        s.pending?.reject(new Error('cancelled'))
        sessions.delete(s.id)
        return json({ ok: true })
      }
      return fail(405, 'method not allowed')
    }

    if (path === '/v1/refresh' && req.method === 'POST') {
      const b = await body(req)
      const provider = String(b.provider ?? '')
      const credential = b.credential as OAuthCredential | undefined
      if (!provider || credential?.type !== 'oauth') {
        return fail(400, 'provider and an oauth credential are required')
      }
      try {
        const refreshed = await refreshOAuthToken(provider as never, {
          access: credential.access,
          refresh: credential.refresh,
          expires: credential.expires,
          apiEndpoint: credential.apiEndpoint,
          enterpriseUrl: credential.enterpriseUrl,
          projectId: credential.projectId,
          email: credential.email,
          accountId: credential.accountId,
          orgId: credential.orgId,
          orgName: credential.orgName,
          authorizedAt: credential.authorizedAt,
        })
        return json({
          credential: oauthCredential(
            normalizeOAuthCredentialExpiry(provider, refreshed),
            credential.authorizedAt ?? Date.now(),
          ),
        })
      } catch (err) {
        return fail(502, err instanceof Error ? err.message : String(err))
      }
    }

    return fail(404, 'not found')
  },
})

setInterval(sweep, 60_000).unref()
console.log(`[omp-auth] listening on 127.0.0.1:${server.port}`)
