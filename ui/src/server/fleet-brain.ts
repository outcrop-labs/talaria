// The fleet's brain is Talaria's own gateway. Rather than pointing agents at a
// raw LLM endpoint, the whole fleet routes its tool loop through Talaria's org
// gateway (`/api/llm/v1`) — so every agent call is guarded (confab checks see
// the full tool trace), metered in one ledger, and observable in one place.
//
// This provisions that wiring into the Talaria-owned fleet .env, out of the box:
//   • a `fleet-gateway` LLM key (minted once, kept across renders)
//   • the `gateway_unmetered_keys` setting (fleet usage is metered downstream by
//     the chat/channel/ticket flows, so the gateway skips it to avoid double-count)
//   • LLM_BASE_URL → the gateway self URL, LLM_API_KEY → the key, and a default
//     LLM_MODEL when one isn't pinned.
//
// It runs on every fleet render (idempotent). Because the .env is Talaria-owned,
// nobody hand-edits gateway creds — you configure the real upstream model in-app
// on /models, and both the app and the fleet use it through the gateway.
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'
import { db } from './db/pg'
import { mintKey } from './llm-keys'
import { listEndpoints } from './agent-defs'
import { getSetting, setSetting } from './audit'
import { FLEET_ENV } from './fleet-render'

const GATEWAY_KEY_NAME = 'fleet-gateway'
// Where the fleet reaches Talaria's gateway. In dev the app runs on the host, so
// agents reach it via the docker host-gateway (`extra_hosts` in the chassis) on
// :5273. In a containerized stack, set TALARIA_GATEWAY_SELF_URL to the app's
// service DNS (e.g. http://talaria-ui:3000/api/llm/v1).
const DEFAULT_SELF_URL = 'http://host.docker.internal:5273/api/llm/v1'
const selfUrl = () => process.env.TALARIA_GATEWAY_SELF_URL || DEFAULT_SELF_URL

const sha = (s: string) => createHash('sha256').update(s).digest('hex')
/** A gateway URL is one that ends in the gateway path — safe for Talaria to own
 *  and migrate. Any other value means the operator deliberately pointed the
 *  fleet at a raw upstream, so we leave it alone. */
const isGatewayUrl = (u: string) => /\/api\/llm\/v1\/?$/.test(u.trim())

function readEnvLine(content: string, key: string): string | null {
  const m = new RegExp(`^${key}=(.*)$`, 'm').exec(content)
  return m ? m[1]!.trim() : null
}

/** Set or replace a KEY=value line; append if absent. */
function upsertEnvLine(content: string, key: string, value: string): string {
  const re = new RegExp(`^${key}=.*$`, 'm')
  const line = `${key}=${value}`
  if (re.test(content)) return content.replace(re, line)
  return `${content.replace(/\n*$/, '')}\n${line}\n`
}

/** Keep the current key if it's a live `fleet-gateway` key; otherwise mint a
 *  fresh one (revoking any stale rows) under an admin owner. Returns the plaintext
 *  secret to write into the fleet .env, or null if there's no user to own it yet. */
async function ensureFleetGatewayKey(currentKey: string | null): Promise<{ secret: string; rotated: boolean } | null> {
  const sql = await db()
  if (currentKey?.startsWith('tlk_')) {
    const live = await sql`
      select 1 from llm_api_keys
      where name = ${GATEWAY_KEY_NAME} and revoked_at is null and key_hash = ${sha(currentKey)}`
    if (live.length) return { secret: currentKey, rotated: false }
  }
  const owner = (await sql`
    select id from users order by (role = 'admin') desc, created_at asc limit 1`) as unknown as Array<{ id: string }>
  const ownerId = owner[0]?.id
  if (!ownerId) return null // no users yet — nothing to own the key; try again next render
  await sql`update llm_api_keys set revoked_at = now() where name = ${GATEWAY_KEY_NAME} and revoked_at is null`
  const { secret } = await mintKey(ownerId, GATEWAY_KEY_NAME)
  return { secret, rotated: true }
}

/** First gateway-resolvable model — a sane default LLM_MODEL so freshly-added
 *  endpoints light up the fleet without hand-editing. Null if none configured. */
async function defaultGatewayModel(): Promise<string | null> {
  const eps = await listEndpoints().catch(() => [])
  const ep = eps.find((e) => e.class === 'local' && e.models.length > 0) ?? eps.find((e) => e.models.length > 0)
  return ep?.models[0] ?? null
}

export interface GatewayModels {
  /** Every model name Talaria's gateway can resolve (across all endpoints). */
  served: Set<string>
  /** Default model to fall back to for names the gateway doesn't serve yet. */
  fallback: string | null
}

/** The set of models Talaria's gateway serves, for the config transform below. */
export async function gatewayModelSet(): Promise<GatewayModels> {
  const eps = await listEndpoints().catch(() => [])
  const served = new Set<string>()
  for (const e of eps) for (const m of e.models) served.add(m)
  const local = eps.find((e) => e.class === 'local' && e.models.length > 0) ?? eps.find((e) => e.models.length > 0)
  return { served, fallback: local?.models[0] ?? null }
}

