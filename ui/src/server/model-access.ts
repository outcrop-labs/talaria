// Member access to gateway models. Admins register models; this decides which
// of them NON-admins may pick (preferred model, muse drafting). Empty list =
// all models (open by default, like agent access); non-empty = exactly those.
// Admins are never restricted — they control when the expensive brains run.
import { getSetting, setSetting } from './audit'
import { gatewayModels, type GatewayModel } from './llm-gateway'
import type { Role } from './users'

const KEY = 'member_model_allowlist'

/** Bare model ids members may use. Empty = no restriction. */
export async function memberModelAllowlist(): Promise<string[]> {
  return getSetting<string[]>(KEY, [])
}

export async function setMemberModelAllowlist(ids: string[]): Promise<void> {
  await setSetting(KEY, [...new Set(ids.map((s) => s.trim()).filter(Boolean))])
}

/** May this role use this model id? Endpoint-qualified ids are judged by the
 *  model they pin (allowing "m" allows "ep/m" on any endpoint). */
export function modelAllowedFor(role: Role, model: string, allow: string[], catalog: GatewayModel[]): boolean {
  if (role === 'admin' || allow.length === 0) return true
  if (allow.includes(model)) return true
  const entry = catalog.find((m) => m.id === model)
  if (entry?.qualified) {
    // "ep/rest": allowed iff the pinned bare model is allowed.
    const rest = model.slice(model.indexOf('/') + 1)
    return allow.includes(rest)
  }
  return false
}

/** The gateway catalog as this role may see it. */
export async function gatewayModelsFor(role: Role): Promise<GatewayModel[]> {
  const all = await gatewayModels()
  if (role === 'admin') return all
  const allow = await memberModelAllowlist()
  if (allow.length === 0) return all
  return all.filter((m) => modelAllowedFor(role, m.id, allow, all))
}
