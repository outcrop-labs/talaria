// Mercury shell (spec §5): one collapsible nav — 208px sidebar ⇄ 64px icon
// rail. The choice persists in localStorage, but the server can't see
// localStorage — reading it during the first render made SSR (expanded) and
// client (collapsed) disagree and hydration fail. The runes version keeps the
// same discipline useSyncExternalStore enforced: module `$state` starts from
// the server snapshot (expanded), and `useNavCollapsed` swaps in the persisted
// client snapshot in an `$effect` (post-hydration), so the server markup and
// the client's first paint always agree. NavRail and the AppLayout loading
// skeleton share this one module state so both frames always agree.
import { readFlag, writeFlag } from '@/lib/persist'

const NAV_COLLAPSED_KEY = 'talaria:nav-collapsed'

// In-memory fallback so the toggle still works when localStorage is
// unavailable (private mode) — the choice simply won't persist. It is also the
// answer for "nothing stored yet", which is the same thing on a first visit:
// nobody has asked for a collapsed rail.
let collapsedFallback = false

// The shared slot. The React version fanned writes out to subscribers through
// a window event; a single reactive module state IS that channel now.
const store = $state({ collapsed: false })

function toggleCollapsed(): void {
  const next = !store.collapsed
  collapsedFallback = next
  writeFlag(NAV_COLLAPSED_KEY, next)
  store.collapsed = next
}

/** Call during component init (it registers an `$effect`). Read `.collapsed`
 *  where the answer is used so it stays reactive. */
export function useNavCollapsed(): { readonly collapsed: boolean; toggleCollapsed: () => void } {
  $effect(() => {
    // Post-hydration: adopt the persisted choice, then follow other tabs.
    store.collapsed = readFlag(NAV_COLLAPSED_KEY, collapsedFallback)
    const onStorage = () => {
      store.collapsed = readFlag(NAV_COLLAPSED_KEY, collapsedFallback)
    }
    window.addEventListener('storage', onStorage) // sync across tabs
    return () => window.removeEventListener('storage', onStorage)
  })
  return {
    get collapsed() {
      return store.collapsed
    },
    toggleCollapsed,
  }
}
