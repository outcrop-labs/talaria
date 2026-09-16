// The desktop-shell bridge. Inside the Talaria desktop app (a Tauri shell
// wrapping this same web UI), the webview carries `window.__TAURI_INTERNALS__`
// and the shell grants this origin the switcher commands plus window chrome
// (titlebar mode, min/max/close/drag). In a browser none of that exists: the
// check is false, the dynamic import never runs, and every consumer renders
// nothing.
//
// A FUNCTION, not a const: module-load order in the SPA must never decide
// this. Tauri injects the internals at document-start, so a call at render
// time always sees the truth.

export const inDesktopShell = () =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window


/** Mirror of the shell's registry entry (desktop/src-tauri/src/registry.rs). */
export interface ShellInstance {
  id: string
  instanceId: string
  label: string
  url: string
  createdAt: number
  lastOpenedAt: number | null
}

// Lazy on purpose: browsers must never download @tauri-apps/api, and a static
// import would put it in the main chunk. Only the desktop webview pays it.
const ipc = () => import('@tauri-apps/api/core').then((m) => m.invoke)

export async function shellListInstances(): Promise<ShellInstance[]> {
  if (!inDesktopShell()) return []
  return ipc().then((invoke) => invoke<ShellInstance[]>('list_instances'))
}

export async function shellActivateInstance(id: string): Promise<void> {
  if (!inDesktopShell()) return
  await ipc().then((invoke) => invoke('activate_instance', { id }))
}

export async function shellShowWelcome(): Promise<void> {
  if (!inDesktopShell()) return
  await ipc().then((invoke) => invoke('show_welcome'))
}

export type TitlebarMode = 'themed' | 'os' | 'none'

export interface DesktopSettings {
  titlebar: TitlebarMode
}

export type DesktopWindowAction = 'minimize' | 'toggleMaximize' | 'close' | 'startDragging'

export async function shellGetDesktopSettings(): Promise<DesktopSettings> {
  if (!inDesktopShell()) return { titlebar: 'themed' }
  return ipc().then((invoke) => invoke<DesktopSettings>('get_desktop_settings'))
}

export async function shellSetTitlebarMode(mode: TitlebarMode): Promise<DesktopSettings> {
  if (!inDesktopShell()) return { titlebar: mode }
  return ipc().then((invoke) => invoke<DesktopSettings>('set_titlebar_mode', { mode }))
}

export async function shellDesktopWindow(action: DesktopWindowAction): Promise<void> {
  if (!inDesktopShell()) return
  await ipc().then((invoke) => invoke('desktop_window', { action }))
}
