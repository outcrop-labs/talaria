// A person's timed mutes of an agent's DMs — its replies in their threads and
// its own check-ins — as the server holds them (`/api/me/agent-mutes`, the
// `agent_dm_mutes` table). While one runs, the agent's rows still land in the
// bell, quietly: no toast, push or mail. One query for the whole rail; a
// write puts the server's answer straight into it.

import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { getJson, putJson } from './fetch-json'
import { toastError } from './toast.svelte'
import { resolve, type MaybeGetter } from './reactive-arg'

export interface AgentMute {
  agentModel: string
  /** ISO; only mutes that still hold are served. */
  mutedUntil: string
}

export interface AgentMutes {
  /** The running mute's end for one agent (ISO), or null. */
  mutedUntil: (model: string) => string | null
  /** Mute until `until` (ISO), or unmute with null. */
  set: (model: string, until: string | null) => Promise<void>
}

export const agentMutesKey = (userId: string) => ['agent-mutes', userId] as const

/** Call during component init. */
export function useAgentMutes(userId: MaybeGetter<string | null | undefined>): AgentMutes {
  const qc = useQueryClient()
  const query = createQuery(() => {
    const id = resolve(userId) ?? null
    return {
      queryKey: agentMutesKey(id ?? 'anon'),
      enabled: id !== null,
      staleTime: 30_000,
      queryFn: async (): Promise<AgentMute[]> =>
        (await getJson<{ mutes: AgentMute[] }>('/api/me/agent-mutes')).mutes,
    }
  })
  return {
    mutedUntil: (model) => query.data?.find((m) => m.agentModel === model)?.mutedUntil ?? null,
    set: async (model, until) => {
      const id = resolve(userId)
      if (!id) return
      try {
        const r = await putJson<{ mutedUntil: string | null }>('/api/me/agent-mutes', {
          agentModel: model,
          mutedUntil: until,
        })
        qc.setQueryData<AgentMute[]>(agentMutesKey(id), (old = []) => [
          ...old.filter((m) => m.agentModel !== model),
          ...(r.mutedUntil ? [{ agentModel: model, mutedUntil: r.mutedUntil }] : []),
        ])
      } catch (e) {
        toastError(until ? 'Mute failed' : 'Unmute failed', e)
      }
      void qc.invalidateQueries({ queryKey: agentMutesKey(id) })
    },
  }
}
