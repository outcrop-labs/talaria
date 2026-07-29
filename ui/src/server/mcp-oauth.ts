// OAuth 2.1 for remote MCP servers — the auth most first-party servers
// (Linear, Stripe, Vercel, GitHub) actually speak: no header to paste, the
// flow is negotiated per the MCP authorization spec:
//   401 + WWW-Authenticate → protected-resource metadata → authorization
//   server metadata → DYNAMIC CLIENT REGISTRATION → authorization-code +
//   PKCE → sealed token store → silent refresh.
// Tokens are held per SUBJECT: 'org' (one shared connection) or a user id
// (per-user connected accounts). The gateway injects the right bearer at
// call time; nothing OAuth-shaped ever reaches an agent config.
import { createHash, randomBytes } from 'node:crypto'
import { db } from './db/pg'
import { seal, open } from './secretbox'

export interface OauthConfig {
  resource: string
  authorizationEndpoint: string
  tokenEndpoint: string
  registrationEndpoint: string | null
  scopes: string[]
  client?: { id: string; secretEnc: string | null; redirectUri: string }
}

interface TokenSet {
  accessToken: string
  refreshToken: string | null
  /** Epoch ms; 0 = unknown (treat as long-lived). */
  expiresAt: number
  tokenType: string
}

const b64url = (b: Buffer) => b.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')

// ── Discovery ───────────────────────────────────────────────────────────────

/** Probe a server unauthenticated; a 401 with resource metadata (or the
 *  well-known fallback) means OAuth. Returns the resolved config, or null for
 *  servers that don't advertise it. */
export async function discoverOauth(serverUrl: string): Promise<OauthConfig | null> {
  try {
    const probe = await fetch(serverUrl, {
      method: 'POST',
      headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'initialize',
        params: { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'talaria', version: '1.0' } },
      }),
      signal: AbortSignal.timeout(10_000),
    })
    if (probe.status !== 401 && probe.status !== 403) return null
    const www = probe.headers.get('www-authenticate') ?? ''
    const metaUrl =
      /resource_metadata="?([^",\s]+)"?/.exec(www)?.[1] ?? new URL('/.well-known/oauth-protected-resource', serverUrl).toString()

    const metaR = await fetch(metaUrl, { signal: AbortSignal.timeout(8_000) })
    if (!metaR.ok) return null
    const meta = (await metaR.json()) as { resource?: string; authorization_servers?: string[]; scopes_supported?: string[] }
    const as = meta.authorization_servers?.[0]
    if (!as) return null

    // RFC 8414: for an issuer WITH a path (github.com/login/oauth), the
    // well-known segment goes BETWEEN host and path.
    const asUrl = new URL(as)
    const asPath = asUrl.pathname.replace(/\/$/, '')
    const candidates = [
      `${asUrl.origin}/.well-known/oauth-authorization-server${asPath}`,
      `${asUrl.origin}/.well-known/oauth-authorization-server`,
      `${asUrl.origin}/.well-known/openid-configuration${asPath}`,
      `${as.replace(/\/$/, '')}/.well-known/openid-configuration`,
    ].filter((v, i, a) => a.indexOf(v) === i)
    let asMeta: Record<string, unknown> | null = null
    for (const c of candidates) {
      const r = await fetch(c, { signal: AbortSignal.timeout(8_000) }).catch(() => null)
      if (r?.ok) {
        const j = (await r.json().catch(() => null)) as Record<string, unknown> | null
        if (j?.authorization_endpoint) {
          asMeta = j
          break
        }
      }
    }
    if (!asMeta?.authorization_endpoint || !asMeta.token_endpoint) return null
    return {
      resource: meta.resource ?? serverUrl,
      authorizationEndpoint: String(asMeta.authorization_endpoint),
      tokenEndpoint: String(asMeta.token_endpoint),
      registrationEndpoint: asMeta.registration_endpoint ? String(asMeta.registration_endpoint) : null,
      scopes: meta.scopes_supported ?? ((asMeta.scopes_supported as string[]) ?? []),
    }
  } catch {
    return null
  }
}

/** Detect + persist a server's OAuth config (idempotent; cheap no-op when
 *  already discovered). Returns whether the server is OAuth-shaped. */
export async function ensureOauthConfig(serverId: string, serverUrl: string): Promise<OauthConfig | null> {
  const sql = await db()
  const [row] = (await sql`select oauth from mcp_servers where id = ${serverId}`) as unknown as Array<{ oauth: OauthConfig | null }>
  if (row?.oauth) return row.oauth
  const config = await discoverOauth(serverUrl)
  if (config) await sql`update mcp_servers set oauth = ${sql.json(config as never)}, updated_at = now() where id = ${serverId}`
  return config
}

