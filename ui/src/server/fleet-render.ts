// The renderer: materialize managed agent versions into a runnable fleet/ dir.
//
//   fleet/agents/<slug>/config.yaml   the agent's Hermes config (from the version)
//   fleet/agents/<slug>/SOUL.md       the agent's soul
//   fleet/docker-compose.yml          generated — one service per managed agent
//   fleet/fleet.json                  the gateway-plane manifest (also written to
//                                     stack/fleet.json, which the bridge watches)
//
// Generated services are derived from the source stack's resolved service block
// (so the ~40 shared env vars and mounts stay faithful), with these changes:
//   • config.yaml/SOUL.md bind mounts point at the rendered files
//   • relative bind mounts become absolute paths into the source stack
//   • named volumes become external references (ai_<name>) — state survives
//   • networks → the external ai_default so the bridge/peers resolve the same
//     DNS name; depends_on/build/ports dropped (deps run in the old project,
//     the bridge reaches agents over the network, host ports retire)
import { mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { parse as parseYaml, stringify as stringifyYaml } from 'yaml'
import { db } from './db/pg'
import type { AgentConfig, AgentDef, AgentVersion } from './agent-defs'

export const STACK_DIR = () => process.env.TALARIA_STACK_DIR ?? '/home/jon/packledger-services/ai/orchestration'
export const FLEET_DIR = () => process.env.TALARIA_FLEET_DIR ?? resolve(process.cwd(), '../fleet')
const BRIDGE_MANIFEST = () =>
  process.env.TALARIA_BRIDGE_MANIFEST ?? resolve(process.cwd(), '../stack/fleet.json')
const SOURCE_COMPOSE_PROJECT = 'ai' // volume/network prefix of the legacy stack

type ComposeService = Record<string, unknown> & {
  build?: unknown
  depends_on?: unknown
  ports?: unknown
  volumes?: string[]
  networks?: unknown
  secrets?: unknown
}

interface RenderTarget {
  def: AgentDef
  version: AgentVersion
}

async function managedAgents(): Promise<RenderTarget[]> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.slug, d.department, d.model, d.display_name as "displayName", d.enabled, d.managed, d.source,
           d.current_version as "currentVersion", d.created_at as "createdAt", d.updated_at as "updatedAt",
           v.id as vid, v.version, v.soul, v.config, v.note, v.created_by as "createdBy", v.created_at as vcreated
    from agent_defs d
    join agent_versions v on v.agent_id = d.id and v.version = d.current_version
    where d.managed and d.enabled
    order by d.slug
  `) as unknown as Array<Record<string, unknown>>
  return rows.map((r) => ({
    def: r as unknown as AgentDef,
    version: {
      id: r.vid,
      agentId: r.id,
      version: r.version,
      soul: r.soul,
      config: r.config,
      note: r.note,
      createdBy: r.createdBy,
      createdAt: r.vcreated,
    } as AgentVersion,
  }))
}

/** Rewrite one bind/volume entry for the generated compose. Returns the entry
 *  plus the named volume it references (if any) for the external volumes map. */
function rewriteMount(entry: string, agentDir: string): { entry: string; namedVolume?: string } {
  const [src, dest, mode] = entry.split(':')
  if (!src || !dest) return { entry }
  const suffix = mode ? `:${mode}` : ''
  if (dest === '/opt/data/config.yaml') return { entry: `${join(agentDir, 'config.yaml')}:${dest}:ro` }
  if (dest === '/opt/data/SOUL.md') return { entry: `${join(agentDir, 'SOUL.md')}:${dest}:ro` }
  if (src.startsWith('./') || src.startsWith('../')) {
    return { entry: `${resolve(STACK_DIR(), src)}:${dest}${suffix}` }
  }
  if (src.startsWith('/')) return { entry }
  // Named volume — reference the legacy project's volume so state survives.
  return { entry: `${src}:${dest}${suffix}`, namedVolume: src }
}

export interface RenderResult {
  agents: string[]
  files: string[]
  warnings: string[]
}

export async function renderFleet(): Promise<RenderResult> {
  const targets = await managedAgents()
  const result: RenderResult = { agents: [], files: [], warnings: [] }

  // Parse in YAML 1.1 — docker compose's own dialect (go-yaml). This matters:
  // `mode: 0400` is OCTAL in 1.1 (256) but decimal 400 in 1.2, which would
  // silently turn a root-only secret into a group-writable one.
  const sourceCompose = parseYaml(await readFile(join(STACK_DIR(), 'docker-compose.yml'), 'utf8'), {
    merge: true,
    version: '1.1',
  }) as { services?: Record<string, ComposeService>; secrets?: Record<string, unknown> }

  const services: Record<string, ComposeService> = {}
  // Secrets referenced by agent services (dex/dewey/dot: litellm_key, gh_token,
  // anthropic_key) — pass their env-sourced definitions through verbatim.
  const secrets: Record<string, unknown> = {}
  const volumes: Record<string, { external: true; name: string } | Record<string, never>> = {}

  // Chassis for created agents: any standard agent service block from the
  // source stack (env/anatomy shared via its anchor), re-stamped per agent.
  const chassisName =
    process.env.TALARIA_CHASSIS_SERVICE ??
    (sourceCompose.services?.['agent-support'] ? 'agent-support' : Object.keys(sourceCompose.services ?? {}).find((s) => s.startsWith('agent-')))

  for (const { def, version } of targets) {
    const agentDir = join(FLEET_DIR(), 'agents', def.slug)
    await mkdir(agentDir, { recursive: true })
    await mkdir(join(agentDir, 'skills'), { recursive: true })

    // Heal docker-made junk: when a container (re)starts while a file
    // bind-mount source is missing (e.g. mid-rename crash), docker resurrects
    // the source as a DIRECTORY — which would make the writes below EISDIR
    // forever after. Clear any directory squatting on a rendered-file path.
    for (const f of ['config.yaml', 'SOUL.md']) {
      const p = join(agentDir, f)
      // Best effort: docker-made junk is root-owned, which we may not be.
      if ((await stat(p).catch(() => null))?.isDirectory()) await rm(p, { recursive: true, force: true }).catch(() => {})
    }

    const raw = (version.config as { raw?: unknown }).raw
    await writeFile(
      join(agentDir, 'config.yaml'),
      `# Rendered by Talaria — ${def.model} v${version.version}. Do not hand-edit; edit in Talaria.\n` +
        // Hermes reads this with PyYAML (YAML 1.1): emit 1.1 so strings like
        // "on"/"off" stay quoted instead of turning into booleans.
        stringifyYaml(raw ?? {}, { version: '1.1' }),
    )
    await writeFile(join(agentDir, 'SOUL.md'), version.soul)
    result.files.push(join(agentDir, 'config.yaml'), join(agentDir, 'SOUL.md'))

    const serviceName = `agent-${def.department}`
    const created = (def as AgentDef & { source?: string }).source === 'created'
    const source = sourceCompose.services?.[created ? (chassisName ?? '') : serviceName]
    if (!source) {
      result.warnings.push(`${def.slug}: no ${created ? 'chassis' : `source service ${serviceName}`} in the stack compose — skipped`)
      continue
    }
    const svc: ComposeService = JSON.parse(JSON.stringify(source)) as ComposeService
    delete svc.build // images are prebuilt; Talaria doesn't rebuild
    delete svc.depends_on // deps (redis, mcp servers) run in the legacy project
    delete svc.ports // the bridge reaches agents over the network; host ports retire

    if (created) {
      // Re-stamp the chassis identity for a brand-new agent.
      const env = (svc.environment ?? {}) as Record<string, unknown>
      env.API_SERVER_KEY = `\${HERMES_KEY_${def.slug.toUpperCase()}}`
      env.API_SERVER_MODEL_NAME = def.model
      svc.environment = env
    }

    if (Array.isArray(svc.volumes)) {
      svc.volumes = svc.volumes.map((v) => {
        const [, dest] = String(v).split(':')
        if (created && dest === '/opt/data') {
          // Fresh state volume in the talaria-fleet project (compose creates it).
          volumes[`hermes-${def.department}`] = {}
          return `hermes-${def.department}:/opt/data`
        }
        if (created && dest === '/opt/dept-skills') {
          return `${join(agentDir, 'skills')}:/opt/dept-skills:ro`
        }
        const { entry, namedVolume } = rewriteMount(String(v), agentDir)
        if (namedVolume) volumes[namedVolume] = { external: true, name: `${SOURCE_COMPOSE_PROJECT}_${namedVolume}` }
        return entry
      })
    }
    svc.networks = ['fleet']
    if (Array.isArray(svc.secrets)) {
      // Entries are either "name" or long-form { source, target, mode, … }.
      // A reference without a resolvable definition is DROPPED (with a warning)
      // — keeping it would make compose reject the whole file and brick every
      // fleet operation, not just this agent.
      svc.secrets = (svc.secrets as Array<string | { source?: string }>).filter((s) => {
        const name = typeof s === 'string' ? s : s.source
        const secretDef = name ? sourceCompose.secrets?.[name] : undefined
        if (name && secretDef) {
          secrets[name] = secretDef
          return true
        }
        result.warnings.push(
          `${serviceName}: secret ${name ?? JSON.stringify(s)} not defined in the source compose — dropped from the rendered service`,
        )
        return false
      })
    }
    services[serviceName] = svc
    result.agents.push(def.model)
  }

  const compose = {
    name: 'talaria-fleet',
    services,
    volumes,
    ...(Object.keys(secrets).length ? { secrets } : {}),
    networks: { fleet: { external: true, name: `${SOURCE_COMPOSE_PROJECT}_default` } },
  }
  const composePath = join(FLEET_DIR(), 'docker-compose.yml')
  await mkdir(FLEET_DIR(), { recursive: true })
  await writeFile(
    composePath,
    `# Generated by Talaria — the managed fleet. Do not hand-edit.\n` + stringifyYaml(compose),
  )
  result.files.push(composePath)

  await writeFleetManifest(result)
  return result
}

