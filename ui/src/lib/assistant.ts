// Client for the signed-in user's personal assistant (/api/me/assistant).
import { createQuery } from '@tanstack/svelte-query'
import { errorMessage, getJson, patchJson, postJson } from '@/lib/fetch-json'

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
  return createQuery(() => ({
    queryKey: ['my-assistant'],
    // "You have no assistant yet" is a 200 carrying `{ assistant: null }` —
    // the route never 404s — so any non-2xx is a failure, not an absence.
    queryFn: async (): Promise<Assistant | null> =>
      (await getJson<{ assistant: Assistant | null }>('/api/me/assistant')).assistant,
  }))
}

export interface AssistantResult {
  assistant?: Assistant
  error?: string
}

// Both writers resolve an AssistantResult — the wizard and the section read
// `r.error` rather than catching, so the door's rejection is folded back into
// that envelope.
export async function createAssistant(input: { name?: string; handle?: string; personality?: string }): Promise<AssistantResult> {
  try {
    return await postJson<AssistantResult>('/api/me/assistant', input)
  } catch (e) {
    return { error: errorMessage(e) }
  }
}

export async function updateAssistant(patch: {
  name?: string
  handle?: string
  personality?: string
  /** A tier name from `tiers` — becomes the default model. */
  model?: string
}): Promise<AssistantResult> {
  try {
    return await patchJson<AssistantResult>('/api/me/assistant', patch)
  } catch (e) {
    return { error: errorMessage(e) }
  }
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