// ── Client registration + the authorize/callback dance ──────────────────────

async function ensureClient(serverId: string, config: OauthConfig, redirectUri: string): Promise<{ id: string; secret: string | null }> {
  if (config.client && config.client.redirectUri === redirectUri) {
    return { id: config.client.id, secret: config.client.secretEnc ? open(config.client.secretEnc) : null }
  }
  // A manually-configured client (DCR-less providers like GitHub) is pinned
  // to whatever redirect the admin registered — reuse it as-is.
  if (config.client && !config.registrationEndpoint) {
    return { id: config.client.id, secret: config.client.secretEnc ? open(config.client.secretEnc) : null }
  }
  if (!config.registrationEndpoint) {
    throw new Error('this provider requires a pre-registered OAuth app — add its client credentials on the server card')
  }
  const r = await fetch(config.registrationEndpoint, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      client_name: 'Talaria',
      redirect_uris: [redirectUri],
      grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'],
      token_endpoint_auth_method: 'none',
    }),
    signal: AbortSignal.timeout(10_000),
  })
  if (!r.ok) throw new Error(`client registration failed (${r.status})`)
  const reg = (await r.json()) as { client_id: string; client_secret?: string }
  const sql = await db()
  const next: OauthConfig = {
    ...config,
    client: { id: reg.client_id, secretEnc: reg.client_secret ? seal(reg.client_secret) : null, redirectUri },
  }
  await sql`update mcp_servers set oauth = ${sql.json(next as never)}, updated_at = now() where id = ${serverId}`
  return { id: reg.client_id, secret: reg.client_secret ?? null }
}

/** Build the authorization URL and park state+verifier for the callback.
 *  `subject` is 'org' or a user id. */
export async function startOauth(serverId: string, serverUrl: string, subject: string, origin: string): Promise<string> {
  const config = await ensureOauthConfig(serverId, serverUrl)
  if (!config) throw new Error("this server doesn't advertise OAuth")
  let redirectUri = `${origin}/api/mcp/oauth/callback`
  if (config.client && !config.registrationEndpoint) redirectUri = config.client.redirectUri
  const client = await ensureClient(serverId, config, redirectUri)
  const state = b64url(randomBytes(24))
  const verifier = b64url(randomBytes(48))
  const sql = await db()
  await sql`delete from mcp_oauth_states where created_at < now() - interval '30 minutes'`
  await sql`
    insert into mcp_oauth_states (state, server_id, subject, verifier, redirect_uri)
    values (${state}, ${serverId}, ${subject}, ${verifier}, ${redirectUri})
  `
  const url = new URL(config.authorizationEndpoint)
  url.searchParams.set('response_type', 'code')
  url.searchParams.set('client_id', client.id)
  url.searchParams.set('redirect_uri', redirectUri)
  url.searchParams.set('state', state)
  url.searchParams.set('code_challenge', b64url(createHash('sha256').update(verifier).digest()))
  url.searchParams.set('code_challenge_method', 'S256')
  url.searchParams.set('resource', config.resource)
  if (config.scopes.length) url.searchParams.set('scope', config.scopes.join(' '))
  return url.toString()
}

const storeTokens = async (serverId: string, subject: string, t: TokenSet): Promise<void> => {
  const sql = await db()
  await sql`
    insert into mcp_oauth_tokens (server_id, subject, tokens_enc) values (${serverId}, ${subject}, ${seal(JSON.stringify(t))})
    on conflict (server_id, subject) do update set tokens_enc = ${seal(JSON.stringify(t))}, updated_at = now()
  `
}

const parseTokenResponse = (j: Record<string, unknown>): TokenSet => ({
  accessToken: String(j.access_token),
  refreshToken: j.refresh_token ? String(j.refresh_token) : null,
  expiresAt: typeof j.expires_in === 'number' ? Date.now() + j.expires_in * 1000 : 0,
  tokenType: j.token_type ? String(j.token_type) : 'Bearer',
})

