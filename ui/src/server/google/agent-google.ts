// Resolve which Google identity an agent acts as.
//
//   personal assistant (agent_defs.owner_user_id set) → acts as its OWNER
//   general fleet agent (no owner)                     → acts as the shared ORG
//
// agent_defs.model is unique and is exactly what an agent presents over MCP as
// x-agent-name, so a single lookup binds the caller to a Google connection.

import { db } from '../db/pg'
import { getAccessToken } from './connections'
import { getOrgAccessToken } from './org-connection'

export interface AgentGoogle {
  token: string
  /** Whose Drive the agent is acting in. */
  principal: 'owner' | 'org'
  ownerUserId: string | null
}

/** A Google access token for the calling agent, or null when the relevant
 *  connection isn't set up (owner hasn't connected / org account not configured).
 *  A personal assistant NEVER falls back to the org account — it acts strictly as
 *  its owner, so it can't silently write into the shared Drive. */
export async function resolveAgentGoogle(agentModel: string, nowMs: number): Promise<AgentGoogle | null> {
  const sql = await db()
  const [def] = await sql<{ ownerUserId: string | null }[]>`
    select owner_user_id as "ownerUserId" from agent_defs where model = ${agentModel} limit 1
  `

  if (def?.ownerUserId) {
    // Personal assistant: strictly its owner's identity.
    const token = await getAccessToken(def.ownerUserId, nowMs).catch(() => null)
    return token ? { token, principal: 'owner', ownerUserId: def.ownerUserId } : null
  }

  // General fleet agent (or unknown): the shared org account.
  const token = await getOrgAccessToken(nowMs).catch(() => null)
  return token ? { token, principal: 'org', ownerUserId: null } : null
}

/** The Talaria user an agent is the personal assistant OF, or null for a general
 *  fleet agent. Calendar/Gmail acting-as is owner-only — general agents don't get
 *  to read/send a human's mail or calendar. */
export async function resolveAgentOwnerUser(agentModel: string): Promise<string | null> {
  const sql = await db()
  const [def] = await sql<{ ownerUserId: string | null }[]>`
    select owner_user_id as "ownerUserId" from agent_defs where model = ${agentModel} limit 1
  `
  return def?.ownerUserId ?? null
}

export interface AgentPrincipal {
  /** true → the shared org account; false → the ownerUserId's account. */
  isOrg: boolean
  ownerUserId: string | null
}

/** Who an agent drafts/acts FOR — without needing a live token (used for queuing
 *  a pending action). Personal assistant → its owner; general agent → the org. */
export async function resolveAgentPrincipal(agentModel: string): Promise<AgentPrincipal> {
  const owner = await resolveAgentOwnerUser(agentModel)
  return owner ? { isOrg: false, ownerUserId: owner } : { isOrg: true, ownerUserId: null }
}