/** The gateway-plane manifest: every enabled agent (managed or legacy), keys
 *  drawn from the stack's .env (HERMES_KEY_<SLUG>) — plus one entry per model
 *  tier (`<base>-<alias>`): the agent's own gateway resolves the alias to its
 *  provider/model, so Talaria can route a request to a specific tier. Written
 *  where the bridge watches it, so topology changes apply without a restart. */
async function writeFleetManifest(result: RenderResult): Promise<void> {
  const sql = await db()
  const defs = (await sql`
    select d.slug, d.department, d.model, v.config
    from agent_defs d
    left join agent_versions v on v.agent_id = d.id and v.version = d.current_version
    where d.enabled order by d.slug
  `) as unknown as Array<{ slug: string; department: string; model: string; config: AgentConfig | null }>

  const env = await readFile(join(STACK_DIR(), '.env'), 'utf8').catch(() => '')
  const keys = new Map<string, string>()
  for (const line of env.split('\n')) {
    const m = /^HERMES_KEY_([A-Z0-9_]+)=(.*)$/.exec(line.trim())
    if (m) keys.set(m[1]!.toLowerCase(), m[2]!.trim())
  }

  const manifest = defs.flatMap((d) => {
    const key = keys.get(d.slug) ?? ''
    if (!key) result.warnings.push(`${d.slug}: no HERMES_KEY_${d.slug.toUpperCase()} in stack .env`)
    const url = `http://agent-${d.department}:8642`
    return [
      { model: d.model, url, key },
      ...(d.config?.aliases ?? []).map((a) => ({ model: `${d.model}-${a.name}`, url, key })),
    ]
  })
  const json = JSON.stringify(manifest)
  for (const path of [join(FLEET_DIR(), 'fleet.json'), BRIDGE_MANIFEST()]) {
    await mkdir(dirname(path), { recursive: true })
    await writeFile(path, json)
    result.files.push(path)
  }
}
