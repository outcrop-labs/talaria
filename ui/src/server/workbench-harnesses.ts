// The harness REGISTRY — the extension point developers build against. A
// harness is a declarative HarnessDefinition (see @talaria/sdk defineHarness):
// how it authenticates, how it's invoked (structured output first), how it
// serves/consumes MCP, and what a driving agent should understand about it.
//
// Three layers merge by slug, later winning:
//   builtin      the harnesses Talaria ships (below)
//   app-shipped  apps/<slug>/harness.ts (defineHarness) — enabled apps only
//   custom       workbench_harness_defs rows (admin-registered JSON)
//
// Effort→model stays the platform's call: agents pick effort, Talaria
// resolves the model — per-agent overrides first, then the code-* roles.
import { db } from './db/pg'
import { resolveRoleModel } from './model-roles'
import { resolveRoute } from './llm-gateway'
import type { HarnessDefinition } from '@/sdk/server'

export type { HarnessDefinition }

export interface ResolvedHarness extends HarnessDefinition {
  source: 'builtin' | `app:${string}` | 'custom'
  /** Full container env: auth-derived + definition env. */
  fullEnv: Record<string, string>
}

const GATEWAY_ENV = { OPENAI_BASE_URL: '${LLM_BASE_URL}', OPENAI_API_KEY: '${LLM_API_KEY}' }

const BUILTINS: HarnessDefinition[] = [
  {
    slug: 'opencode',
    label: 'opencode',
    auth: 'gateway',
    env: { OPENCODE_CONFIG: '/opt/workbench-config/opencode.json' },
    modelPrefix: 'openai/',
    invoke: 'npx -y opencode-ai run --model <model> "<task>"',
    jsonInvoke: 'npx -y opencode-ai run --model <model> --format json "<task>"',
    probe: 'npx -y opencode-ai --version',
    mcpConfig: { format: 'opencode-json', filename: 'opencode.json' },
    guide:
      'opencode is session-based: each run continues a project session. Use --format json and read the structured result (message, file edits) instead of scraping text. It reads OPENCODE_CONFIG for MCP servers — your Talaria tools are already wired in there.',
  },
  {
    slug: 'claude-code',
    label: 'Claude Code',
    auth: { provider: 'anthropic', envVar: 'ANTHROPIC_API_KEY' },
    invoke: 'npx -y @anthropic-ai/claude-code -p "<task>" --model <model> --mcp-config /opt/workbench-config/mcp.json',
    jsonInvoke: 'npx -y @anthropic-ai/claude-code -p "<task>" --model <model> --output-format json --mcp-config /opt/workbench-config/mcp.json',
    mcpServe: { command: 'npx', args: ['-y', '@anthropic-ai/claude-code', 'mcp', 'serve'] },
    probe: 'npx -y @anthropic-ai/claude-code --version',
    mcpConfig: { format: 'claude-json', filename: 'mcp.json' },
    guide:
      "Claude Code returns a structured result with --output-format json (result text, cost, session_id) — resume a session with --resume <session_id> to ask follow-ups instead of restarting. Prefer its MCP tools when registered on your config; otherwise use the JSON form and read the result object, never raw logs.",
  },
  {
    slug: 'oh-my-pi',
    label: 'Oh My Pi',
    auth: 'gateway',
    invoke: 'npx -y @oh-my-pi/pi-coding-agent "<task>" --model <model>',
    probe: 'npx -y @oh-my-pi/pi-coding-agent --version',
    guide: 'Oh My Pi (omp) is a full terminal coding agent — hash-anchored edits, LSP, subagents, a browser. It inherits MCP/rules config from .claude/.codex-style dirs in the workspace, so your pass-through config reaches it via the repo. No first-party MCP server mode yet: drive it one-shot per task and verify its work yourself with git diff and tests.',
  },
  {
    slug: 'codex',
    label: 'Codex CLI',
    auth: 'gateway',
    invoke: 'npx -y @openai/codex exec --model <model> "<task>"',
    jsonInvoke: 'npx -y @openai/codex exec --model <model> --json "<task>"',
    mcpServe: { command: 'npx', args: ['-y', '@openai/codex', 'mcp'] },
    probe: 'npx -y @openai/codex --version',
    guide: 'Codex exec emits JSON events with --json — read the final result event. As an MCP server (codex mcp) it exposes tool-driven sessions; prefer that when registered on your config.',
  },
]

