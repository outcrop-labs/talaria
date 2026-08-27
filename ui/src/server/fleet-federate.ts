// Federation: bring OUTSIDE agents into Talaria as first-class citizens. Reads
// a Hermes-format directory (agents.yaml roster + agents/<department>/SOUL.md
// + config.yaml), then creates each agent NATIVELY: Talaria def + v1, fresh
// gateway key, model targets mapped into the endpoint registry, skills copied
// into the fleet dir, run on the Talaria chassis with a fresh state volume.
// Nothing keeps pointing at the source directory afterwards — it's a one-way
// door, re-runnable (existing slugs are reported and skipped).
import { cp, readFile, stat } from 'node:fs/promises'
import { join } from 'node:path'
import { parse as parseYaml } from 'yaml'
import { db } from './db/pg'
import { addVersionIfChanged, ensureEndpoint, addEndpointModels, upsertAgentDef, DEPT_RE, SLUG_RE, type AgentConfig, type ModelTarget } from './agent-defs'
import { ensureAgentKey } from './fleet-create'
import { FLEET_DIR } from './fleet-render'

interface RosterEntry {
  slug?: string
  name?: string
  department?: string
  description?: string
}

interface RawModelBlock {
  provider?: string
  model?: string
  base_url?: string
  api_key?: string
  context_length?: number | string
}

/** "${LLM_API_KEY}" → "LLM_API_KEY"; literal keys are never stored. */
const keyEnvOf = (apiKey?: string): string | null => {
  const m = /^\$\{([A-Z0-9_]+)\}$/.exec(apiKey?.trim() ?? '')
  return m ? m[1]! : null
}

/** Endpoint identity + class for a model target. Local = the LAN inference
 *  plane; proxies to cloud models and first-party providers = cloud. */
function endpointFor(block: RawModelBlock): { name: string; class: 'local' | 'cloud'; provider: string; baseUrl: string | null } {
  const provider = (block.provider ?? 'custom').toLowerCase()
  const baseUrl = block.base_url?.replace(/\/$/, '') ?? null
  if (baseUrl) {
    const host = baseUrl.replace(/^https?:\/\//, '').split(/[/:]/)[0] ?? baseUrl
    const isLocal = /inference-router|vllm|ollama|llama|localhost|127\.0\.0\.1/.test(host) && !/litellm/.test(host)
    return { name: host, class: isLocal ? 'local' : 'cloud', provider, baseUrl }
  }
  return { name: provider, class: 'cloud', provider, baseUrl: null }
}

async function targetOf(block: RawModelBlock): Promise<ModelTarget> {
  const ep = endpointFor(block)
  await ensureEndpoint({
    name: ep.name,
    provider: ep.provider,
    baseUrl: ep.baseUrl,
    class: ep.class,
    apiKeyEnv: keyEnvOf(block.api_key),
    contextLength: typeof block.context_length === 'number' ? block.context_length : null,
  })
  if (block.model) await addEndpointModels(ep.name, [block.model])
  const ctx = Number(block.context_length)
  return {
    endpoint: ep.name,
    model: block.model ?? '',
    ...(Number.isFinite(ctx) && ctx > 0 ? { contextLength: ctx } : {}),
  }
}

export interface FederateResult {
  agents: Array<{ slug: string; model: string; status: 'federated' | 'exists' }>
  errors: string[]
}

export async function federateFromDir(dir: string, actor: string): Promise<FederateResult> {
  const result: FederateResult = { agents: [], errors: [] }
  if (!(await stat(dir).catch(() => null))?.isDirectory()) {
    result.errors.push(`${dir}: not a directory on the server`)
    return result
  }

  let roster: RosterEntry[]
  try {
    const doc = parseYaml(await readFile(join(dir, 'agents.yaml'), 'utf8')) as { agents?: RosterEntry[] } | RosterEntry[]
    roster = Array.isArray(doc) ? doc : (doc.agents ?? [])
  } catch (e) {
    result.errors.push(`agents.yaml: ${(e as Error).message}`)
    return result
  }

  const sql = await db()
  for (const entry of roster) {
    const slug = entry.slug?.trim()
    const department = entry.department?.trim()
    if (!slug || !department) continue
    // Same alphabet the interactive path (createAgent) enforces, checked BEFORE
    // these strings reach an .env line (ensureAgentKey), a path join, or a
    // RegExp — an imported roster is third-party input, not operator typing.
    if (!SLUG_RE.test(slug) || !DEPT_RE.test(department)) {
      result.errors.push(`${slug || '(unnamed)'}: slug must be lowercase alphanumeric and department lowercase-kebab (e.g. "analyst" / "research")`)
      continue
    }
    try {
      const exists = await sql`select 1 from agent_defs where slug = ${slug}`
      if (exists.length) {
        result.agents.push({ slug, model: '', status: 'exists' })
        continue
      }

      const agentDir = join(dir, 'agents', department)
      const soul = await readFile(join(agentDir, 'SOUL.md'), 'utf8').catch(() => '')
      const rawCfg = (parseYaml(await readFile(join(agentDir, 'config.yaml'), 'utf8').catch(() => '')) ?? {}) as Record<string, unknown>

      const main = rawCfg.model && typeof rawCfg.model === 'object' ? await targetOf(rawCfg.model as RawModelBlock) : undefined
      const aliases: AgentConfig['aliases'] = []
      for (const [name, v] of Object.entries((rawCfg.model_aliases ?? {}) as Record<string, RawModelBlock | string>)) {
        if (typeof v === 'string') {
          const [provider, ...rest] = v.split('/')
          aliases.push({ name, ...(await targetOf({ provider, model: rest.join('/') })) })
        } else {
          aliases.push({ name, ...(await targetOf(v)) })
        }
      }
      const fallbacks: ModelTarget[] = []
      for (const f of (rawCfg.fallback_providers as RawModelBlock[] | undefined) ?? []) {
        fallbacks.push(await targetOf(f))
      }
      const plugins = ((rawCfg.plugins as { enabled?: string[] } | undefined)?.enabled ?? []).filter(
        (p): p is string => typeof p === 'string',
      )
      const mcpServers = Object.keys((rawCfg.mcp_servers as Record<string, unknown> | undefined) ?? {})

      // Native creation: fresh key, source 'created' (fresh state volume in
      // OUR orchestration — provenance lives in the version note).
      await ensureAgentKey(slug)
      const def = await upsertAgentDef({
        slug,
        department,
        displayName: entry.name?.trim() || slug.charAt(0).toUpperCase() + slug.slice(1),
        source: 'created',
      })
      await sql`update agent_defs set managed = true, updated_at = now() where id = ${def.id}`
      await addVersionIfChanged(def.id, {
        soul,
        config: { main, aliases, fallbacks, plugins, mcpServers, raw: rawCfg } as AgentConfig,
        note: `federated from ${dir}`,
        createdBy: actor,
      })

      // Skills come WITH the agent, into Talaria's own roots.
      await cp(join(agentDir, 'skills'), join(FLEET_DIR(), 'agents', slug, 'skills'), {
        recursive: true,
        force: false,
        errorOnExist: false,
      }).catch(() => {})

      result.agents.push({ slug, model: def.model, status: 'federated' })
    } catch (e) {
      result.errors.push(`${slug}: ${(e as Error).message}`)
    }
  }
  return result
}
