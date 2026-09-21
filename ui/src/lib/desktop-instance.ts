/** Pure helpers for the in-UI desktop switcher. Kept out of desktop-shell.ts
 *  so tests don't load the Tauri bridge. */

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
