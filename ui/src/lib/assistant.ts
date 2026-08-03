// Client for the signed-in user's personal assistant (/api/me/assistant).
import { useQuery } from '@tanstack/react-query'
import { getJson } from '@/lib/fetch-json'

export interface AssistantTier {
  name: string
  model: string
  active: boolean
}

export interface Assistant {
  id: string
  slug: string
  model: string
  department: string
  displayName: string
  enabled: boolean
  personality: string | null
  running: boolean
  /** What powers it right now (the main target's model). */
  currentModel: string | null
  /** Named tiers the owner can switch between. */
  tiers: AssistantTier[]
}

export function useAssistant() {
  return useQuery({
    queryKey: ['my-assistant'],
    // "You have no assistant yet" is a 200 carrying `{ assistant: null }` —
    // the route never 404s — so any non-2xx is a failure, not an absence.
    queryFn: async (): Promise<Assistant | null> =>
      (await getJson<{ assistant: Assistant | null }>('/api/me/assistant')).assistant,
  })
}

export interface AssistantResult {
  assistant?: Assistant
  error?: string
}

export async function createAssistant(input: { name?: string; handle?: string; personality?: string }): Promise<AssistantResult> {
  const r = await fetch('/api/me/assistant', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(input),
  }).catch(() => null)
  return ((await r?.json().catch(() => null)) as AssistantResult | null) ?? { error: 'could not create your assistant' }
}

export async function updateAssistant(patch: {
  name?: string
  handle?: string
  personality?: string
  /** A tier name from `tiers` — becomes the default model. */
  model?: string
}): Promise<AssistantResult> {
  const r = await fetch('/api/me/assistant', {
    method: 'PATCH',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  }).catch(() => null)
  return ((await r?.json().catch(() => null)) as AssistantResult | null) ?? { error: 'could not update your assistant' }
}

/** Suggest a handle from a display name: "Maxie!" → "maxie". */
export function suggestHandle(name: string): string {
  const base = name
    .toLowerCase()
    .replace(/'s assistant$/i, '')
    .replace(/[^a-z0-9]/g, '')
    .slice(0, 30)
  return /^[a-z]/.test(base) && base.length >= 2 ? base : ''
}

export const HANDLE_RE = /^[a-z][a-z0-9]{1,29}$/
