// Where the dock's "Back to app" goes: the last address the person was at
// outside the settings area. PURE: the dock holds the value and feeds it every
// address it sees, so nothing here knows the router.

export const APP_HOME = '/home'

/**
 * The remembered address after seeing `href`.
 *
 * An address inside the area never replaces the memory. One that equals the
 * memory clears it: the area's app rows arrive with the apps read, so a page
 * opened straight on an app's manage surface is judged outside until that
 * read lands, and "back" must not point at the page it is on.
 */
export function rememberOutside(last: string | null, href: string, inside: boolean): string | null {
  if (!inside) return href
  return last === href ? null : last
}

export function backToApp(last: string | null): string {
  return last ?? APP_HOME
}
