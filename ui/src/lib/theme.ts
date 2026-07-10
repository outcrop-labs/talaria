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
    description: 'Basalt near-black with violet→magenta neon: the winged-sandal HUD',
    icon: '☿',
  },
  {
    id: 'mercury-light',
    label: 'Mercury Light',
    description: 'Pale regolith grey with a deep violet accent: daylight HUD',
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