/** Exchange the authorization code. Returns where the browser should land. */
export async function handleOauthCallback(state: string, code: string): Promise<{ subject: string; serverId: string }> {
  const sql = await db()
  const [row] = (await sql`
    delete from mcp_oauth_states where state = ${state} returning server_id as "serverId", subject, verifier, redirect_uri as "redirectUri"
  `) as unknown as Array<{ serverId: string; subject: string; verifier: string; redirectUri: string }>
  if (!row) throw new Error('unknown or expired authorization state')
  const [srv] = (await sql`select oauth, url from mcp_servers where id = ${row.serverId}`) as unknown as Array<{
    oauth: OauthConfig | null
    url: string
  }>
  if (!srv?.oauth?.client) throw new Error('server lost its OAuth config mid-flow')
  const body = new URLSearchParams({
    grant_type: 'authorization_code',
    code,
    redirect_uri: row.redirectUri,
    client_id: srv.oauth.client.id,
    code_verifier: row.verifier,
    resource: srv.oauth.resource,
  })
  if (srv.oauth.client.secretEnc) body.set('client_secret', open(srv.oauth.client.secretEnc))
  const r = await fetch(srv.oauth.tokenEndpoint, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body,
    signal: AbortSignal.timeout(15_000),
  })
  const j = (await r.json().catch(() => ({}))) as Record<string, unknown>
  if (!r.ok || !j.access_token) throw new Error(String(j.error_description ?? j.error ?? `token exchange failed (${r.status})`))
  await storeTokens(row.serverId, row.subject, parseTokenResponse(j))
  return { subject: row.subject, serverId: row.serverId }
}

// ── Token use ───────────────────────────────────────────────────────────────

export async function hasOauthTokens(serverId: string, subject: string): Promise<boolean> {
  const sql = await db()
  return (await sql`select 1 as ok from mcp_oauth_tokens where server_id = ${serverId} and subject = ${subject}`).length > 0
}

export async function dropOauthTokens(serverId: string, subject: string): Promise<void> {
  const sql = await db()
  await sql`delete from mcp_oauth_tokens where server_id = ${serverId} and subject = ${subject}`
}

/** A live access token for the subject — silently refreshed when expiring.
 *  Null = not connected (or refresh rejected → tokens dropped, reconnect). */
export async function oauthTokenFor(serverId: string, subject: string): Promise<string | null> {
  const sql = await db()
  const [row] = (await sql`
    select tokens_enc as enc from mcp_oauth_tokens where server_id = ${serverId} and subject = ${subject}
  `) as unknown as Array<{ enc: string }>
  if (!row) return null
  let t: TokenSet
  try {
    t = JSON.parse(open(row.enc)) as TokenSet
  } catch {
    return null
  }
  const stale = t.expiresAt !== 0 && t.expiresAt - Date.now() < 60_000
  if (!stale) return t.accessToken
  if (!t.refreshToken) return t.accessToken // long-shot: let the upstream judge
  const [srv] = (await sql`select oauth from mcp_servers where id = ${serverId}`) as unknown as Array<{ oauth: OauthConfig | null }>
  if (!srv?.oauth?.client) return t.accessToken
  const body = new URLSearchParams({
    grant_type: 'refresh_token',
    refresh_token: t.refreshToken,
    client_id: srv.oauth.client.id,
    resource: srv.oauth.resource,
  })
  if (srv.oauth.client.secretEnc) body.set('client_secret', open(srv.oauth.client.secretEnc))
  const r = await fetch(srv.oauth.tokenEndpoint, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body,
    signal: AbortSignal.timeout(15_000),
  }).catch(() => null)
  const j = ((await r?.json().catch(() => null)) ?? null) as Record<string, unknown> | null
  if (!r?.ok || !j?.access_token) {
    if (r && [400, 401].includes(r.status)) await dropOauthTokens(serverId, subject) // revoked — force reconnect
    return null
  }
  const next = parseTokenResponse(j)
  if (!next.refreshToken) next.refreshToken = t.refreshToken // rotation optional
  await storeTokens(serverId, subject, next)
  return next.accessToken
}

/** Store admin-provided OAuth app credentials (providers without dynamic
 *  registration — GitHub). The redirect URI must be registered with the
 *  provider exactly as shown in the UI. */
export async function setManualOauthClient(
  serverId: string,
  serverUrl: string,
  clientId: string,
  clientSecret: string | null,
  redirectUri: string,
): Promise<void> {
  const config = await ensureOauthConfig(serverId, serverUrl)
  if (!config) throw new Error("this server doesn't advertise OAuth")
  const sql = await db()
  const next: OauthConfig = {
    ...config,
    client: { id: clientId, secretEnc: clientSecret ? seal(clientSecret) : null, redirectUri },
  }
  await sql`update mcp_servers set oauth = ${sql.json(next as never)}, updated_at = now() where id = ${serverId}`
}

/** Surface the flags the UI needs: is this OAuth, does it self-register, is a
 *  client configured. */
export async function oauthMeta(serverId: string): Promise<{ dcr: boolean; clientSet: boolean } | null> {
  const sql = await db()
  const [row] = (await sql`select oauth from mcp_servers where id = ${serverId}`) as unknown as Array<{ oauth: OauthConfig | null }>
  if (!row?.oauth) return null
  return { dcr: row.oauth.registrationEndpoint !== null, clientSet: !!row.oauth.client }
}
