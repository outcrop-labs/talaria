// Shell window-chrome settings. The Tauri window is shared across every
// instance webview, so this lives in the desktop shell (settings.json next
// to the instance registry), not in the instance's own prefs.

import {
  inDesktopShell,
  shellDesktopWindow,
  shellGetDesktopSettings,
  shellSetSuspendAfter,
  shellSetTitlebarMode,
  type DesktopSettings,
  type TitlebarMode,
} from './desktop-shell'

let settings = $state<DesktopSettings>({ titlebar: 'themed' })
let loaded = $state(false)

export function desktopSettings(): DesktopSettings {
  return settings
}

export function desktopSettingsLoaded(): boolean {
  return loaded
}

let watching = false

export async function loadDesktopSettings(): Promise<void> {
  if (!inDesktopShell()) return
  // The window is shared: another instance (or the launcher) can change the
  // titlebar mode while this webview is hidden but loaded. Re-read it each
  // time this instance is shown again, or the themed bar it drew at load
  // stays up under the OS titlebar.
  if (!watching && typeof document !== 'undefined') {
    watching = true
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden) void shellGetDesktopSettings().then((s) => (settings = s), () => {})
    })
  }
  settings = await shellGetDesktopSettings()
  loaded = true
}

export async function setTitlebarMode(mode: TitlebarMode): Promise<void> {
  settings = await shellSetTitlebarMode(mode)
}

export async function setSuspendAfter(minutes: number): Promise<void> {
  settings = await shellSetSuspendAfter(minutes)
}

export function desktopWindow(
  action: 'minimize' | 'toggleMaximize' | 'close' | 'startDragging',
): void {
  void shellDesktopWindow(action)
}
