// Mercury theme — Talaria's two-mode design system (dark + light).
// Mirrors hermes-workspace's data-theme + .dark/.light contract so component
// lifts behave identically, but ships only the Mercury identity.

export type ThemeId = 'mercury' | 'mercury-light'

export const THEMES: Array<{
  id: ThemeId
  label: string
  description: string
  icon: string
}> = [
  {
    id: 'mercury',
    label: 'Mercury',
    description: 'Near-black instrument surfaces with a warm gold signal (default)',
    icon: '☿',
  },
  {
    id: 'mercury-light',
    label: 'Mercury Light',
    description: 'Warm paper-white surfaces with a deep gold accent (daylight)',
    icon: '☾',
  },
]

const STORAGE_KEY = 'talaria-theme'
export const DEFAULT_THEME: ThemeId = 'mercury'

export function isDarkTheme(theme: ThemeId): boolean {
  return theme === 'mercury'
}

export function isValidTheme(value: string | null | undefined): value is ThemeId {
  return value === 'mercury' || value === 'mercury-light'
}

/** The opposite-mode variant (for the light/dark toggle). */
export function toggleVariant(theme: ThemeId): ThemeId {
  return theme === 'mercury' ? 'mercury-light' : 'mercury'
}

export function getStoredTheme(): ThemeId {
  if (typeof window === 'undefined') return DEFAULT_THEME
  const stored = window.localStorage.getItem(STORAGE_KEY)
  return isValidTheme(stored) ? stored : DEFAULT_THEME
}

export function applyTheme(theme: ThemeId): void {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  root.setAttribute('data-theme', theme)
  const mode = isDarkTheme(theme) ? 'dark' : 'light'
  root.classList.remove('light', 'dark')
  root.classList.add(mode)
  root.style.setProperty('color-scheme', mode)
  if (typeof window !== 'undefined') window.localStorage.setItem(STORAGE_KEY, theme)
}

/**
 * Subscribe to theme FLIPS, with one observer for the whole page.
 *
 * A canvas cannot inherit a CSS variable the way DOM paint does, so anything
 * that resolves `--theme-*` into pixels has to be told when they change and
 * repaint — otherwise dark-theme dots sit on a paper-white surface until some
 * later frame happens to run. `applyTheme` writes both the attribute and the
 * mode class, and this watches both.
 *
 * Ref-counted, because the subscribers are canvas fields: skeletons exist only
 * while a query is in flight, and a button's bloom only while it is mounted.
 * One observer beats one per field — the app has hundreds of buttons.
 */
export function onThemeChange(cb: () => void): () => void {
  if (typeof document === 'undefined') return () => {}

  themeSubs.add(cb)
  if (!themeObserver) {
    themeObserver = new MutationObserver(() => {
      for (const sub of themeSubs) sub()
    })
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ['data-theme', 'class'],
    })
  }
  return () => {
    themeSubs.delete(cb)
    if (themeSubs.size === 0 && themeObserver) {
      themeObserver.disconnect()
      themeObserver = null
    }
  }
}

const themeSubs = new Set<() => void>()
let themeObserver: MutationObserver | null = null
