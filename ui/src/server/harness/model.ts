// THE model-resolution chain. Every harness resolves its model through here.
//
// WHY THIS FILE EXISTS: the same eight lines — the platform-agent pin, then the
// Utility model role, then TALARIA_COPILOT_MODEL, then 'pl-main', then the first
// routable bare model — were hand-copied VERBATIM into the titler,
// skill-summaries, model-info and kb-okf harnesses, and with local variations
// into muse (adds the user's preference plus the member allowlist),
// workbench-harnesses (adds the effort fall-down) and comms-decay (pin, else
// the owner's muse). Seven spellings of one policy (AUDIT-HARNESS-2026-08-06,
// finding 1.10). Changing the policy meant finding all seven, and nobody ever
// did. Their heirs live in the Rust defs (api/src/harness/defs/,
// api/src/agent_skills.rs); this chain is the SPA-side spelling.
//
// AUDIT 1.7 — 'pl-main' was a BARE STRING at seven call sites. It is the
// reference deployment's main model and a perfectly good preference; it is a
// terrible literal, because on a self-host that never named an endpoint
// 'pl-main' the sites that STOP there resolve to nothing and the whole subsystem
// silently no-ops — no error, no log, just a feature that quietly isn't there.
// Here it survives only as a NAMED preference inside the last-resort step, which
// derives its answer from the live catalog either way and merely prefers this id
// when the gateway happens to serve it. No harness spells it again.
//
// RESOLUTION CONTRACT, inherited from model-roles.ts and platform-agents.ts and
// non-negotiable: a model only wins while it still ROUTES on the gateway. A
// deleted model can never silently break a subsystem — the harness falls to the
// next step instead. Every step in this file honors that (see `routes` below for
// exactly where each check happens and why some are not repeated).
import { gatewayModels, resolveRoute, type GatewayModel } from '../llm-gateway'
import { memberModelAllowlist, modelAllowedFor } from '../model-access'
import { resolveRoleModel, type ModelRole } from '../model-roles'
import { platformAgentModel, type PlatformAgentId } from '../platform-agents'
import { getPreferredModel, getUserRole, type Role } from '../users'

export type ModelChainStep = 'pin' | 'role' | 'utility' | 'env' | 'preferred' | 'first-routable'

export interface ModelSpec {
  /** Admin assignment slot on Models -> Platform (server/platform-agents.ts). */
  pin?: PlatformAgentId
  /** The model role this harness belongs to (server/model-roles.ts). */
  role?: ModelRole
  /** Order to try. Default: ['pin','role','utility','env','first-routable'].
   *
   *  AN EMPTY CHAIN IS A DECLARATION, NOT AN OVERSIGHT, and it is the right one
   *  for every harness whose model comes from the SUBJECT of the call — the
   *  owner's own assistant, the agent on the ticket, the agent in the channel or
   *  the plan, the researching agent, the search stage's mode-dependent sonar.
   *  Those callers pass `RunContext.model`, which short-circuits this function
   *  entirely (see `execute` in run.ts), so the chain is never consulted in
   *  production and its only job is to say what should happen if a caller ever
   *  forgets. `[]` answers "nothing, loudly": the run returns `no model
   *  available for harness "<id>"` and the feature no-ops with a sentence an
   *  operator can act on.
   *
   *  The alternative these harnesses used to declare — `['utility',
   *  'first-routable']`, on the theory that the fitness suite would need
   *  something to fall back to — is worse in both halves. The fitness suite
   *  PINS by construction (its whole question is "how does THIS model do"), so
   *  the fallback is never reached there either; and if a production caller
   *  ever dropped its pin, a work-session turn would run on the org's utility
   *  model, be filed to the ticket as the assigned agent's work, and look
   *  exactly like the agent having a bad day. A silent identity substitution is
   *  the one failure mode worth failing loudly to avoid. */
  chain?: ModelChainStep[]
  /** For user-scoped harnesses (muse, distiller): the owner, enabling the
   *  'preferred' step and the member model allowlist. */
  userId?: string
}

export interface ResolvedHarnessModel { model: string; step: ModelChainStep }

const DEFAULT_CHAIN: ModelChainStep[] = ['pin', 'role', 'utility', 'env', 'first-routable']

// The reference deployment names its main endpoint model 'pl-main'. Preferring
// it in the last-resort step reproduces today's behavior on that install
// (pl-main beat the alphabetical scan) without making any install DEPEND on the
// name: where it doesn't exist, the step still returns a real model instead of
// nothing. This list is the ONLY place in the codebase allowed to spell it.
const LAST_RESORT_PREFERENCE: string[] = ['pl-main']

/** Everything this chain reads from the rest of the server. Injected so the
 *  chain itself is testable without a database or a gateway: the ordering
 *  policy is the part that keeps getting re-derived wrong, and it is pure. */
