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
import { createHash } from 'node:crypto'
import { cp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { parse as parseYaml, stringify as stringifyYaml } from 'yaml'
import { db } from './db/pg'
import { resolveWorkbench, type WorkbenchProfile } from './workbench'
import { materializeAgentSecrets } from './agent-secrets'
import { ensureGatewayBrain, gatewayModelSet, routeConfigThroughGateway } from './fleet-brain'
import { ensureMcpService } from './mcp-service'
import { serversForAgent } from './mcp-registry'

/** The MCP gateway base as fleet containers reach it — the UI server over the
 *  docker host bridge (same pattern as the talaria-mcp fleet URL). */
const MCP_GW_BASE = () =>
  process.env.TALARIA_MCP_GW_URL ?? `http://host.docker.internal:${process.env.PORT ?? 5273}/api/mcp/gw`
import { orgProfile, orgSoulHeader, toolkitSoulHeader, voiceSoulHeader } from './org'
import { getGuardConfig, guardCoachingFor } from './guardrails'
import type { AgentConfig, AgentDef, AgentVersion } from './agent-defs'

export const FLEET_DIR = () => process.env.TALARIA_FLEET_DIR ?? resolve(process.cwd(), '../fleet')
/** The fleet's env file (agent keys + compose interpolation) — Talaria-owned. */
export const FLEET_ENV = () => join(FLEET_DIR(), '.env')
/** The chassis every agent renders from: one service block + per-slug extras.
 *  Talaria-owned (extracted once at cutover from the legacy stack). */
const CHASSIS_FILE = () => process.env.TALARIA_CHASSIS_FILE ?? join(FLEET_DIR(), 'chassis.yml')

/** fleet/.env must carry TALARIA_AGENT_KEY so compose can interpolate it into
 *  each agent's env (the toolkit MCP header reads it there). Append-once. */
// Repo-shipped fleet skills (scripts/skills/*) seed into the fleet's shared
// skills root on render. Pristine copies (byte-identical to what was seeded,
// tracked in .seeds.json) follow canonical updates; a copy the admin edited
// via the skills UI is never clobbered. fleet/ itself is gitignored; this is
// how canonical skills like talaria-toolkit reach every install.
const SEED_SKILLS_DIR = () => resolve(process.cwd(), '../scripts/skills')

/** Content hash of a skill dir: sorted relative paths + file bytes. */
async function skillDirHash(root: string): Promise<string> {
  const h = createHash('sha256')
  const walk = async (dir: string, rel: string): Promise<void> => {
    const entries = (await readdir(dir, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))
    for (const e of entries) {
      if (e.name.startsWith('.')) continue
      const p = join(dir, e.name)
      const r = rel ? `${rel}/${e.name}` : e.name
      if (e.isDirectory()) await walk(p, r)
      else if (e.isFile()) {
        h.update(r)
        h.update('\0')
        h.update(await readFile(p))
      }
    }
  }
  await walk(root, '')
  return h.digest('hex')
}

async function seedSharedSkills(): Promise<void> {
  const dest = join(FLEET_DIR(), 'skills')
  await mkdir(dest, { recursive: true })
  const manifestPath = join(dest, '.seeds.json')
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8').catch(() => '{}')) as Record<string, string>
  let dirty = false
  const seeds = await readdir(SEED_SKILLS_DIR(), { withFileTypes: true }).catch(() => [])
  for (const seed of seeds) {
    if (!seed.isDirectory()) continue
    const src = join(SEED_SKILLS_DIR(), seed.name)
    const target = join(dest, seed.name)
    try {
      const srcHash = await skillDirHash(src)
      if (!(await stat(target).catch(() => null))) {
        await cp(src, target, { recursive: true })
        manifest[seed.name] = srcHash
        dirty = true
        continue
      }
      if (manifest[seed.name] === srcHash) continue // already carrying this seed
      const targetHash = await skillDirHash(target)
      if (targetHash === srcHash) {
        // In sync (e.g. pre-manifest install that never diverged) — adopt.
        manifest[seed.name] = srcHash
        dirty = true
      } else if (manifest[seed.name] === targetHash) {
        // Pristine copy of an older seed — carry the canonical update forward.
        await rm(target, { recursive: true, force: true })
        await cp(src, target, { recursive: true })
        manifest[seed.name] = srcHash
        dirty = true
      } else if (!manifest[seed.name]) {
        // Pre-manifest install that differs from today's seed: admin edit or
        // stale canonical — can't tell, so never clobber. Update via skills UI.
        console.warn(`[fleet] skill ${seed.name} predates seed tracking and differs from canonical — left as-is`)
      }
      // else: admin-edited — theirs wins, silently.
    } catch (e) {
      console.error(`[fleet] seeding skill ${seed.name} failed:`, e instanceof Error ? e.message : e)
    }
  }
  if (dirty) await writeFile(manifestPath, JSON.stringify(manifest, null, 2))
}

async function ensureFleetEnvKey(): Promise<void> {
  const key = process.env.TALARIA_AGENT_KEY
  if (!key) return
  const envPath = FLEET_ENV()
  const current = await readFile(envPath, 'utf8').catch(() => '')
  if (/^TALARIA_AGENT_KEY=/m.test(current)) return
  await writeFile(envPath, `${current.replace(/\n?$/, '\n')}TALARIA_AGENT_KEY=${key}\n`)
}

/** The EXTERNAL docker network the whole fleet joins (compose never creates
 *  external networks — fleet-docker ensures it exists before any `up`). */
export async function fleetNetworkName(): Promise<string> {
  const text = await readFile(CHASSIS_FILE(), 'utf8').catch(() => null)
  const chassis = text ? (parseYaml(text, { merge: true, version: '1.1' }) as Chassis) : null
  return chassis?.network?.name ?? 'talaria'
}
// Docker-level names inherited from the pre-Talaria stack: imported agents'
// state volumes (ai_hermes-<dept>) and the shared infra network (ai_default).
// These are volume/network NAMES, not a code dependency on that repo.
const LEGACY_DOCKER_PROJECT = 'ai'

type ComposeService = Record<string, unknown> & {
  env_file?: string[]
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

// The host the app reaches agents on. Loopback in dev (host app + published
// ports); set to a service DNS name for a fully containerized deployment.
export const AGENT_HOST = () => process.env.TALARIA_AGENT_HOST ?? '127.0.0.1'
const GATEWAY_PORT_BASE = 8770

/** Assign each managed agent a stable loopback port (persisted, never reused),
 *  so the rendered compose and the manifest agree across membership changes. */
async function ensureGatewayPorts(slugs: string[]): Promise<Map<string, number>> {
  const sql = await db()
  const existing = (await sql`
    select slug, gateway_port as port from agent_defs where gateway_port is not null
  `) as unknown as Array<{ slug: string; port: number }>
  const map = new Map<string, number>(existing.map((r) => [r.slug, r.port]))
  let next = (map.size ? Math.max(...map.values()) : GATEWAY_PORT_BASE - 1) + 1
  for (const slug of slugs) {
    if (map.has(slug)) continue
    const port = next++
    await sql`update agent_defs set gateway_port = ${port} where slug = ${slug}`
    map.set(slug, port)
  }
  return map
}

async function managedAgents(): Promise<RenderTarget[]> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.slug, d.department, d.model, d.display_name as "displayName", d.enabled, d.managed, d.source,
           d.role, d.workbench, d.workbench_profile as "workbenchProfile", d.workbench_harness as "workbenchHarness",
           d.active_slot as "activeSlot",
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

interface ChassisExtras {
  environment?: Record<string, unknown>
  volumes?: string[]
  secrets?: unknown[]
}

interface Chassis {
  service: ComposeService
  /** Per-agent additions beyond the uniform chassis, keyed by slug. */
  extras?: Record<string, ChassisExtras>
  /** Shared named-volume definitions (workspaces, repos, kanban). */
  volumes?: Record<string, unknown>
  /** Secret definitions the extras may reference. */
  secrets?: Record<string, unknown>
  /** External docker network the fleet joins (fresh installs use their own;
   *  this machine's legacy default is ai_default). */
  network?: { name: string }
}

export interface RenderResult {
  agents: string[]
  files: string[]
  warnings: string[]
}

/** During a roll: additionally render this agent's INCOMING slot alongside its
 *  active one, on the given port. The manifest keeps pointing at the active
 *  port — cutover is a DB update + a plain re-render after health. */
export interface RollOverlay {
  slug: string
  slot: 'a' | 'b'
  port: number
}

/** The next unclaimed loopback port for an incoming slot. */
export async function nextFreePort(): Promise<number> {
  const sql = await db()
  const rows = (await sql`select coalesce(max(gateway_port), ${GATEWAY_PORT_BASE - 1}) as m from agent_defs`) as unknown as Array<{ m: number }>
  return Math.max(rows[0]!.m, GATEWAY_PORT_BASE - 1) + 1
}

export async function renderFleet(opts: { roll?: RollOverlay } = {}): Promise<RenderResult> {
  const targets = await managedAgents()
  const result: RenderResult = { agents: [], files: [], warnings: [] }

  // The fleet's default brain is Talaria's own gateway: provision the gateway
  // credential into the Talaria-owned .env so agents route through /api/llm/v1
  // (guarded + metered + observable). Best-effort — never blocks a render.
  const brain = await ensureGatewayBrain().catch((e: Error) => {
    result.warnings.push(`gateway brain: ${e.message}`)
    return null
  })
  if (brain?.managed && !brain.model) {
    result.warnings.push('gateway brain: no model configured yet — add an LLM endpoint on /models to give agents a brain')
  }

  // Parse in YAML 1.1 — docker compose's own dialect (go-yaml). This matters:
  // `mode: 0400` is OCTAL in 1.1 (256) but decimal 400 in 1.2, which would
  // silently turn a root-only secret into a group-writable one.
  const chassisText = await readFile(CHASSIS_FILE(), 'utf8').catch(() => null)
  if (!chassisText) {
    throw new Error(`fleet chassis missing at ${CHASSIS_FILE()} — the harness cannot render agents without it`)
  }
  const chassis = parseYaml(chassisText, { merge: true, version: '1.1' }) as Chassis
  if (!chassis?.service) throw new Error(`fleet chassis at ${CHASSIS_FILE()} has no "service" block`)

  // Every rendered soul opens with the toolkit contract (always) and the
  // organization context (when configured) — Talaria-first tool use is the
  // DEFAULT for every agent, not a per-soul nicety.
  const orgHeader = orgSoulHeader(await orgProfile())
  const soulHeader = [orgHeader, voiceSoulHeader(), toolkitSoulHeader()].filter(Boolean).join('\n\n')
  // Guard coaching (opt-in): repeated findings become per-agent behavioral
  // notes — templated counts + advice only, never flagged content, delivered
  // at render rather than mid-conversation (see guardrails.ts).
  const coachOn = (await getGuardConfig()).coach

  // Agents' configs point at the toolkit MCP — make sure it's actually up,
  // and that the compose env can interpolate the fleet key into the header.
  ensureMcpService()
  await ensureFleetEnvKey()
  await seedSharedSkills()

  // Every agent's LLM specs are rewritten to route through Talaria's gateway —
  // model names the gateway doesn't serve fall back to the default (warned once).
  const gwModels = await gatewayModelSet()
  const remapped = new Set<string>()
  // Stable host port per agent — the app calls each persona gateway directly.
  const ports = await ensureGatewayPorts(targets.map((t) => t.def.slug))

  const services: Record<string, ComposeService> = {}
  const secrets: Record<string, unknown> = {}
  const volumes: Record<string, { external: true; name: string } | Record<string, never> | unknown> = {
    ...(chassis.volumes ?? {}),
  }

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
    // Un-interweave: all model tiers point at Talaria's gateway, never at
    // litellm/inference-router/anthropic directly.
    const routed = routeConfigThroughGateway(raw ?? {}, gwModels, (m) => remapped.add(m)) as Record<string, unknown>
    // Every agent gets the Talaria toolkit MCP (tickets, artifacts, channels,
    // KB, save-image — the whole safe surface) — served by talaria-mcp in
    // fleet HTTP mode, identity via the injected X-Agent-Name header, keyed by
    // the fleet agent key (env-interpolated in the container).
    routed.mcp_servers = {
      ...((routed.mcp_servers as Record<string, unknown> | undefined) ?? {}),
      talaria: {
        // Through the MCP gateway (not the service directly): the org's
        // per-agent/per-person tool subsets for the toolkit apply to every
        // call, same as any registry server.
        url: `${MCP_GW_BASE()}/talaria`,
        headers: { 'X-Agent-Name': def.model, 'X-Api-Key': '${TALARIA_AGENT_KEY}' },
      },
    }
    // The agent's CHOSEN coding harness, as an MCP server on its own config
    // (stdio, in-sandbox) — the agent drives it with tools, not raw output.
    // Only when the profile ships a real image (binaries present).
    {
      const wbc = await resolveWorkbench({
        department: def.department,
        role: (def as unknown as { role?: string | null }).role ?? null,
        workbench: ((def as unknown as { workbench?: string }).workbench ?? 'auto') as 'off' | 'auto' | 'on',
        workbenchProfile: (def as unknown as { workbenchProfile?: string | null }).workbenchProfile ?? null,
      }).catch(() => null)
      if (wbc?.image) {
        const { listHarnessDefs } = await import('./workbench-harnesses')
        const pick = (def as unknown as { workbenchHarness?: string | null }).workbenchHarness
        const chosen = pick && wbc.harnesses.includes(pick) ? pick : wbc.harnesses[0]
        const h = (await listHarnessDefs()).find((x) => x.slug === chosen)
        if (h?.mcpServe) {
          ;(routed.mcp_servers as Record<string, unknown>)[h.slug] = { command: h.mcpServe.command, args: h.mcpServe.args }
        }
      }
    }
    // Org-registry MCP servers (Manage → MCP) ride in as GATEWAY URLs — the
    // agent never sees an upstream address or credential, and the gateway
    // enforces its tool allowlist server-side. Config re-renders on registry
    // changes; Hermes re-reads on mtime, so no restart needed.
    for (const srv of await serversForAgent(def.model)) {
      const entry: Record<string, unknown> = {
        url: `${MCP_GW_BASE()}/${srv.name}`,
        headers: { 'X-Agent-Name': def.model, 'X-Api-Key': '${TALARIA_AGENT_KEY}' },
      }
      if (srv.timeoutSecs) entry.timeout = srv.timeoutSecs
      ;(routed.mcp_servers as Record<string, unknown>)[srv.name] = entry
    }
    // Hermes only discovers skills outside ~/.hermes/skills via
    // skills.external_dirs — without this the /opt/skills + /opt/dept-skills
    // mounts exist in the container but the skill registry never scans them.
    // config.yaml is a bind-mounted file and Hermes caches on its mtime, so
    // this lands in running agents on the next invocation, no restart.
    const skillsCfg = (routed.skills as Record<string, unknown> | undefined) ?? {}
    const extDirs = Array.isArray(skillsCfg.external_dirs) ? (skillsCfg.external_dirs as unknown[]).map(String) : []
    routed.skills = {
      ...skillsCfg,
      external_dirs: [...new Set([...extDirs, '/opt/skills', '/opt/dept-skills'])],
    }
    await writeFile(
      join(agentDir, 'config.yaml'),
      `# Rendered by Talaria — ${def.model} v${version.version}. Do not hand-edit; edit in Talaria.\n` +
        // Hermes reads this with PyYAML (YAML 1.1): emit 1.1 so strings like
        // "on"/"off" stay quoted instead of turning into booleans.
        stringifyYaml(routed, { version: '1.1' }),
    )
    // The rendered soul carries the organization header (a projection — the
    // stored soul stays clean; the header tracks the org settings), so every
    // agent knows whose team it's on, including ones authored before org config.
    const coaching = coachOn ? await guardCoachingFor(def.model).catch(() => '') : ''
    await writeFile(join(agentDir, 'SOUL.md'), `${[soulHeader, coaching].filter(Boolean).join('\n\n')}\n\n${version.soul}`)
    result.files.push(join(agentDir, 'config.yaml'), join(agentDir, 'SOUL.md'))

    // The service name carries the active slot ('a' = bare, 'b' = "-b") so a
    // roll can run both generations side by side under one compose project.
    const slot = ((def as unknown as { activeSlot?: string }).activeSlot === 'b' ? 'b' : 'a') as 'a' | 'b'
    const serviceName = `agent-${def.department}${slot === 'b' ? '-b' : ''}`
    const imported = (def as AgentDef & { source?: string }).source === 'imported'
    const extras = chassis.extras?.[def.slug]

    // Every agent is the SAME chassis — dumb agents, harness-owned settings.
    const svc: ComposeService = JSON.parse(JSON.stringify(chassis.service)) as ComposeService
    delete svc.build
    delete svc.depends_on
    delete svc.profiles
    // Publish the persona gateway on a stable loopback port so the app reaches
    // this agent directly (no bridge/multiplexer). Loopback-only; the HERMES key
    // still gates it.
    svc.ports = [`127.0.0.1:${ports.get(def.slug)}:8642`]

    const env = { ...((svc.environment ?? {}) as Record<string, unknown>), ...(extras?.environment ?? {}) }
    env.API_SERVER_KEY = `\${HERMES_KEY_${def.slug.toUpperCase()}}`
    env.API_SERVER_MODEL_NAME = def.model
    // The toolkit MCP header interpolates this from the container env.
    env.TALARIA_AGENT_KEY = '${TALARIA_AGENT_KEY}'
    // Work pickup: agents poll for assigned tickets on this cadence. The
    // chassis default was 0 (OFF) — which silently disabled ticket pickup
    // fleet-wide. 45s default, host env still overrides.
    env.TALARIA_HEARTBEAT_SECONDS = '${TALARIA_HEARTBEAT_SECONDS:-45}'

    // Workbench overlay — the agent's runtime profile (sandbox tools for its
    // role), resolved from THE setting (off/auto/on + fit rules). A profile
    // may override the image, add env, and mount extra volumes; everything
    // else about the chassis stays identical.
    const wb = await resolveWorkbench({
      department: def.department,
      role: (def as unknown as { role?: string | null }).role ?? null,
      workbench: ((def as unknown as { workbench?: string }).workbench ?? 'auto') as 'off' | 'auto' | 'on',
      workbenchProfile: (def as unknown as { workbenchProfile?: string | null }).workbenchProfile ?? null,
    }).catch(() => null as WorkbenchProfile | null)
    if (wb) {
      if (wb.image) svc.image = wb.image
      for (const [k, v] of Object.entries(wb.env)) env[k] = v
      env.TALARIA_WORKBENCH_PROFILE = wb.slug
      // Harness auth, gateway-first: OpenAI-compatible harnesses point at
      // Talaria's gateway (same creds the persona already uses — metered,
      // attributed); native harnesses get their provider's key interpolated
      // from the endpoint registry's env contract, scoped to this container.
      // GitHub attribution: commits made in this sandbox are AUTHORED as the
      // agent — its display name and a stable per-agent email — so history
      // and blame show who did the work, not a generic bot. (API actions —
      // branch/PR/merge — still show the App's identity; per-agent bots would
      // mean one App per agent, so PRs carry the agent label in their body.)
      // Harness state PERSISTS: each harness's sessions/history live on the
      // department's state volume, not ephemeral /root — surviving container
      // recreates and SHARED across the department's agents (hand-offs:
      // a session one agent starts, a department-mate can resume).
      env.CLAUDE_CONFIG_DIR = '/opt/data/workbench/harness/claude'
      env.CODEX_HOME = '/opt/data/workbench/harness/codex'
      env.XDG_DATA_HOME = '/opt/data/workbench/harness/xdg'
      // Playwright browsers persist too — UI testing is first-class dev work.
      env.PLAYWRIGHT_BROWSERS_PATH = '/opt/data/workbench/harness/playwright'
      const agentLabel = `${def.displayName} (Talaria agent)`
      const agentEmail = `${def.model}@agents.talaria.local`
      env.GIT_AUTHOR_NAME = agentLabel
      env.GIT_AUTHOR_EMAIL = agentEmail
      env.GIT_COMMITTER_NAME = agentLabel
      env.GIT_COMMITTER_EMAIL = agentEmail
      const { listHarnessDefs: harnessRegistry } = await import('./workbench-harnesses')
      const registry = await harnessRegistry()
      const sqlc = await db()
      const endpoints = (await sqlc`select provider, api_key_env as "apiKeyEnv" from llm_endpoints where api_key_env is not null`) as unknown as Array<{ provider: string; apiKeyEnv: string }>
      for (const slug of wb.harnesses) {
        const h = registry.find((x) => x.slug === slug)
        if (!h) continue
        for (const [k, v] of Object.entries(h.fullEnv)) env[k] = v
        if (h.auth !== 'gateway') {
          const ep = endpoints.find((e) => e.provider === (h.auth as { provider: string }).provider)
          if (ep) env[(h.auth as { envVar: string }).envVar] = '${' + ep.apiKeyEnv + '}'
        }
      }
    }
    svc.environment = env

    // Per-agent state volume: imported agents keep their pre-Talaria volume
    // (external, legacy-named) so their memory survives; created agents get a
    // project-local one that compose creates on first up.
    const stateVolume = `hermes-${def.department}`
    volumes[stateVolume] = imported ? { external: true, name: `${LEGACY_DOCKER_PROJECT}_${stateVolume}` } : {}

    // MCP pass-through: the agent's EXISTING grants (talaria + registry
    // servers incl. the workbench surface), rendered into each harness's
    // native config format — pointed at the same per-agent gateway, keyed by
    // the same env-interpolated fleet key. Zero in-sandbox reconnection;
    // grant changes re-render, revocations bite at the gateway instantly.
    if (wb) {
      const names = ['talaria', ...(await serversForAgent(def.model)).map((x) => x.name)]
      const uniq = [...new Set(names)]
      const gwUrl = (n: string) => `${MCP_GW_BASE()}/${n}`
      const wbDir = join(agentDir, 'workbench')
      await mkdir(wbDir, { recursive: true })
      // Every profile harness that consumes MCP gets the agent's grants in
      // ITS OWN format — the registry (builtin + app-shipped + custom) says
      // which file and which shape.
      const { listHarnessDefs: passthroughRegistry } = await import('./workbench-harnesses')
      const passDefs = await passthroughRegistry()
      const renderers: Record<string, () => unknown> = {
        // ${VAR} expands from the container env.
        'claude-json': () => ({
          mcpServers: Object.fromEntries(
            uniq.map((n) => [n, { type: 'http', url: gwUrl(n), headers: { 'X-Agent-Name': def.model, 'X-Api-Key': '${TALARIA_AGENT_KEY}' } }]),
          ),
        }),
        // {env:VAR} is opencode's env-substitution syntax.
        'opencode-json': () => ({
          $schema: 'https://opencode.ai/config.json',
          mcp: Object.fromEntries(
            uniq.map((n) => [n, { type: 'remote', url: gwUrl(n), headers: { 'X-Agent-Name': def.model, 'X-Api-Key': '{env:TALARIA_AGENT_KEY}' }, enabled: true }]),
          ),
        }),
      }
      const written = new Set<string>()
      for (const slug of wb.harnesses) {
        const h = passDefs.find((x) => x.slug === slug)
        if (!h?.mcpConfig || written.has(h.mcpConfig.filename)) continue
        const render = renderers[h.mcpConfig.format]
        if (!render) continue
        await writeFile(join(wbDir, h.mcpConfig.filename), JSON.stringify(render(), null, 2))
        written.add(h.mcpConfig.filename)
      }
    }
    const wbMounts = wb?.mounts ?? []
    svc.volumes = [
      ...(wb ? [`${join(agentDir, 'workbench')}:/opt/workbench-config:ro`] : []),
      ...wbMounts,
      `${stateVolume}:/opt/data`,
      `${join(agentDir, 'config.yaml')}:/opt/data/config.yaml:ro`,
      `${join(agentDir, 'SOUL.md')}:/opt/data/SOUL.md:ro`,
      `${join(agentDir, 'skills')}:/opt/dept-skills:ro`,
      // Fleet-wide skills (e.g. the talaria-toolkit onboarding skill) — every
      // agent gets them; Hermes reads skills per invocation, so edits are live.
      `${join(FLEET_DIR(), 'skills')}:/opt/skills:ro`,
      // Chassis + extras mounts pass through: shared skill/hook/plugin roots
      // (absolute, fleet-owned) and named volumes (defined in chassis.volumes).
      ...((svc.volumes ?? []) as string[]).filter((v) => {
        const dest = String(v).split(':')[1]
        return !['/opt/data', '/opt/data/config.yaml', '/opt/data/SOUL.md', '/opt/dept-skills', '/opt/skills'].includes(dest ?? '')
      }),
      ...(extras?.volumes ?? []),
    ]

    // Per-agent secrets (UI-configured, DB-encrypted) materialize into the
    // agent dir and load via env_file — nothing hand-edited in fleet/.env.
    if (await materializeAgentSecrets(def.id, def.slug)) {
      svc.env_file = [join(agentDir, 'secrets.env')]
    } else {
      delete svc.env_file
    }

    svc.networks = ['fleet']
    if (extras?.secrets) {
      // Entries are either "name" or long-form { source,  }. A reference
      // without a definition in chassis.secrets is DROPPED (with a warning) —
      // keeping it would make compose reject the whole file.
      svc.secrets = (extras.secrets as Array<string | { source?: string }>).filter((s) => {
        const name = typeof s === 'string' ? s : s.source
        const secretDef = name ? chassis.secrets?.[name] : undefined
        if (name && secretDef) {
          secrets[name] = secretDef
          return true
        }
        result.warnings.push(`${serviceName}: secret ${name ?? JSON.stringify(s)} not defined in chassis.yml — dropped`)
        return false
      })
    }
    services[serviceName] = svc

    // Mid-roll: the INCOMING slot renders alongside, identical except for its
    // fresh published port. Same mounts and state volume — the old container
    // retires right after cutover + drain, so the overlap stays brief.
    if (opts.roll && opts.roll.slug === def.slug) {
      const incoming = JSON.parse(JSON.stringify(svc)) as ComposeService
      incoming.ports = [`127.0.0.1:${opts.roll.port}:8642`]
      services[`agent-${def.department}${opts.roll.slot === 'b' ? '-b' : ''}`] = incoming
    }
    result.agents.push(def.model)
  }

  for (const m of remapped) result.warnings.push(`gateway: ${m} (register the provider on /models to restore this tier)`)

  const compose = {
    name: 'talaria-fleet',
    services,
    volumes,
    ...(Object.keys(secrets).length ? { secrets } : {}),
    // One unified Talaria network for every Talaria container.
    networks: { fleet: { external: true, name: chassis.network?.name ?? 'talaria' } },
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

/** The fleet manifest Talaria reads to reach agents directly: every enabled
 *  agent's persona gateway URL (host + its published port) + HERMES key, plus
 *  one entry per model tier (`<base>-<alias>`). No bridge — the app calls each
 *  URL itself. Written to fleet/fleet.json. */
async function writeFleetManifest(result: RenderResult): Promise<void> {
  const sql = await db()
  const defs = (await sql`
    select d.slug, d.department, d.model, d.gateway_port as "gatewayPort", v.config
    from agent_defs d
    left join agent_versions v on v.agent_id = d.id and v.version = d.current_version
    where d.enabled order by d.slug
  `) as unknown as Array<{ slug: string; department: string; model: string; gatewayPort: number | null; config: AgentConfig | null }>

  const env = await readFile(FLEET_ENV(), 'utf8').catch(() => '')
  const keys = new Map<string, string>()
  for (const line of env.split('\n')) {
    const m = /^HERMES_KEY_([A-Z0-9_]+)=(.*)$/.exec(line.trim())
    if (m) keys.set(m[1]!.toLowerCase(), m[2]!.trim())
  }

  const manifest = defs.flatMap((d) => {
    const key = keys.get(d.slug) ?? ''
    if (!key) result.warnings.push(`${d.slug}: no HERMES_KEY_${d.slug.toUpperCase()} in the fleet .env`)
    if (!d.gatewayPort) result.warnings.push(`${d.slug}: no gateway port assigned yet — render again`)
    const url = `http://${AGENT_HOST()}:${d.gatewayPort ?? 0}`
    return [
      { model: d.model, url, key },
      ...(d.config?.aliases ?? []).map((a) => ({ model: `${d.model}-${a.name}`, url, key })),
    ]
  })
  const path = join(FLEET_DIR(), 'fleet.json')
  await mkdir(dirname(path), { recursive: true })
  await writeFile(path, JSON.stringify(manifest))
  result.files.push(path)
}
