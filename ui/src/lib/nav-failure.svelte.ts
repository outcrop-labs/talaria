// The one place a navigation failure becomes visible.
//
// sv-router's click interceptor preventDefaults the click BEFORE navigating,
// and on a failed lazy `import()` (stale chunk after a deploy, a dropped
// request, a top-level throw in the route module) `onNavigate` calls the
// route hooks' onError and rethrows — so the URL never updates, the view
// never swaps, and the throw lands outside any component boundary. Every
// click on a not-yet-loaded route re-fails the same way: the sidebar reads
// as dead until a full refresh, which is the bug this store answers.
//
// router.ts's onError hook reports here; NavigationFailureBanner (mounted in
// AppLayout) reads and clears. Kept separate from nav-failure.ts because
// runes need the Svelte compile pass and that module must stay unit-testable.
import { describeNavigationFailure, stillFailingMessage, type NavigationFailure } from './nav-failure'

const state = $state({ failure: null as NavigationFailure | null })

/** Called from the router's onError hook — the one choke point every failed
 *  navigation passes through after the click was already preventDefaulted. */
export function reportNavigationFailure(href: string, error: unknown): void {
  state.failure = { href, message: describeNavigationFailure(error), at: Date.now() }
}

export function clearNavigationFailure(): void {
  state.failure = null
}

/** Read `.current` where the answer is used so it stays reactive (the same
 *  discipline useNavCollapsed states). */
export function useNavigationFailure(): { readonly current: NavigationFailure | null } {
  return {
    get current() {
      return state.failure
    },
  }
}

/** The banner's Try again: re-attempt the failed navigation in THIS document.
 *  Resolves true when the attempt navigated (the store is cleared — the
 *  path-change effect in the banner does the same for any later successful
 *  navigation). Resolves false when it failed again: for chunk loads that is
 *  the expected shape wherever the engine caches the rejected import (Chrome
 *  does — verified), which is what Reload is for, and the message says so via
 *  stillFailingMessage instead of echoing the first banner's advice. */
export async function retryNavigation(navigate: (href: string) => Promise<unknown>): Promise<boolean> {
  const attempt = state.failure?.href
  if (!attempt) return false
  try {
    await navigate(attempt)
    if (state.failure?.href === attempt) state.failure = null
    return true
  } catch (error) {
    state.failure = { href: attempt, message: stillFailingMessage(error), at: Date.now() }
    return false
  }
}
