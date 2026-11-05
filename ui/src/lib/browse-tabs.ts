// The Browse view's pure state derivation: everything the tab strip renders
// is computed here, so the route body (routes/app/Browse.svelte) stays a thin
// renderer. The shapes mirror the shell's registry (desktop/src-tauri/src/
// registry.rs `BrowserTab`, camelCase serde) via the bridge's exported
// interface — but this module takes the raw list as an argument and never
// imports the bridge or the svelte-query runtime (the module-split
// convention; workchains.test.ts pins the same seam).

import type { BrowserTab } from './desktop-shell'

export type { BrowserTab } from './desktop-shell'

/**
 * The shell's ordering contract: pinned tabs first, each side by creation
 * time (registry::load_tabs sorts on read). The bridge hands the list over
 * in that order; the strip must never silently re-sort what the shell said —
 * a violation means the shell and the UI disagree about order, which is a
 * bug to surface, not a cosmetic difference to paint over.
 * @returns the same list, if it holds the contract
 */
export function assertPinnedFirst(tabs: BrowserTab[]): BrowserTab[] {
  let seenUnpinned = false
  for (const t of tabs) {
    if (t.pinned && seenUnpinned) {
      throw new Error(
        `browse tab order violates the shell contract: pinned tab ${t.id} follows an unpinned one`
      )
    }
    seenUnpinned ||= !t.pinned
  }
  return tabs
}

/** The strip's two groups, each keeping the order the shell sent. */
export function tabGroups(tabs: BrowserTab[]): { pinned: BrowserTab[]; unpinned: BrowserTab[] } {
  const pinned: BrowserTab[] = []
  const unpinned: BrowserTab[] = []
  for (const t of tabs) {
    ;(t.pinned ? pinned : unpinned).push(t)
  }
  return { pinned, unpinned }
}

/**
 * What the strip shows for a tab: the page's own title, or — until the page
 * says one — the URL's host, the way a browser names a still-loading tab. A
 * URL that will not even parse (a shell bug) is shown as it is: the honest
 * string beats a blank.
 */
export function displayTitle(tab: BrowserTab): string {
  if (tab.title) return tab.title
  try {
    return new URL(tab.url).host || tab.url
  } catch {
    return tab.url
  }
}

/**
 * The payload the bridge sends for a pin toggle (`set_browser_tab_pinned`):
 * kept as one shape so the strip's buttons and any later surface send the
 * same thing. Pinning is always allowed — the person's say-so, never the
 * page's — so there is nothing to refuse.
 */
export function pinPayload(id: string, pinned: boolean): { id: string; pinned: boolean } {
  return { id, pinned }
}