const GATEWAY_BASE_URL = '${LLM_BASE_URL}'
const GATEWAY_API_KEY = '${LLM_API_KEY}'

// Imported agents carry legacy litellm-router model names. Map them to the real
// provider model ids Talaria's gateway serves. As agents are redesigned they use
// real ids directly and this bridge shrinks to nothing.
const LEGACY_MODEL_MAP: Record<string, string> = {
  glm: 'z-ai/glm-5.2', // legacy litellm "glm" → OpenRouter's Z.AI GLM
}

/** An LLM model spec inside an agent config: has a model name and some endpoint
 *  marker (base_url / provider / key_env / api_key). MCP servers (url + headers,
 *  no `model`) and other blocks are left untouched. */
function isModelSpec(o: unknown): o is Record<string, unknown> {
  if (!o || typeof o !== 'object' || Array.isArray(o)) return false
  const r = o as Record<string, unknown>
  return typeof r.model === 'string' && ('base_url' in r || 'provider' in r || 'key_env' in r || 'api_key' in r)
}

/** Route every LLM model spec in a raw agent config through Talaria's gateway:
 *  base_url/api_key point at the gateway, provider becomes openai-compatible, and
 *  the model name is kept (or falls back to the default if the gateway doesn't
 *  serve it yet). Un-interweaves agents from litellm/inference-router/anthropic —
 *  they have exactly one upstream, and Talaria routes to the real providers. */
export function routeConfigThroughGateway(
  raw: unknown,
  models: GatewayModels,
  warn: (m: string) => void,
): unknown {
  const walk = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(walk)
    if (v && typeof v === 'object') {
      if (isModelSpec(v)) {
        const spec = v as Record<string, unknown>
        const name = spec.model as string
        const canonical = LEGACY_MODEL_MAP[name] ?? name
        const model = models.served.has(canonical) ? canonical : (models.fallback ?? canonical)
        // Warn only when we couldn't route to the intended model (unserved →
        // fallback); a known legacy remap (glm → z-ai/glm-5.2) is intentional.
        if (!models.served.has(canonical)) warn(`model "${name}" not served by the gateway — routing to "${model}"`)
        const { key_env: _drop, ...rest } = spec
        return { ...rest, model, base_url: GATEWAY_BASE_URL, api_key: GATEWAY_API_KEY, provider: 'custom' }
      }
      return Object.fromEntries(Object.entries(v as Record<string, unknown>).map(([k, val]) => [k, walk(val)]))
    }
    return v
  }
  return walk(raw)
}

export interface GatewayBrain {
  url: string | null
  model: string | null
  keyRotated: boolean
  managed: boolean // false when the operator pointed the fleet at a non-gateway upstream
}

/** Provision (or refresh) the fleet's gateway brain in the Talaria-owned .env.
 *  Idempotent and best-effort — a failure here never blocks a fleet render. */
export async function ensureGatewayBrain(): Promise<GatewayBrain> {
  const envPath = FLEET_ENV()
  await mkdir(dirname(envPath), { recursive: true })
  const content = await readFile(envPath, 'utf8').catch(() => '')

  // The gateway skips metering for this key (usage is counted downstream).
  const unmetered = await getSetting<string[]>('gateway_unmetered_keys', [GATEWAY_KEY_NAME])
  if (!unmetered.includes(GATEWAY_KEY_NAME)) await setSetting('gateway_unmetered_keys', [...unmetered, GATEWAY_KEY_NAME])

  const cur = readEnvLine(content, 'LLM_BASE_URL')
  if (cur && !isGatewayUrl(cur)) {
    // Operator override: a raw upstream. Respect it — no gateway brain.
    return { url: cur, model: readEnvLine(content, 'LLM_MODEL'), keyRotated: false, managed: false }
  }

  const url = selfUrl()
  const key = await ensureFleetGatewayKey(readEnvLine(content, 'LLM_API_KEY'))

  let next = content
  if (!next.includes('# talaria-managed (gateway brain)')) {
    next =
      `${next.replace(/\n*$/, '')}\n\n` +
      `# talaria-managed (gateway brain) — the fleet's default LLM is Talaria's org\n` +
      `# gateway, so every agent call is guarded, metered, and observable. Configure\n` +
      `# the real upstream model in-app on /models; don't hand-edit these three lines.\n`
  }
  next = upsertEnvLine(next, 'LLM_BASE_URL', url)
  if (key) next = upsertEnvLine(next, 'LLM_API_KEY', key.secret)

  let model = readEnvLine(next, 'LLM_MODEL')
  if (!model) {
    model = (await defaultGatewayModel()) ?? ''
    if (model) next = upsertEnvLine(next, 'LLM_MODEL', model)
  }

  if (next !== content) await writeFile(envPath, next)
  return { url, model: model || null, keyRotated: key?.rotated ?? false, managed: true }
}
