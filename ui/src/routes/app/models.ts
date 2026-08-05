// Shared constants/helpers for the Models page (Models.svelte + its panels).
import type { AffectedAgent } from '@/lib/models'

// The model-backend registry: providers + the models each offers. Agents'
// tiers pick from these catalogs; the class (local/cloud) drives the cost split.
export const MODEL_TABS = [
  { id: 'models', label: 'Models' },
  { id: 'roles', label: 'Roles' },
  { id: 'platform', label: 'Platform' },
  { id: 'access', label: 'Access' },
] as const
export type ModelsTab = (typeof MODEL_TABS)[number]['id']

export const describeAffected = (affected: AffectedAgent[]) =>
  affected
    .map((a) => `  • ${a.slug}${a.aliases.length ? ` (tiers: ${a.aliases.join(', ')})` : ''}${a.fallbacks ? ' (fallback)' : ''}`)
    .join('\n')

// The OpenRouter no-train routing default: deny data collection and restrict
// to US providers. No provider list is stored — the gateway injects the live
// US pool from OpenRouter's provider catalog on every call (llm-gateway).
export const OPENROUTER_NO_TRAIN = {
  provider: {
    data_collection: 'deny',
    allow_fallbacks: true,
  },
}
export const GENERIC_NO_TRAIN = { provider: { data_collection: 'deny' } }
