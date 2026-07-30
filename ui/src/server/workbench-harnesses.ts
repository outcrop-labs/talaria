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
}

export const HARNESSES: HarnessAdapter[] = [
  {
    slug: 'opencode',
    label: 'opencode',
    auth: 'gateway',
    gatewayEnv: { OPENAI_BASE_URL: '${LLM_BASE_URL}', OPENAI_API_KEY: '${LLM_API_KEY}', OPENCODE_CONFIG: '/opt/workbench-config/opencode.json' },
    mcpConfig: 'opencode-json',
    invoke: 'opencode run --model openai/<model> "<task>"',
  },
  {
    slug: 'claude-code',
    label: 'Claude Code',
    auth: 'native',
    native: { provider: 'anthropic', envVar: 'ANTHROPIC_API_KEY' },
    gatewayEnv: {},
    mcpConfig: 'claude-json',
    invoke: 'claude -p "<task>" --model <model> --mcp-config /opt/workbench-config/mcp.json',
  },
  {
    slug: 'codex',
    label: 'Codex CLI',
    auth: 'gateway',
    gatewayEnv: { OPENAI_BASE_URL: '${LLM_BASE_URL}', OPENAI_API_KEY: '${LLM_API_KEY}' },
    mcpConfig: 'none',
    invoke: 'codex exec --model <model> "<task>"',
  },
]

export const harness = (slug: string): HarnessAdapter | undefined => HARNESSES.find((h) => h.slug === slug)

export type Effort = 'light' | 'standard' | 'heavy'

/** Effort → model, with a fall-down chain so unset slots never strand work:
 *  heavy → standard → light → utility chain → null (caller surfaces it). */
export async function effortModel(effort: Effort): Promise<string | null> {
  const order: Effort[] = effort === 'heavy' ? ['heavy', 'standard', 'light'] : effort === 'standard' ? ['standard', 'light'] : ['light']
  for (const e of order) {
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
export async function effortModels(): Promise<Record<Effort, string | null>> {
  return {
    light: await effortModel('light'),
    standard: await effortModel('standard'),
    heavy: await effortModel('heavy'),
  }
}
