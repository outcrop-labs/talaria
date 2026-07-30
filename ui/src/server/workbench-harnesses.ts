// Harness adapters — the registry of execution tools a workbench can carry.
// Each adapter declares how the harness authenticates (gateway-first: every
// OpenAI-compatible harness points at Talaria's gateway so its tokens are
// metered and attributed; 'native' harnesses need a platform-connected key of
// their provider, provisioned scoped into the sandbox) and how it consumes
// MCP config (phase 6 renders the agent's grants into that format).
//
// Effort→model is the split Jon set: AGENTS pick effort, TALARIA picks the
// model — via the code-* model roles, falling back down the chain so an unset
// slot never strands a job.
import { resolveRoleModel } from './model-roles'
import { resolveRoute } from './llm-gateway'

export interface HarnessAdapter {
  slug: string
  label: string
  /** How the harness reaches a model. 'gateway' = OpenAI-compatible env
   *  pointed at Talaria's own gateway (metered, attributed — preferred).
   *  'native' = needs its provider's real API key from the endpoint registry. */
  auth: 'gateway' | 'native'
  /** For native harnesses: the provider whose key to provision + the env var. */
  native?: { provider: string; envVar: string }
  /** Env template pointing the harness at the gateway (compose-interpolated). */
  gatewayEnv: Record<string, string>
  /** How this harness consumes MCP servers (phase 6 renders into this). */
  mcpConfig: 'claude-json' | 'opencode-json' | 'none'
  /** How the persona invokes it inside the sandbox, model slotted in. */
  invoke: string
  /** Prefix the model id needs for THIS harness (e.g. opencode's provider
   *  namespace) — keeps the invocation line directly runnable. */
  modelPrefix?: string
  /** Structured invocation — the API-integration form (JSON events/result),
   *  never raw stdout scraping. */
  jsonInvoke?: string
  /** How to run this harness AS an MCP server (stdio) — when present and the
   *  workbench image carries the binary, it registers on the agent's own
   *  Hermes config so the harness is driven with TOOLS. */
  mcpServe?: { command: string; args: string[] }
  /** What the driving agent should understand about this harness. */
  guide: string
}

export const HARNESSES: HarnessAdapter[] = [
  {
    slug: 'opencode',
    label: 'opencode',
    auth: 'gateway',
    gatewayEnv: { OPENAI_BASE_URL: '${LLM_BASE_URL}', OPENAI_API_KEY: '${LLM_API_KEY}', OPENCODE_CONFIG: '/opt/workbench-config/opencode.json' },
    mcpConfig: 'opencode-json',
    invoke: 'opencode run --model <model> "<task>"',
    jsonInvoke: 'opencode run --model <model> --format json "<task>"',
    modelPrefix: 'openai/',
    guide:
      'opencode is session-based: each run continues a project session. Use --format json and read the structured result (message, file edits) instead of scraping text. It reads OPENCODE_CONFIG for MCP servers — your Talaria tools are already wired in there.',
  },
  {
    slug: 'claude-code',
    label: 'Claude Code',
    auth: 'native',
    native: { provider: 'anthropic', envVar: 'ANTHROPIC_API_KEY' },
    gatewayEnv: {},
    mcpConfig: 'claude-json',
    invoke: 'claude -p "<task>" --model <model> --mcp-config /opt/workbench-config/mcp.json',
    jsonInvoke: 'claude -p "<task>" --model <model> --output-format json --mcp-config /opt/workbench-config/mcp.json',
    mcpServe: { command: 'claude', args: ['mcp', 'serve'] },
    guide:
      "Claude Code returns a structured result with --output-format json (result text, cost, session_id) — resume a session with --resume <session_id> to ask follow-ups instead of restarting. Prefer its MCP tools when registered on your config; otherwise use the JSON form and read the result object, never raw logs.",
  },
  {
    slug: 'oh-my-pi',
    label: 'Oh My Pi',
    auth: 'gateway',
    gatewayEnv: { OPENAI_BASE_URL: '${LLM_BASE_URL}', OPENAI_API_KEY: '${LLM_API_KEY}' },
    mcpConfig: 'none',
    invoke: 'omp run --model <model> "<task>"',
    guide: 'Ask it for structured output where supported; verify its work yourself with git diff and the test suite — you own the result, not the harness.',
  },
  {
    slug: 'codex',
    label: 'Codex CLI',
    auth: 'gateway',
    gatewayEnv: { OPENAI_BASE_URL: '${LLM_BASE_URL}', OPENAI_API_KEY: '${LLM_API_KEY}' },
    mcpConfig: 'none',
    invoke: 'codex exec --model <model> "<task>"',
    jsonInvoke: 'codex exec --model <model> --json "<task>"',
    mcpServe: { command: 'codex', args: ['mcp'] },
    guide: 'Codex exec emits JSON events with --json — read the final result event. As an MCP server (codex mcp) it exposes tool-driven sessions; prefer that when registered on your config.',
  },
]

export const harness = (slug: string): HarnessAdapter | undefined => HARNESSES.find((h) => h.slug === slug)

/** The model id as THIS harness's CLI expects it. */
export const harnessModelArg = (h: HarnessAdapter, model: string): string => `${h.modelPrefix ?? ''}${model}`

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
