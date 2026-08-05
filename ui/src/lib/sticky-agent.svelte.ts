import type { AgentModel } from '@/lib/agents'

// Remembers which agent you were last talking to on a surface, and lets links
// elsewhere deep-link straight into a conversation with a specific agent via
// `?agent=<model>`. Precedence: query param → last-used (localStorage) → first.

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (the agents list loads from a query). */
type MaybeGetter<T> = T | (() => T)
const resolveValue = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

/** React returned a `[selected, select]` tuple; destructuring a rune would
 *  freeze it, so Svelte callers read `sticky.selected` and call
 *  `sticky.select(id)`. */
export function useStickyAgent(surface: 'chat' | 'plan' | 'research', agents: MaybeGetter<AgentModel[]>) {
  const key = `talaria.agent.${surface}`
  let selected = $state<string | null>(null)

  $effect(() => {
    const list = resolveValue(agents)
    if (selected || list.length === 0) return
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
    const known = (id: string | null) => !!id && list.some((a) => a.id === id)
    selected = (known(deepLink) && deepLink) || (known(stored) && stored) || list[0]!.id
  })

  const select = (id: string) => {
    selected = id
    try {
      localStorage.setItem(key, id)
    } catch {
      /* private mode / no storage */
    }
  }

  return {
    /** The selected agent id (null until the agents list arrives). */
    get selected() {
      return selected
    },
    select,
  }
}
