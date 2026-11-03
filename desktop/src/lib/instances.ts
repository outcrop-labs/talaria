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
  /** How long an instance's page may stay off screen before the shell
   *  suspends it, in minutes. 0 is Off. */
  suspendAfterMinutes: number
  /** Closing the window keeps Talaria running so notifications arrive. */
  closeToBackground: boolean
}

export type DesktopWindowAction = 'minimize' | 'toggleMaximize' | 'close' | 'startDragging'

/** An instance whose page did not come up (src-tauri/src/loading.rs): it got
 *  no answer and was closed (`failed`), it answered with something that is
 *  not Talaria and is kept behind the card (`blocked`: Show the page), or it
 *  has not loaded in 25 s and is still loading behind the card (`stalled`). */
export interface LoadFailure {
  seq: number
  id: string
  label: string
  message: string
  /** Absent: a shell from before the watchdog (a failure). */
  kind?: 'failed' | 'stalled' | 'blocked'
}

/** Everything the launcher renders (commands.rs::ShellSnapshot). */
export interface ShellSnapshot {
  view: 'welcome' | 'active'
  activeId: string | null
  titlebar: TitlebarMode
  /** 0 is Off. */
  suspendAfterMinutes: number
  instances: Instance[]
  /** Instances with a live webview this session — opening one is instant. */
  loaded: string[]
  /** Instances this session suspended: opening one recreates its webview at
   *  its last page, with no probe. */
  suspended: string[]
  notices: string[]
  /** The instance to reopen at boot, until the first view change. */
  reopen: string | null
  failure: LoadFailure | null
  /** Closing the window keeps Talaria running. */
  closeToBackground: boolean
  /** What closing does on this desktop now. */
  background: Background
  /** Instances muted here: no banners; the badge still counts. Absent: a
   *  shell that predates muting. */
  muted?: string[]
  /** Each instance's unread count (For you), by local id. */
  unread?: Record<string, number>
}

/** What closing the window does: hide to the tray (Linux) or the menu bar
 *  (macOS), minimize (a Linux desktop with no tray, stock GNOME), or quit
 *  (Windows). */
export type Background = 'tray' | 'menu-bar' | 'minimize' | 'none'

/** What the OS says about notifications (src-tauri/src/notify/mod.rs). */
export interface NotificationStatus {
  backend: string
  permission: 'granted' | 'denied' | 'not-determined' | 'unavailable'
  server?: { name: string; vendor: string; version: string; spec: string }
  detail?: string
  lastError?: string
  pending: boolean
  closeToBackground: boolean
  background: Background
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

/** Close an instance's page kept behind the card, still loading or not
 *  Talaria (the instance stays registered; its last page is forgotten). */
export const closeInstancePage = (id: string) => invoke<void>('close_instance_page', { id })

/** Show a page that is not Talaria as it is, with the shell's way back on it. */
export const showInstancePage = (id: string) => invoke<void>('show_instance_page', { id })

/** Mute an instance on this desktop: its banners stop, its badge share stays. */
export const setInstanceMuted = (id: string, muted: boolean) =>
  invoke<void>('set_instance_muted', { id, muted })

export const getDesktopSettings = () => invoke<DesktopSettings>('get_desktop_settings')

export const setTitlebarMode = (mode: TitlebarMode) =>
  invoke<DesktopSettings>('set_titlebar_mode', { mode })

export const setSuspendAfter = (minutes: number) =>
  invoke<DesktopSettings>('set_suspend_after', { minutes })

export const desktopWindow = (action: DesktopWindowAction) =>
  invoke<void>('desktop_window', { action })

export interface DesktopUpdate {
  version: string
  notes: string | null
}

export const checkForUpdate = () => invoke<DesktopUpdate | null>('check_for_update')

export const notificationStatus = () => invoke<NotificationStatus>('desktop_notification_status')

/** macOS: the system prompt (from this click; never at launch). */
export const requestNotifications = () => invoke<NotificationStatus>('desktop_notification_request')

export const quit = () => invoke<void>('desktop_quit')

export const installUpdate = () => invoke<void>('install_update')

/** Strip an invoke rejection down to the message the shell sent. */
export const errorText = (e: unknown): string =>
  String(e).replace(/^Error:\s*/, '').replace(/^"|"$/g, '')

/** The host:port of an origin, for the second line of a row. */
export const hostOf = (url: string) => url.replace(/^https?:\/\//, '')
