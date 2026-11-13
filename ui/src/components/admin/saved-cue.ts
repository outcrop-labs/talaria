// The header's "Saved" flash, for Admin's panels. Admin is one page of nine
// tabs and only some of them save on change, so the page says which
// (`ADMIN_TABS`) and owns the flash. The shared `useAutosave()` cannot serve a
// tabbed page: a panel that asks for its cue leaves it on after its tab is
// gone, and a tab with an explicit Save would then claim to save by itself.
import { getContext, setContext } from 'svelte'

const KEY = Symbol('admin-saved-cue')

/** Call in the page. `flash` lights "Saved" in its header. */
export function provideSavedCue(flash: () => void): void {
  setContext(KEY, flash)
}

/** Call during init in a panel whose fields save on change or blur, and call
 *  `flash()` when a save lands. Outside the page it does nothing. */
export function useSavedCue(): { flash: () => void } {
  return { flash: getContext<(() => void) | undefined>(KEY) ?? (() => {}) }
}