// App-shipped harnesses — same discovery contract as app MCP surfaces.
const HARNESS_MODS = import.meta.glob('../../../apps/*/harness.ts') as Record<string, () => Promise<unknown>>
const appSlugOf = (path: string): string => /apps\/([^/]+)\//.exec(path)?.[1] ?? path

async function appHarnesses(): Promise<Array<{ def: HarnessDefinition; app: string }>> {
  const { enabledApps } = await import('./apps')
  const enabled = new Set((await enabledApps()).map((a) => a.slug))
  const out: Array<{ def: HarnessDefinition; app: string }> = []
  for (const [path, load] of Object.entries(HARNESS_MODS)) {
    const app = appSlugOf(path)
    if (!enabled.has(app)) continue
    const mod = (await load().catch(() => null)) as { default?: HarnessDefinition } | null
    if (mod?.default?.slug && mod.default.invoke && mod.default.guide) out.push({ def: mod.default, app })
  }
  return out
}

function resolveDef(def: HarnessDefinition, source: ResolvedHarness['source']): ResolvedHarness {
  const authEnv = def.auth === 'gateway' ? GATEWAY_ENV : {}
  return { ...def, source, fullEnv: { ...authEnv, ...(def.env ?? {}) } }
}

/** The merged registry — builtin < app-shipped < admin-custom, by slug. */
export async function listHarnessDefs(): Promise<ResolvedHarness[]> {
  const bySlug = new Map<string, ResolvedHarness>()
  for (const b of BUILTINS) bySlug.set(b.slug, resolveDef(b, 'builtin'))
  for (const { def, app } of await appHarnesses().catch(() => [] as Array<{ def: HarnessDefinition; app: string }>)) {
    bySlug.set(def.slug, resolveDef(def, `app:${app}`))
  }
  const sql = await db()
  const rows = (await sql`select slug, definition from workbench_harness_defs where enabled`) as unknown as Array<{
    slug: string
    definition: HarnessDefinition
  }>
  for (const r of rows) {
    const def = { ...r.definition, slug: r.slug }
    if (def.invoke && def.guide) bySlug.set(r.slug, resolveDef(def, 'custom'))
  }
  return [...bySlug.values()]
}

/** Admin-custom definitions (declarative only — no code runs from these). */
export async function upsertCustomHarness(slug: string, definition: HarnessDefinition, createdBy: string): Promise<void> {
  const sql = await db()
  await sql`
    insert into workbench_harness_defs (slug, definition, created_by)
    values (${slug}, ${sql.json(definition as unknown as Record<string, string>)}, ${createdBy})
    on conflict (slug) do update set definition = excluded.definition, enabled = true, updated_at = now()
  `
}

export async function deleteCustomHarness(slug: string): Promise<void> {
  const sql = await db()
  await sql`delete from workbench_harness_defs where slug = ${slug}`
}

/** The model id as THIS harness's CLI expects it. */
export const harnessModelArg = (h: HarnessDefinition, model: string): string => `${h.modelPrefix ?? ''}${model}`

// ── Effort → model (unchanged contract) ──────────────────────────────────────

export type Effort = 'light' | 'standard' | 'heavy'

/** Effort → model: per-agent override first, then the global roles with a
 *  fall-down chain so unset slots never strand work. */
export async function effortModel(effort: Effort, overrides?: Partial<Record<Effort, string>>): Promise<string | null> {
  const own = overrides?.[effort]
  if (own && (await resolveRoute(own))) return own
  const order: Effort[] = effort === 'heavy' ? ['heavy', 'standard', 'light'] : effort === 'standard' ? ['standard', 'light'] : ['light']
  for (const e of order) {
    const o = overrides?.[e]
    if (o && (await resolveRoute(o))) return o
    const m = await resolveRoleModel(`code-${e}`)
    if (m) return m
  }
  const utility = await resolveRoleModel('utility')
  if (utility) return utility
  for (const m of [process.env.TALARIA_COPILOT_MODEL ?? null, 'pl-main']) {
    if (m && (await resolveRoute(m))) return m
  }
  return null
}

/** All three resolved at once — start_job hands the agent the full map so it
 *  sees its options in effort terms, never raw catalog spelunking. */
export async function effortModels(overrides?: Partial<Record<Effort, string>>): Promise<Record<Effort, string | null>> {
  return {
    light: await effortModel('light', overrides),
    standard: await effortModel('standard', overrides),
    heavy: await effortModel('heavy', overrides),
  }
}
