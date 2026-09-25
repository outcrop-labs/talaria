// The dock shell's client state — the successor to nav-rail.svelte.ts, which
// retires with the rail in the swap's second half.
//
// Two slots, two disciplines:
//
// DOCKED (persisted, `talaria:nav-docked`) — the dock shell's persisted
// choice, with the rail module's exact SSR discipline copied over: module
// `$state` starts from the server snapshot (docked — the dock is the
// resting shell), and the hook swaps in the persisted client snapshot in an
// `$effect` (post-hydration), so server markup and the client's first paint
// always agree. A FRESH KEY on purpose: `talaria:nav-collapsed` belongs to
// the rail and dies with it, and a person's rail choice must not leak into
// the dock shell as some half-remembered preference.
//
// MANAGE OPEN (session state, never persisted) — whether the manage sidebar
// is open right now. Deliberately not the same kind of state as `docked`:
// the pane is a gesture ("show me the control plane"), not a preference, and
// restoring it on reload would paint a 208px pane over whatever view the
// person actually came back for. Default closed. Both surfaces reach it
// through this one module — the dock's gear tile TOGGLES, the user menu's
// Manage entry OPENS (never toggles: the popover closing around the click
// must not read as a second toggle).
import { readFlag, writeFlag } from '@/lib/persist'

const NAV_DOCKED_KEY = 'talaria:nav-docked'

// In-memory fallback so the toggle still works when localStorage is
// unavailable (private mode) — the choice simply won't persist. It is also
// the answer for "nothing stored yet", which is the same thing on a first
// visit: the dock is the shell, so nobody having asked anything still gets
// the dock.
let dockedFallback = true

// The shared slot. One reactive module state IS the channel between the
// surfaces — the same trick the rail's store pulled.
const store = $state({ docked: true, manageOpen: false })

/** The dock shell's persisted state. Call during component init (it
 *  registers an `$effect`); read `.docked` where the answer is used so it
 *  stays reactive. */
export function useNavDocked(): { readonly docked: boolean; toggleDocked: () => void } {
  $effect(() => {
    // Post-hydration: adopt the persisted choice, then follow other tabs.
    store.docked = readFlag(NAV_DOCKED_KEY, dockedFallback)
    const onStorage = () => {
      store.docked = readFlag(NAV_DOCKED_KEY, dockedFallback)
    }
    window.addEventListener('storage', onStorage) // sync across tabs
    return () => window.removeEventListener('storage', onStorage)
  })
  return {
    get docked() {
      return store.docked
    },
    toggleDocked() {
      const next = !store.docked
      dockedFallback = next
      writeFlag(NAV_DOCKED_KEY, next)
      store.docked = next
    },
  }
}

// The manage sidebar's mutators, bare so a surface can ACT without holding a
// reactive read (the user menu's entry opens; the pane's rows close on
// navigate). Components that RENDER on the state read it through
// `useManageSidebar` below so the pane and the gear tile stay live.
export function openManageSidebar(): void {
  store.manageOpen = true
}

export function closeManageSidebar(): void {
  store.manageOpen = false
}

export function toggleManageSidebar(): void {
  store.manageOpen = !store.manageOpen
}

/** Reactive read of the manage sidebar's open state, for the components that
 *  paint on it (the pane itself, the dock's gear tile). */
export function useManageSidebar(): { readonly manageOpen: boolean } {
  return {
    get manageOpen() {
      return store.manageOpen
    },
  }
}