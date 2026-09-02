// The renderer: materialize managed agent versions into a runnable fleet/ dir.
//
//   fleet/agents/<slug>/config.yaml   the agent's Hermes config (from the version)
//   fleet/agents/<slug>/SOUL.md       the agent's soul
//   fleet/docker-compose.yml          generated — one service per managed agent
//   fleet/fleet.json                  the manifest the app reads to reach agents
//
// Generated services are derived from the chassis service block (chassis.yml —
// the shared env vars and mounts), with these changes:
//   • config.yaml/SOUL.md bind mounts point at the rendered files
//   • imported agents' state volumes become external references (ai_<name>) —
//     state survives; created agents get project-local volumes
//   • networks → the external fleet network; depends_on/build/profiles dropped
//     (the app reaches agents via stable loopback ports, or compose DNS names)
import { createHash } from 'node:crypto'
import { chmod, cp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { parse as parseYaml, stringify as stringifyYaml } from 'yaml'
import { db } from './db/pg'
import { ensureAgentApiKey, legacyMigrationStatus, legacyMigrationWarning } from './agent-auth'
import { resolveWorkbench, type WorkbenchProfile } from './workbench'
import { grantedHandlesFor } from './workspace-secrets'
import { materializeAgentSecrets } from './agent-secrets'
import { ensureGatewayBrain, gatewayModelSet, routeConfigThroughGateway } from './fleet-brain'
import { ensureMcpService } from './mcp-service'
import { serversForAgent } from './mcp-registry'

/** The MCP gateway base as fleet containers reach it — the UI server over the
 *  docker host bridge (same pattern as the talaria-mcp fleet URL). */
export const MCP_GW_BASE = () =>
  process.env.TALARIA_MCP_GW_URL ?? `http://host.docker.internal:${process.env.PORT ?? 5273}/api/mcp/gw`
import { orgProfile, orgSoulHeader, toolkitSoulHeader, voiceSoulHeader } from './org'
import { getGuardConfig, guardCoachingFor } from './guardrails'
import type { AgentConfig, AgentDef, AgentVersion } from './agent-defs'

export const FLEET_DIR = () => process.env.TALARIA_FLEET_DIR ?? resolve(process.cwd(), '../fleet')
/** The fleet's compose project. One fleet per project is the identity the
 *  whole lifecycle assumes (container names, volume names, reconcile scope),
 *  so anything sharing a docker host with agents — a second instance, a
 *  devbox — must drive its own project or the two reconcile each other's
 *  containers. Everything that names the project resolves through here (same
 *  pattern as TALARIA_FLEET_NETWORK for the network). */
export const fleetProject = () => process.env.TALARIA_FLEET_PROJECT ?? 'talaria-fleet'
/** The fleet's env file (agent keys + compose interpolation) — Talaria-owned. */
export const FLEET_ENV = () => join(FLEET_DIR(), '.env')
/** The chassis every agent renders from: one service block + per-slug extras.
 *  Talaria-owned. */
const CHASSIS_FILE = () => process.env.TALARIA_CHASSIS_FILE ?? join(FLEET_DIR(), 'chassis.yml')

/** fleet/.env must carry TALARIA_AGENT_KEY (the app's own hop to the toolkit
 *  service) plus one TALARIA_AGENT_KEY_<SLUG> per agent, so compose can
 *  interpolate each agent's OWN credential into its env. Shared keys append
 *  once; per-agent keys are rewritten from the DB every render (see
 *  ensureAgentEnvKeys). The file is 0600 in a 0700 dir — it is plaintext
 *  credentials for the entire fleet. */
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

/** fleet/.env holds every agent's PLAINTEXT credential, so it is written and
 *  kept at 0600 inside a 0700 dir — same rule as agent-secrets' secrets.env.
 *  Anything else means any local account (or any workbench agent with a shell)
 *  can impersonate the whole fleet, and re-rendering wouldn't fix it. */
async function writeFleetEnv(envPath: string, content: string): Promise<void> {
  await writeFile(envPath, content, { mode: 0o600 })
  await chmod(envPath, 0o600).catch(() => {}) // existing file keeps its old mode otherwise
  await chmod(dirname(envPath), 0o700).catch(() => {})
}

async function ensureFleetEnvKey(): Promise<void> {
  const envPath = FLEET_ENV()
  let current = await readFile(envPath, 'utf8').catch(() => '')
  // Even with nothing to append, an existing world-readable file must be locked
  // down — installs that rendered before this change are the ones at risk.
  await chmod(envPath, 0o600).catch(() => {})
  await chmod(dirname(envPath), 0o700).catch(() => {})
  const append: string[] = []
  const need = (name: string, value: string | undefined) => {
    if (value && !new RegExp(`^${name}=`, 'm').test(current)) append.push(`${name}=${value}`)
  }
  need('TALARIA_AGENT_KEY', process.env.TALARIA_AGENT_KEY)
  // Native-auth harness keys: any provider key a registry harness references
  // must reach compose interpolation — provisioned from the server env when
  // present; absent keys stay absent (doctor/auth surfaces tell the agent).
  try {
    const [{ listHarnessDefs }, sqlc] = await Promise.all([import('./workbench-harnesses'), db()])
    const endpoints = (await sqlc`select provider, api_key_env as "apiKeyEnv" from llm_endpoints where api_key_env is not null`) as unknown as Array<{ provider: string; apiKeyEnv: string }>
    for (const h of await listHarnessDefs()) {
      if (h.auth === 'gateway') continue
      const ep = endpoints.find((e) => e.provider === (h.auth as { provider: string }).provider)
      if (ep) need(ep.apiKeyEnv, process.env[ep.apiKeyEnv])
    }
  } catch {
    /* registry unavailable — agent key provisioning above still holds */
  }
  if (!append.length) return
  await writeFleetEnv(envPath, `${current.replace(/\n?$/, '\n')}${append.join('\n')}\n`)
}

/** Compose-interpolation name for an agent's own credential. */
export const AGENT_KEY_VAR = (slug: string) => `TALARIA_AGENT_KEY_${slug.toUpperCase()}`

/** Materialize every managed agent's credential into the fleet .env, minting
 *  on first render (agent-auth owns the secret; the DB is the source of
 *  truth, so a line lost here comes back identical rather than rotating a
 *  running container out of its own identity). Same shape as HERMES_KEY_<SLUG>,
 *  the other per-agent secret compose interpolates.
 *
 *  The DB wins over whatever the file says: a line is REWRITTEN, never skipped
 *  because it exists. Skipping on presence silently bricks an agent whose slug
 *  was reused after a delete (stale line, no agent_keys row → the container
 *  presents a dead secret and gets an undiagnosable 401), and the same happens
 *  after a DB restore against a preserved .env. */
async function ensureAgentEnvKeys(targets: RenderTarget[]): Promise<void> {
  const envPath = FLEET_ENV()
  const current = await readFile(envPath, 'utf8').catch(() => '')
  let next = current
  const append: string[] = []
  for (const { def } of targets) {
    const name = AGENT_KEY_VAR(def.slug)
    const line = `${name}=${await ensureAgentApiKey(def.id)}`
    const existing = new RegExp(`^${name}=.*$`, 'm')
    if (existing.test(next)) next = next.replace(existing, () => line) // fn form: a secret is never a $-pattern
    else append.push(line)
  }
  if (append.length) next = `${next.replace(/\n?$/, '\n')}# per-agent credentials — Talaria-owned\n${append.join('\n')}\n`
  if (next === current) {
    await chmod(envPath, 0o600).catch(() => {})
    await chmod(dirname(envPath), 0o700).catch(() => {})
    return
  }
  await writeFleetEnv(envPath, next)
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
  // and that the compose env can interpolate each agent's key into the header.
  ensureMcpService()
  await ensureFleetEnvKey()
  await ensureAgentEnvKeys(targets)
  await seedSharedSkills()

  // Credential-migration visibility. A render is the operator's moment: say who
  // is still authenticating with the org-wide key, because that — not a guess —
  // is what decides when TALARIA_AGENT_KEY_LEGACY=off stops being an outage.
  const legacy = await legacyMigrationStatus().catch(() => null)
  const legacyWarning = legacy ? legacyMigrationWarning(legacy) : null
  if (legacy && legacyWarning) {
    result.warnings.push(legacyWarning)
    if (legacy.windowOpen) console.warn(`[fleet] ${legacyWarning}`)
    else console.error(`[fleet] ${legacyWarning}`)
  }

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
    // No custom gate: harnesses run via npx from the stock image (first use
    // installs into the persistent npm cache); a real workbench image just
    // makes first-run instant.
    {
      const wbc = await resolveWorkbench({
        department: def.department,
        role: def.role ?? null,
        workbench: def.workbench ?? 'auto',
        workbenchProfile: def.workbenchProfile ?? null,
      }).catch(() => null)
      if (wbc) {
        const { listHarnessDefs } = await import('./workbench-harnesses')
        const pick = def.workbenchHarness
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
    // WHAT THIS AGENT MAY SPEND WITHOUT SEEING. Handles and kinds only — the
    // store has no shape that carries a value, so there is nothing here to leak
    // even if a soul were read by the wrong person.
    //
    // IT BELONGS IN THE SOUL rather than in a per-turn prompt because a
    // credential grant is a standing fact about the agent, like its toolkit: an
    // agent that learns mid-conversation that it could have pushed is an agent
    // that already told somebody it could not. A grant changes on the next
    // render, which is the same cadence every other standing fact here follows.
    //
    // An agent granted nothing gets an empty string and is told nothing, which
    // is also correct — a list of names it cannot use is a map of the
    // workspace's credentials.
    const secretHandles = await grantedHandlesFor(def.model).catch(() => '')
    await writeFile(join(agentDir, 'SOUL.md'), `${[soulHeader, coaching, secretHandles].filter(Boolean).join('\n\n')}\n\n${version.soul}`)
    result.files.push(join(agentDir, 'config.yaml'), join(agentDir, 'SOUL.md'))

    // ── THE GIT CREDENTIAL HELPER ─────────────────────────────────────────────
    //
    // WHERE A HANDLE CANNOT REACH. Handles substitute at the MCP gateway, which
    // is every tool call an agent makes through Talaria — but NOT the shell
    // inside a workbench sandbox, where a coding harness runs `git push` with
    // its own bash tool and we are not in the path. Pushing code is the main
    // thing a workbench credential is for, so that gap made handles unusable
    // exactly where they were needed most.
    //
    // Git's credential protocol closes it. Git hands a helper the protocol, host
    // and path on stdin and reads `username=`/`password=` back; this one asks
    // Talaria over the agent's OWN key. Git keeps the answer in process memory,
    // so the value never enters the model's context, never lands in command
    // output, and is never written to disk — the model runs `git push` and it
    // simply works, without ever holding a credential.
    //
    // WRITTEN PER AGENT because the credential it presents is per agent: the
    // answer is scoped to what THIS agent was granted, and a shared helper would
    // have to be told which agent it was speaking for by something the agent
    // controls.
    // PORTABLE ACROSS HARNESS IMAGES, and that is not a nicety. The first
    // version used `curl` unconditionally; `alpine/git` has no curl, so the
    // helper exited 0 having printed nothing — which git reads as "no
    // credential exists" and reports as `could not read Username`. A helper
    // that cannot RUN must not be indistinguishable from a credential that is
    // not there, so it falls back to wget and says so on stderr if neither is
    // present. Silence is the one answer this script is not allowed to give.
    const helper = [
      '#!/bin/sh',
      '# Written by Talaria (fleet-render). Answers `get`; `store` and `erase`',
      '# are no-ops because we ARE the store, and forgetting is done by revoking',
      '# the grant rather than by anything git says.',
      '[ "$1" = "get" ] || exit 0',
      'host=""; proto=""; path=""',
      'while IFS="=" read -r k v; do',
      '  [ "$k" = "host" ] && host="$v"',
      '  [ "$k" = "protocol" ] && proto="$v"',
      '  # PATH IS WHAT SCOPES A GITHUB ANSWER to one repo. Git only sends it',
      '  # when credential.useHttpPath is set, which the rendered gitconfig does.',
      '  [ "$k" = "path" ] && path="$v"',
      'done',
      '[ -n "$host" ] || exit 0',
      'url="$TALARIA_API_URL/api/secrets/git-credential"',
      'body="{\\"host\\":\\"$host\\",\\"protocol\\":\\"$proto\\",\\"path\\":\\"$path\\"}"',
      'if command -v curl >/dev/null 2>&1; then',
      '  resp=$(curl -sS --fail -X POST "$url" \\',
      '    -H "X-Agent-Name: $API_SERVER_MODEL_NAME" -H "X-Api-Key: $API_SERVER_KEY" \\',
      '    -H "content-type: application/json" -d "$body" 2>/dev/null) || exit 0',
      'elif command -v wget >/dev/null 2>&1; then',
      '  # TWO WGETS EXIST and they disagree. BusyBox (every alpine-derived',
      '  # harness image) takes --post-data; GNU wget takes --body-data with',
      '  # --method=POST and rejects the other. Trying BusyBox first and falling',
      '  # through costs one failed call on GNU and works on both, which beats',
      '  # guessing the image.',
      '  resp=$(wget -qO- --post-data="$body" \\',
      '    --header="X-Agent-Name: $API_SERVER_MODEL_NAME" --header="X-Api-Key: $API_SERVER_KEY" \\',
      '    --header="content-type: application/json" "$url" 2>/dev/null)',
      '  [ -n "$resp" ] || resp=$(wget -qO- --method=POST --body-data="$body" \\',
      '    --header="X-Agent-Name: $API_SERVER_MODEL_NAME" --header="X-Api-Key: $API_SERVER_KEY" \\',
      '    --header="content-type: application/json" "$url" 2>/dev/null) || exit 0',
      'else',
      '  echo "talaria: no curl or wget in this image — cannot fetch a credential for $host" >&2',
      '  exit 0',
      'fi',
      '# Parsed with sed rather than jq for the same reason: jq is not guaranteed',
      '# in every harness image, and a helper that fails because a tool is missing',
      '# looks exactly like a credential that does not exist.',
      'u=$(printf %s "$resp" | sed -n \'s/.*"username":"\\([^"]*\\)".*/\\1/p\')',
      'p=$(printf %s "$resp" | sed -n \'s/.*"password":"\\([^"]*\\)".*/\\1/p\')',
      '[ -n "$p" ] || exit 0',
      'echo "username=$u"',
      'echo "password=$p"',
      '',
    ].join('\n')
    const helperPath = join(agentDir, 'git-credential-talaria')
    await writeFile(helperPath, helper, { mode: 0o755 })
    // SYSTEM-WIDE gitconfig rather than the agent's ~/.gitconfig: a workbench
    // job runs the harness as whatever user that image uses, and a helper only
    // the root home knows about is a helper that silently does not run. It also
    // has to survive the harness writing its own ~/.gitconfig, which several do.
    const gitconfigPath = join(agentDir, 'gitconfig')
    // `useHttpPath` is what makes git send the repository along with the host.
    // Without it a GitHub answer could only be scoped to github.com — i.e. to
    // every repo the installation can reach — and repo-scoping is the whole
    // reason this is safer than the clone URL it replaces.
    await writeFile(gitconfigPath, ['[credential]', '\thelper = talaria', '\tuseHttpPath = true', ''].join('\n'))
    result.files.push(helperPath, gitconfigPath)

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
    // this agent directly. Loopback-only; the HERMES key
    // still gates it. Container-dial deployments skip publishing entirely — the
    // manifest below dials the compose service name, so the published port is
    // dead weight there, and every install sharing one docker host (second
    // instance, devbox) allocates identical ports from the same seed data and
    // would collide on the host.
    if (process.env.TALARIA_AGENT_DIAL !== 'container') {
      svc.ports = [`127.0.0.1:${ports.get(def.slug)}:8642`]
    }

    const env = { ...((svc.environment ?? {}) as Record<string, unknown>), ...(extras?.environment ?? {}) }
    env.API_SERVER_KEY = `\${HERMES_KEY_${def.slug.toUpperCase()}}`
    env.API_SERVER_MODEL_NAME = def.model
    // The agent's OWN credential, under the same env name every rendered
    // header/config already interpolates — so identity travels with the key
    // and the shared org key never enters an agent container again.
    env.TALARIA_AGENT_KEY = `\${${AGENT_KEY_VAR(def.slug)}}`
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
      role: def.role ?? null,
      workbench: def.workbench ?? 'auto',
      workbenchProfile: def.workbenchProfile ?? null,
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
      // npx-run harnesses install into a PERSISTENT cache — first use pays
      // the download once, every later run (and container recreate) is warm.
      env.npm_config_cache = '/opt/data/workbench/harness/npm'
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
        // 'custom' hands rendering to the harness's own code (app-shipped
        // only — admin JSON can't carry functions); it owns its env syntax.
        const body =
          h.mcpConfig.format === 'custom'
            ? h.renderMcpConfig?.({
                agentModel: def.model,
                servers: uniq.map((n) => ({ name: n, url: gwUrl(n) })),
                apiKeyEnvVar: 'TALARIA_AGENT_KEY',
              })
            : renderers[h.mcpConfig.format]?.()
        if (body === undefined) continue
        await writeFile(join(wbDir, h.mcpConfig.filename), JSON.stringify(body, null, 2))
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
      // THE CREDENTIAL HELPER, on PATH and configured below. Mounted rather
      // than baked into the image so a grant takes effect on the next render
      // instead of the next image build.
      `${join(agentDir, 'git-credential-talaria')}:/usr/local/bin/git-credential-talaria:ro`,
      `${join(agentDir, 'gitconfig')}:/etc/gitconfig:ro`,
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
      if (process.env.TALARIA_AGENT_DIAL !== 'container') {
        incoming.ports = [`127.0.0.1:${opts.roll.port}:8642`]
      }
      services[`agent-${def.department}${opts.roll.slot === 'b' ? '-b' : ''}`] = incoming
    }
    result.agents.push(def.model)
  }

  for (const m of remapped) result.warnings.push(`gateway: ${m} (register the provider on /models to restore this tier)`)

  const compose = {
    name: fleetProject(),
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
    select d.slug, d.department, d.model, d.gateway_port as "gatewayPort", d.active_slot as "activeSlot", v.config
    from agent_defs d
    left join agent_versions v on v.agent_id = d.id and v.version = d.current_version
    where d.enabled order by d.slug
  `) as unknown as Array<{ slug: string; department: string; model: string; gatewayPort: number | null; activeSlot: string | null; config: AgentConfig | null }>

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
    // Container mode (TALARIA_AGENT_DIAL=container, set by docker/compose.yml):
    // the app runs ON the fleet network, so it dials each agent's compose
    // service name directly — the loopback-published ports above sit on the
    // HOST's 127.0.0.1, which a container cannot reach. Slot-aware, same
    // service-name expression the renderer itself uses; during a roll the DB's
    // active_slot stays the source of truth and cutover re-renders.
    const url =
      process.env.TALARIA_AGENT_DIAL === 'container'
        ? `http://agent-${d.department}${d.activeSlot === 'b' ? '-b' : ''}:8642`
        : `http://${AGENT_HOST()}:${d.gatewayPort ?? 0}`
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
