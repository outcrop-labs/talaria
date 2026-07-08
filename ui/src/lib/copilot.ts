// Client for the drafting copilot + model preferences.
import { useQuery } from '@tanstack/react-query'

export type CopilotKind = 'soul' | 'personality' | 'skill' | 'memory' | 'cron' | 'document'

export interface CopilotRequest {
  kind: CopilotKind
  instruction: string
  current?: string
  context?: string
  chat?: Array<{ role: 'user' | 'assistant'; content: string }>
}

/** Stream a copilot draft; onChunk receives text pieces as they arrive.
 *  Returns the full text. Throws with the server's message on failure. */
export async function streamCopilot(input: CopilotRequest, onChunk: (piece: string) => void, signal?: AbortSignal): Promise<string> {
  const r = await fetch('/api/copilot', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(input),
    signal,
  })
  if (!r.ok || !r.body) {
    const j = (await r.json().catch(() => null)) as { error?: string } | null
    throw new Error(j?.error ?? `copilot failed (${r.status})`)
  }
  const reader = r.body.getReader()
  const decoder = new TextDecoder()
  let full = ''
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    const piece = decoder.decode(value, { stream: true })
    full += piece
    onChunk(piece)
  }
  return full
}

export interface GatewayModel {
  id: string
  endpoints: string[]
}

/** The model catalog + the model the caller's copilot resolves to right now. */
export function useModels() {
  return useQuery({
    queryKey: ['gateway-models'],
    staleTime: 60_000,
    queryFn: async (): Promise<{ models: GatewayModel[]; effective: string | null }> => {
      const r = await fetch('/api/models', { credentials: 'same-origin' })
      if (!r.ok) return { models: [], effective: null }
      return r.json()
    },
  })
}

export function usePreferredModel() {
  return useQuery({
    queryKey: ['profile-prefs'],
    queryFn: async (): Promise<{ preferredModel: string | null }> => {
      const r = await fetch('/api/profile', { credentials: 'same-origin' })
      if (!r.ok) return { preferredModel: null }
      return r.json()
    },
  })
}

export async function savePreferredModel(model: string | null): Promise<void> {
  await fetch('/api/profile', {
    method: 'PUT',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ preferredModel: model }),
  })
}

/** Parse the copilot's cron JSON (tolerates stray fences/prose around it). */
export function parseCronDraft(text: string): { name: string; schedule: string; prompt: string } | null {
  const m = /\{[\s\S]*\}/.exec(text)
  if (!m) return null
  try {
    const j = JSON.parse(m[0]) as { name?: string; schedule?: string; prompt?: string }
    if (!j.name || !j.schedule || !j.prompt) return null
    return { name: String(j.name), schedule: String(j.schedule), prompt: String(j.prompt) }
  } catch {
    return null
  }
}
