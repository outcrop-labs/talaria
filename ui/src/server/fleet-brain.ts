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