export interface HarnessModelDeps {
  platformAgentModel: (id: PlatformAgentId) => Promise<string | null>
  resolveRoleModel: (role: ModelRole) => Promise<string | null>
  /** Does this model id land on an endpoint right now? */
  routes: (model: string) => Promise<boolean>
  gatewayModels: () => Promise<GatewayModel[]>
  getPreferredModel: (userId: string) => Promise<string | null>
  getUserRole: (userId: string) => Promise<Role>
  memberModelAllowlist: () => Promise<string[]>
  modelAllowedFor: (role: Role, model: string, allow: string[], catalog: GatewayModel[]) => boolean
  /** Read late, not at import time — a test must never have to touch process.env. */
  copilotEnvModel: () => string | null
}

const REAL_DEPS: HarnessModelDeps = {
  platformAgentModel,
  resolveRoleModel,
  routes: async (m) => (await resolveRoute(m)) !== null,
  gatewayModels,
  getPreferredModel,
  getUserRole,
  memberModelAllowlist,
  modelAllowedFor,
  copilotEnvModel: () => process.env.TALARIA_COPILOT_MODEL ?? null,
}

/** Memoize one async read for the duration of a single resolution: a chain that
 *  falls through five steps must not fetch the catalog five times. */
const once = <T>(f: () => Promise<T>): (() => Promise<T>) => {
  let p: Promise<T> | null = null
  return () => (p ??= f())
}

/** Null when the gateway serves nothing this spec can reach. Every step is
 *  validated with resolveRoute() before it wins - the existing contract that a
 *  deleted model can never silently break a subsystem (see model-roles.ts). */
export async function resolveHarnessModel(spec: ModelSpec): Promise<ResolvedHarnessModel | null> {
  return resolveHarnessModelWith(spec, REAL_DEPS)
}

/** The chain itself, over injected dependencies. Exported for tests; production
 *  callers want `resolveHarnessModel`. */
export async function resolveHarnessModelWith(spec: ModelSpec, deps: HarnessModelDeps): Promise<ResolvedHarnessModel | null> {
  const catalog = once(() => deps.gatewayModels())

  // The member model allowlist is ORG POLICY: an admin gating the expensive
  // brains decides which models a non-admin may be handed, and no refactor gets
  // to route around it (the muse def's step is the behavior being preserved
  // here — api/src/harness/defs/muse.rs). It
  // applies to the steps that hand a USER's own choice or the user-visible
  // catalog to a harness - 'preferred', 'role', 'utility', 'first-routable'.
  // It deliberately does NOT apply to 'pin' or 'env': an admin-assigned platform
  // agent model and a server env default are org policy themselves, set by the
  // people the allowlist exists to serve. Org-scoped harnesses (no userId -
  // titler, librarian, blurb-writer) have no owner to gate, so the gate is open.
  const gate = once(async (): Promise<(model: string) => boolean> => {
    const userId = spec.userId
    if (!userId) return () => true
    const [role, allow, cat] = await Promise.all([deps.getUserRole(userId), deps.memberModelAllowlist(), catalog()])
    return (model: string) => deps.modelAllowedFor(role, model, allow, cat)
  })
  const gated = async (model: string | null): Promise<string | null> => (model && (await gate())(model) ? model : null)

  // Where each step's routability check lives:
  //  - 'pin' and 'role'/'utility' are validated INSIDE platformAgentModel and
  //    resolveRoleModel, which is their documented contract. We do not re-check:
  //    resolveRoute advances a per-model round-robin cursor, so a second
  //    validating call would skew endpoint distribution for every harness run.
  //  - 'env' and 'preferred' are raw strings from config and from a user row.
  //    Nothing has vetted them, so they go through routes() here.
  //  - 'first-routable' reads the catalog, every entry of which is by
  //    construction served by a registered endpoint.
  const attempt = async (step: ModelChainStep): Promise<string | null> => {
    switch (step) {
      case 'pin':
        return spec.pin ? await deps.platformAgentModel(spec.pin) : null
      case 'role':
        return spec.role ? await gated(await deps.resolveRoleModel(spec.role)) : null
      case 'utility':
        return gated(await deps.resolveRoleModel('utility'))
      case 'env': {
        const m = deps.copilotEnvModel()
        return m && (await deps.routes(m)) ? m : null
      }
      case 'preferred': {
        if (!spec.userId) return null
        const pref = await gated(await deps.getPreferredModel(spec.userId))
        return pref && (await deps.routes(pref)) ? pref : null
      }
      case 'first-routable': {
        const allowed = await gate()
        // Bare ids only: an endpoint-qualified pin ("ep/model") would strand the
        // harness on one backend, which is the opposite of a last resort.
        // gatewayModels() sorts by id, so "first" is stable across runs.
        const bare = (await catalog()).filter((m) => !m.qualified && allowed(m.id))
        return bare.find((m) => LAST_RESORT_PREFERENCE.includes(m.id))?.id ?? bare[0]?.id ?? null
      }
    }
  }

  // The winning STEP is part of the answer, not a detail: the runner records it
  // on the harness_runs row, and the model-fitness UI reads those rows to show
  // an operator which fallback actually carried a harness in production. A
  // subsystem limping along on 'first-routable' for a month is a real finding,
  // and today it is invisible.
  for (const step of spec.chain ?? DEFAULT_CHAIN) {
    const model = await attempt(step)
    if (model) return { model, step }
  }
  return null
}
