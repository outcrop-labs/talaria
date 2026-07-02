// One-shot importer: ingest the existing hand-maintained Hermes stack
// (ai/orchestration) into Talaria's agent_defs / agent_versions / llm_endpoints.
// Idempotent — re-running only creates a new version when something changed.
//
//   TALARIA_STACK_DIR  the orchestration dir (default: the packledger layout)
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { parse as parseYaml } from 'yaml'
import {
  addEndpointModels,
  addVersionIfChanged,
  ensureEndpoint,
  upsertAgentDef,
  type AgentConfig,
  type ModelTarget,
} from './agent-defs'

const STACK_DIR = () =>
  process.env.TALARIA_STACK_DIR ?? '/home/jon/packledger-services/ai/orchestration'

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
 *  plane; proxies to cloud models (litellm) and first-party providers = cloud. */
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

export interface ImportResult {
  agents: Array<{ slug: string; model: string; version: number; created: boolean }>
  errors: string[]
}

export async function importFleetFromStack(actor: string): Promise<ImportResult> {
  const dir = STACK_DIR()
  const result: ImportResult = { agents: [], errors: [] }

  let roster: RosterEntry[]
  try {
    const doc = parseYaml(await readFile(join(dir, 'agents.yaml'), 'utf8')) as { agents?: RosterEntry[] } | RosterEntry[]
    roster = Array.isArray(doc) ? doc : (doc.agents ?? [])
  } catch (e) {
    result.errors.push(`agents.yaml: ${(e as Error).message}`)
    return result
  }

  for (const entry of roster) {
    const slug = entry.slug?.trim()
    const department = entry.department?.trim()
    if (!slug || !department) continue
    try {
      const agentDir = join(dir, 'agents', department)
      const soul = await readFile(join(agentDir, 'SOUL.md'), 'utf8').catch(() => '')
      const rawCfg = (parseYaml(await readFile(join(agentDir, 'config.yaml'), 'utf8').catch(() => '')) ?? {}) as Record<
        string,
        unknown
      >

      const main = rawCfg.model && typeof rawCfg.model === 'object' ? await targetOf(rawCfg.model as RawModelBlock) : undefined

      const aliases: AgentConfig['aliases'] = []
      const rawAliases = (rawCfg.model_aliases ?? {}) as Record<string, RawModelBlock | string>
      for (const [name, v] of Object.entries(rawAliases)) {
        if (typeof v === 'string') {
          // Shorthand "provider/model"
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

      const def = await upsertAgentDef({
        slug,
        department,
        displayName: entry.name?.trim() || slug.charAt(0).toUpperCase() + slug.slice(1),
      })
      const config: AgentConfig = { main, aliases, fallbacks, plugins, mcpServers, raw: rawCfg }
      const { version, created } = await addVersionIfChanged(def.id, {
        soul,
        config,
        note: `imported from ${dir}`,
        createdBy: actor,
      })
      result.agents.push({ slug, model: def.model, version, created })
    } catch (e) {
      result.errors.push(`${slug}: ${(e as Error).message}`)
    }
  }
  return result
}
