import { getJson } from '@/lib/fetch-json'

// Shared shapes for the coding-account panels. One file so the agent panel,
// the login modal and the ticket strip agree on the wire rather than each
// re-declaring it slightly differently.

/** One signed-in coding account, as `/api/workbench/coding/accounts/{id}` lists it. */
export interface CodingAccount {
  id: number
  /** The service — the store id (`openai-codex`, never `openai-codex-device`). */
  provider: string
  /** The door the person signed in through, which may differ from `provider`. */
  loginProvider: string
  email: string | null
  accountId: string | null
  orgId: string | null
  orgName: string | null
  primary: boolean
  /** Whether the org still permits this service. */
  permitted: boolean
  expiresAt: string | null
  authorizedAt: string | null
  disabledAt: string | null
  disabledCause: string | null
  createdAt: string | null
  /** role → model, this account's own picks. */
  roles: Record<string, string>
}

/** What a job with no ticket pin would run on right now. */
export interface ResolvedPlan {
  accountId: number | null
  provider: string
  email?: string | null
  source: 'ticket' | 'primary'
  gateway: boolean
  models: Record<string, string>
}

export interface CodingAccountsBody {
  accounts: CodingAccount[]
  /** The gateway plan's own role picks — `account_id is null` server-side. */
  gatewayRoles: Record<string, string>
  roles: string[]
  resolved: ResolvedPlan | null
}

/** One service omp can sign in to, from omp's own roster. */
export interface CodingService {
  id: string
  name: string
  /** Where the credential is stored; the service the allowlist is about. */
  storeAs: string
  available?: boolean
  flow: 'oauth-code' | 'device-code' | 'custom' | 'api-key'
  /** Whether a pasted code can finish it — the path used when the browser
   *  cannot reach this instance's loopback. */
  pasteCode: boolean
  permitted?: boolean
  unavailable?: boolean
}

/** A login in flight, as the login routes report it. */
export interface CodingLogin {
  id: string
  provider: string
  phase: 'starting' | 'authorize' | 'waiting' | 'input' | 'done' | 'error'
  url: string | null
  launchUrl: string | null
  instructions: string | null
  progress: string | null
  prompt: {
    kind: 'prompt' | 'code'
    message: string
    placeholder: string | null
    secret: boolean
    allowEmpty: boolean
  } | null
  error: string | null
  /** Present only on the terminal poll that stored the credential. */
  accountId?: number
  email?: string | null
  orgName?: string | null
}

/** The Talaria gateway as a plan, so it renders beside the subscriptions. */
export const GATEWAY_PLAN = {
  accountId: null as number | null,
  provider: 'talaria',
  label: 'Talaria gateway',
  gateway: true,
  account: undefined as CodingAccount | undefined,
}

/** One model a plan can fill a role with. */
export interface CodingModel {
  id: string
  name?: string
}

/**
 * Fill `cache[provider]` with that provider's models, once.
 *
 * Both the agent panel and the ticket picker need this and neither should own
 * it: the list is per provider, not per surface, and a provider omp has no
 * catalog for answers with nothing rather than an error — the role can still
 * be left to fall back, so a missing catalog must not look like a failure.
 */
export async function loadModelsInto(
  cache: Record<string, CodingModel[]>,
  provider: string,
): Promise<void> {
  if (cache[provider]) return
  try {
    const body = await getJson<{ models: CodingModel[] }>(
      `/api/workbench/coding/services?models=${encodeURIComponent(provider)}`,
    )
    cache[provider] = body.models
  } catch {
    cache[provider] = []
  }
}

/** omp's role names, in words a person reads rather than omp's own. */
export function roleLabel(role: string): string {
  switch (role) {
    case 'default':
      return 'default'
    case 'smol':
      return 'small'
    case 'slow':
      return 'review'
    case 'plan':
      return 'planning'
    default:
      return role
  }
}

/** What this service is about to ask for — the sentence above the button. */
export function flowHint(service: CodingService): string {
  switch (service.flow) {
    case 'device-code':
      return 'Open the link and enter the code it shows. This page finishes on its own once you approve.'
    case 'oauth-code':
      return service.pasteCode
        ? 'Open the link and approve. If your browser cannot reach this server, paste the code or the full redirect URL it lands on.'
        : 'Open the link and approve.'
    case 'custom':
      return 'Follow the prompts — this service asks for a detail or two before it hands over the link.'
    default:
      return 'Follow the prompts.'
  }
}
