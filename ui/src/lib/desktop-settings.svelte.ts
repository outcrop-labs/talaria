// Shell window-chrome settings. The Tauri window is shared across every
// instance webview, so this lives in the desktop shell (settings.json next
// to the instance registry), not in the instance's own prefs.

import {
  inDesktopShell,
  shellDesktopWindow,
  shellGetDesktopSettings,
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

export async function loadDesktopSettings(): Promise<void> {
  if (!inDesktopShell()) return
  settings = await shellGetDesktopSettings()
  loaded = true
}

export async function setTitlebarMode(mode: TitlebarMode): Promise<void> {
  settings = await shellSetTitlebarMode(mode)
}

export function desktopWindow(
  action: 'minimize' | 'toggleMaximize' | 'close' | 'startDragging',
): void {
  void shellDesktopWindow(action)
}
