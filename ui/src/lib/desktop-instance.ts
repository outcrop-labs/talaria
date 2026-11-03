/** Pure helpers for the in-UI desktop switcher. Kept out of desktop-shell.ts
 *  so tests don't load the Tauri bridge. */

import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'

export function instanceHost(url: string): string {
  return url.replace(/^https?:\/\//, '')
}

/** The name the switcher shows. An instance with no company name still has a
 *  host — an empty label made rows look like they'd vanished from the menu. */
export function instanceDisplayLabel(instance: { label: string; url: string }): string {
  const label = instance.label.trim()
  return label || instanceHost(instance.url)
}

/** Current instance: beacon uuid first, then this webview's origin. The origin
 *  fallback is what keeps the row checked when the operator hasn't set a
 *  company name (and the branding query is still in flight). */
export function findCurrentInstance<T extends { instanceId: string; url: string }>(
  instances: T[],
  beaconId: string | null | undefined,
  origin: string,
): T | undefined {
  const id = (beaconId ?? '').trim()
  if (id) {
    const hit = instances.find((i) => i.instanceId === id)
    if (hit) return hit
  }
  const originNorm = origin.replace(/\/$/, '')
  return instances.find((i) => i.url.replace(/\/$/, '') === originNorm)
}

/** The reload shortcut as this OS spells it: what the shell's key script
 *  takes (desktop/src-tauri/src/reload.rs). */
export function reloadShortcut(userAgent: string): string {
  return /Mac|iPhone|iPad/.test(userAgent) ? '⌘R' : 'Ctrl+R'
}

export interface SwitcherActions {
  activate: (id: string) => void
  reload: () => void
  manage: () => void
  quit: () => void
}

/** The switcher's menu: every instance with the current one checked, then
 *  this page's Reload, the launcher, and Quit. Closing the window keeps
 *  Talaria running so notifications arrive, so the menu ends with the way out
 *  (Ctrl+Q works too: the shell handles it in every page, whatever its Talaria
 *  version). Reload is here because the page has no browser around it. */
export function switcherMenu(
  instances: { id: string; label: string; url: string }[],
  currentId: string | undefined,
  userAgent: string,
  act: SwitcherActions,
): ContextMenuEntry[] {
  return [
    ...instances.map(
      (instance): ContextMenuEntry => ({
        label: instanceDisplayLabel(instance),
        checked: instance.id === currentId,
        onSelect: () => act.activate(instance.id),
      }),
    ),
    ...(instances.length ? ['sep' as const] : []),
    { label: 'Reload page', hint: reloadShortcut(userAgent), onSelect: act.reload },
    { label: 'Add or manage instances', onSelect: act.manage },
    'sep',
    { label: 'Quit Talaria', onSelect: act.quit },
  ]
}
