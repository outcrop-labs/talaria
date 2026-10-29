/** Typed wrappers over the shell commands (src-tauri/src/commands.rs). */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export interface Instance {
  id: string
  instanceId: string
  label: string
  url: string
  createdAt: number
  lastOpenedAt: number | null
}

export type TitlebarMode = 'themed' | 'os' | 'none'

export interface DesktopSettings {
  titlebar: TitlebarMode
}

export type DesktopWindowAction = 'minimize' | 'toggleMaximize' | 'close' | 'startDragging'

/** An instance whose page failed to load; the launcher offers a retry. */
export interface LoadFailure {
  seq: number
  id: string
  label: string
  message: string
}

/** Everything the launcher renders (commands.rs::ShellSnapshot). */
export interface ShellSnapshot {
  view: 'welcome' | 'active'
  activeId: string | null
  titlebar: TitlebarMode
  instances: Instance[]
  /** Instances with a live webview this session — opening one is instant. */
  loaded: string[]
  notices: string[]
  /** The instance to reopen at boot, until the first view change. */
  reopen: string | null
  failure: LoadFailure | null
}

/** What `add_instance` answers when the person cancelled it. */
export const CANCELLED = 'cancelled'

export const listInstances = () => invoke<Instance[]>('list_instances')

export const getShellState = () => invoke<ShellSnapshot>('get_shell_state')

/** Rust pushes the snapshot on every change, from either side of the shell. */
export const onShellState = (fn: (s: ShellSnapshot) => void): Promise<UnlistenFn> =>
  listen<ShellSnapshot>('shell-state', (e) => fn(e.payload))

export const addInstance = (url: string) => invoke<Instance>('add_instance', { url })

export const cancelAddInstance = () => invoke<boolean>('cancel_add_instance')

export const probeInstance = (id: string) => invoke<void>('probe_instance', { id })

export const activateInstance = (id: string) => invoke<void>('activate_instance', { id })

export const showWelcome = () => invoke<void>('show_welcome')

export const removeInstance = (id: string) => invoke<Instance[]>('remove_instance', { id })

export const getDesktopSettings = () => invoke<DesktopSettings>('get_desktop_settings')

export const setTitlebarMode = (mode: TitlebarMode) =>
  invoke<DesktopSettings>('set_titlebar_mode', { mode })

export const desktopWindow = (action: DesktopWindowAction) =>
  invoke<void>('desktop_window', { action })

export interface DesktopUpdate {
  version: string
  notes: string | null
}

export const checkForUpdate = () => invoke<DesktopUpdate | null>('check_for_update')

export const installUpdate = () => invoke<void>('install_update')

/** Strip an invoke rejection down to the message the shell sent. */
export const errorText = (e: unknown): string =>
  String(e).replace(/^Error:\s*/, '').replace(/^"|"$/g, '')

/** The host:port of an origin, for the second line of a row. */
export const hostOf = (url: string) => url.replace(/^https?:\/\//, '')
