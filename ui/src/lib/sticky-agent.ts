import { useEffect, useState } from 'react'
import type { AgentModel } from '@/lib/agents'

// Remembers which agent you were last talking to on a surface, and lets links
// elsewhere deep-link straight into a conversation with a specific agent via
// `?agent=<model>`. Precedence: query param → last-used (localStorage) → first.
export function useStickyAgent(
  surface: 'chat' | 'plan' | 'research',
  agents: AgentModel[],
): readonly [string | null, (id: string) => void] {
  const key = `talaria.agent.${surface}`
  const [selected, setSelected] = useState<string | null>(null)

  useEffect(() => {
    if (selected || agents.length === 0) return
    let deepLink: string | null = null
    try {
      const url = new URL(window.location.href)
      deepLink = url.searchParams.get('agent')
      if (deepLink) {
        url.searchParams.delete('agent') // consume it so a reload doesn't re-pin
        window.history.replaceState({}, '', url)
      }
    } catch {
      /* no window (SSR) */
    }
    const stored = typeof localStorage !== 'undefined' ? localStorage.getItem(key) : null
    const known = (id: string | null) => !!id && agents.some((a) => a.id === id)
    setSelected((known(deepLink) && deepLink) || (known(stored) && stored) || agents[0]!.id)
  }, [agents, selected, key])

  const select = (id: string) => {
    setSelected(id)
    try {
      localStorage.setItem(key, id)
    } catch {
      /* private mode / no storage */
    }
  }

  return [selected, select] as const
}
