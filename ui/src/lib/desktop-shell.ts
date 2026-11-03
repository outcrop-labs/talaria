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

/** Reload this page through the shell, as its Ctrl+R / F5 / Cmd+R key script
 *  does (desktop/src-tauri/src/reload.rs). `hard` skips the HTTP cache where
 *  the engine can. A shell that predates the command, or any refusal, gets an
 *  ordinary reload instead: a person who asked to reload is never left with
 *  nothing. */
export async function shellReload(hard = false): Promise<void> {
  if (inDesktopShell()) {
    try {
      await ipc().then((invoke) => invoke('desktop_reload', { hard }))
      return
    } catch {
      // Below.
    }
  }
  window.location.reload()
}

export type TitlebarMode = 'themed' | 'os' | 'none'

export interface DesktopSettings {
  titlebar: TitlebarMode
  /** Minutes an instance's page may stay off screen before the shell
   *  suspends it; 0 is Off. Absent: a shell that predates suspending. */
  suspendAfterMinutes?: number
  /** Closing the window keeps the shell running so notifications arrive.
   *  Absent: a shell that predates it (Close quits). */
  closeToBackground?: boolean
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

export async function shellSetSuspendAfter(minutes: number): Promise<DesktopSettings> {
  if (!inDesktopShell()) return { titlebar: 'themed', suspendAfterMinutes: minutes }
  return ipc().then((invoke) => invoke<DesktopSettings>('set_suspend_after', { minutes }))
}

export async function shellDesktopWindow(action: DesktopWindowAction): Promise<void> {
  if (!inDesktopShell()) return
  await ipc().then((invoke) => invoke('desktop_window', { action }))
}

export interface DesktopUpdate {
  version: string
  notes: string | null
}

export async function shellCheckForUpdate(): Promise<DesktopUpdate | null> {
  if (!inDesktopShell()) return null
  return ipc().then((invoke) => invoke<DesktopUpdate | null>('check_for_update'))
}

export async function shellInstallUpdate(): Promise<void> {
  if (!inDesktopShell()) return
  await ipc().then((invoke) => invoke('install_update'))
}

/** One OS banner for this instance (interface I-2). `kind` is the
 *  notification's kind (`dm`, `approval_pending`): the shell picks urgency and
 *  category from it and drops anything that is not a plain slug. `renotify`
 *  re-alerts a banner already up under the same tag. An older shell ignores
 *  both. */
export async function shellShowNotification(n: {
  title: string
  body?: string
  tag?: string
  href?: string
  kind?: string
  renotify?: boolean
}): Promise<void> {
  if (!inDesktopShell()) return
  await ipc().then((invoke) =>
    invoke('desktop_notify', {
      title: n.title,
      body: n.body,
      tag: n.tag,
      href: n.href,
      kind: n.kind,
      renotify: n.renotify,
    }),
  )
}

/** What the shell knows about OS notifications, as the OS reports it now
 *  (desktop/src-tauri/src/notify/mod.rs `Status`). */
export interface ShellNotificationStatus {
  /** `fdo` (Linux), `un` (macOS), `ns-legacy (unbundled dev run)`, `winrt`,
   *  `winrt-powershell`. */
  backend: string
  permission: 'granted' | 'denied' | 'not-determined' | 'unavailable'
  /** The Linux notification server, as it names itself. */
  server?: { name: string; vendor: string; version: string; spec: string }
  capabilities?: string[]
  /** The reason in words: `unavailable: org.freedesktop.DBus.Error.…`. */
  detail?: string
  lastError?: string
  lastPostedAt?: number
  /** The last post had not been answered when the page was told. */
  pending: boolean
  closeToBackground: boolean
  /** What closing the window does on this desktop now. */
  background: 'tray' | 'menu-bar' | 'minimize' | 'none'
  /** This instance is muted in the launcher: no banners from it on this
   *  desktop; its unread count still counts toward the badge. Absent: a
   *  shell that predates muting. */
  muted?: boolean
}

export interface ShellTestResult {
  ok: boolean
  error?: string
  status: ShellNotificationStatus
}

/** A shell older than a command refuses it as unknown or not allowed: that
 *  is "this shell has no such feature", answered as null, not an error. */
export const isMissingCommand = (e: unknown): boolean =>
  /not found|not allowed|unknown command/i.test(String(e))

async function optional<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  if (!inDesktopShell()) return null
  const invoke = await ipc()
  try {
    return await invoke<T>(cmd, args)
  } catch (e) {
    if (isMissingCommand(e)) return null
    throw e
  }
}

/** The notification service's state. Null: a shell that predates the
 *  command, or a browser. */
export const shellNotificationStatus = (): Promise<ShellNotificationStatus | null> =>
  optional<ShellNotificationStatus>('desktop_notification_status')

/** Ask the OS (macOS shows its prompt, or opens System Settings when denied
 *  before). From a click only. */
export const shellRequestNotificationPermission = (): Promise<ShellNotificationStatus | null> =>
  optional<ShellNotificationStatus>('desktop_notification_request')

/** One real banner through the shell's own path, and what happened to it. */
export const shellSendTestNotification = (): Promise<ShellTestResult | null> =>
  optional<ShellTestResult>('desktop_notification_test')

/** This instance's unread count, for the dock and launcher badge. Best
 *  effort: a badge that cannot be set is not worth an error. */
export async function shellSetBadge(count: number): Promise<void> {
  try {
    await optional('desktop_set_badge', { count: Math.max(0, Math.floor(count)) })
  } catch {
    // Swallowed — see above.
  }
}

/** Quit the desktop app (Close only hides it, so notifications arrive). */
export async function shellQuit(): Promise<void> {
  await optional('desktop_quit')
}

/** Whether closing the window keeps the app running. Null: an older shell. */
export const shellSetCloseToBackground = (on: boolean): Promise<DesktopSettings | null> =>
  optional<DesktopSettings>('set_close_to_background', { on })

/** Close this instance's OS banners for these tags (I-22) — a read elsewhere
 *  clears them here (I-16). The shell keys them by this page's own webview,
 *  so another instance's tags never close these, and takes at most 100 a
 *  call. False: a browser, or a shell that predates the command. */
export async function shellCloseNotifications(tags: string[]): Promise<boolean> {
  try {
    return (await optional<unknown>('desktop_notify_close', { tags })) !== null
  } catch {
    return false
  }
}

/** Milliseconds since the last OS input (I-22) — what a desktop client's
 *  presence `state` is computed from. Null: a browser, a shell that predates
 *  the command, or an OS that cannot say. One reading answers every page for
 *  half a second, so asking often costs nothing. */
export async function shellOsIdleMs(): Promise<number | null> {
  try {
    const ms = await optional<number>('desktop_os_idle')
    return typeof ms === 'number' && Number.isFinite(ms) && ms >= 0 ? ms : null
  } catch {
    return null
  }
}
