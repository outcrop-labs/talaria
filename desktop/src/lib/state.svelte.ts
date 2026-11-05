/** The launcher's whole state. Rust owns persistence, geometry and which view
 * the window shows; the launcher mirrors it (`sync`) — at boot, on every
 * `shell-state` event Rust pushes, and whenever this webview becomes visible
 * again — and owns only what is on screen here: the dialogs, an open in
 * progress, and messages. Nothing is read once and trusted after: an
 * instance's switcher, its Settings → Profile titlebar choice and a
 * notification click all change shell state while the launcher is hidden. */

import { tick } from 'svelte'
import {
  activateInstance,
  closeInstancePage,
  errorText,
  getShellState,
  notificationStatus,
  onShellState,
  installUpdate,
  probeInstance,
  removeInstance,
  requestNotifications,
  setInstanceMuted,
  setSuspendAfter,
  setTitlebarMode,
  showInstancePage,
  snoozeUpdate,
  type Background,
  type BrowserTab,
  type Instance,
  type LoadFailure,
  type NotificationStatus,
  type ShellSnapshot,
  type TitlebarMode,
  type UpdateOffer,
} from './instances'

export interface Problem {
  id: string
  label: string
  message: string
  /** The shell's own check said no, not the page: the card offers "Open
   * anyway". The check is not the webview — no cookies, its own proxy lookup
   * and TLS trust — so an instance behind a sign-in gate, or behind a proxy
   * only the webview uses, can fail it and still load. */
  probe: boolean
  /** The page has not loaded and is still loading behind the card: it also
   * offers Keep waiting, and Retry and Back close it first. */
  stalled: boolean
  /** The page answered with something that is not Talaria (a challenge, a
   * sign-in gate) and is kept behind the card: it also offers Show the page,
   * and Retry and Back close it first. */
  blocked: boolean
}
export const shell = $state({
  instances: [] as Instance[],
  view: 'welcome' as ShellSnapshot['view'],
  activeId: null as string | null,
  titlebar: 'themed' as TitlebarMode,
  /** Minutes off screen before an instance is suspended; 0 is Off. */
  suspendAfterMinutes: 10,
  /** Instances with a live webview: opening them needs no probe. */
  loaded: [] as string[],
  /** Instances the shell suspended: warm, so no probe either. */
  suspended: [] as string[],
  /** Startup notices (an unreadable registry or settings file). */
  notices: [] as string[],
  /** The first snapshot has arrived. */
  ready: false,
  /** The instance the person was last in — the highlighted row. */
  recentId: null as string | null,
  /** An open waiting on its probe: "Opening X…", with a cancel. */
  opening: null as { id: string; label: string } | null,
  /** Why the last open did not happen, with a retry. */
  problem: null as Problem | null,
  /** Any other launcher-level failure (a remove, a titlebar change). */
  error: '',
  adding: false,
  confirmRemove: null as string | null,
  /** What closing the window does on this desktop (the snapshot's). */
  background: 'none' as Background,
  closeToBackground: true,
  /** The OS's notification state, read at boot and on every return here. */
  notify: null as NotificationStatus | null,
  /** The macOS prompt is up (the Allow card's click). */
  asking: false,
  /** Instances muted on this desktop: no banners, the badge still counts. */
  muted: [] as string[],
  /** Each instance's unread count (For you), by local id. */
  unread: {} as Record<string, number>,
  /** The update chip (Rust decides; every view shows the same one). */
  update: null as UpdateOffer | null,
  /** The Browse view's tabs (TALA-83), pinned first. Absent: a shell that
   *  predates browse reads as none. */
  browserTabs: [] as BrowserTab[],
  /** The Browse view's tab on screen, by local id. */
  activeTab: null as string | null,
})


let dismissedFailure = 0
const dismissedNotices = new Set<string>()
let openSeq = 0

const mostRecent = (instances: Instance[]) =>
  instances
    .filter((i) => i.lastOpenedAt !== null)
    .reduce<Instance | null>((a, b) => (a && (a.lastOpenedAt ?? 0) >= (b.lastOpenedAt ?? 0) ? a : b), null)
    ?.id ?? null

function sync(s: ShellSnapshot) {
  shell.instances = s.instances
  shell.view = s.view
  shell.update = s.update ?? null
  shell.browserTabs = s.browserTabs ?? []
  shell.activeTab = s.activeTab ?? null
  shell.recentId = mostRecent(s.instances)
  shell.suspendAfterMinutes = s.suspendAfterMinutes ?? shell.suspendAfterMinutes
  shell.loaded = s.loaded
  shell.muted = s.muted ?? []
  shell.unread = s.unread ?? {}
  shell.update = s.update ?? null
  shell.browserTabs = s.browserTabs ?? []
  shell.activeTab = s.activeTab ?? null
  shell.closeToBackground = s.closeToBackground ?? shell.closeToBackground
  shell.recentId = mostRecent(s.instances)
  applyFailure(s.failure)
  if (shell.confirmRemove && !s.instances.some((i) => i.id === shell.confirmRemove)) {
    shell.confirmRemove = null
  }
  shell.ready = true
}

/** A page that did not come up (Rust closed or kept its webview and showed
 * us). Gone from the snapshot: the instance was opened since (the shell
 * opens a page that answers while its card is up), so the card goes too; a
 * probe's card is the launcher's own and stays. */
function applyFailure(failure: LoadFailure | null) {
  if (!failure) {
    if (shell.problem && !shell.problem.probe) shell.problem = null
    return
  }
  if (failure.seq <= dismissedFailure) return
  dismissedFailure = failure.seq
  shell.problem = {
    id: failure.id,
    label: failure.label,
    message: failure.message,
    probe: false,
    stalled: failure.kind === 'stalled',
    blocked: failure.kind === 'blocked',
  }
}

export async function refresh() {
  try {
    sync(await getShellState())
  } catch (e) {
    shell.error = errorText(e)
    shell.ready = true
  }
  void readNotify()
}

/** The OS's notification state. A shell too old to say leaves it null. */
export async function readNotify() {
  try {
    shell.notify = await notificationStatus()
  } catch {
    shell.notify = null
  }
}

/** The Allow card's click: macOS asks the person, once. */
export async function allowNotifications() {
  shell.asking = true
  try {
    shell.notify = await requestNotifications()
  } catch (e) {
    shell.error = errorText(e)
  } finally {
    shell.asking = false
  }
}

/** Two frames, so whatever just changed on screen has been painted. Bounded,
 * because a hidden document never runs rAF. */
const painted = () =>
  Promise.race([
    new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
    new Promise<void>((resolve) => setTimeout(resolve, 120)),
  ])

/**
 * Open an instance. One opened cold (no webview this session) is probed
 * first, so an unreachable one becomes a message with a retry here instead of
 * a blank window, or WebKit's error page with no way back. A live one and a
 * suspended one are warm: the shell starts the load on the click, with no
 * probe. `fresh` skips the probe too: the add just made it, or the person
 * chose "Open anyway" over its answer.
 *
 * Before Rust hides this webview, the launcher is put at rest and painted:
 * WebKitGTK presents a hidden view's LAST frame when it is shown again, so a
 * dialog or status still on screen at hide time used to flash back on every
 * return to the launcher.
 */
export async function open(id: string, o: { fresh?: boolean; settle?: boolean } = {}) {
  const seq = ++openSeq
  const instance = shell.instances.find((i) => i.id === id)
  if (!instance) return
  shell.error = ''
  shell.problem = null
  let settle = o.settle ?? false
  const warm = shell.loaded.includes(id) || shell.suspended.includes(id)
  if (!o.fresh && !warm) {
    shell.opening = { id, label: instance.label }
    settle = true
    try {
      await probeInstance(id)
    } catch (e) {
      if (seq !== openSeq) return
      shell.opening = null
      shell.problem = { id, label: instance.label, message: errorText(e), probe: true, stalled: false, blocked: false }
      return
    }
    if (seq !== openSeq) return
  }
  shell.opening = null
  shell.recentId = id
  if (settle) {
    await tick()
    await painted()
    if (seq !== openSeq) return
  }
  try {
    await activateInstance(id)
  } catch (e) {
    shell.error = errorText(e)
  }
}

/** The card's Back to instances. A page still loading behind it is closed,
 * so nothing keeps trying in the background. */
export async function backToInstances(problem: Problem) {
  shell.problem = null
  if (!problem.stalled && !problem.blocked) return
  try {
    await closeInstancePage(problem.id)
    shell.loaded = shell.loaded.filter((id) => id !== problem.id)
  } catch (e) {
    shell.error = errorText(e)
  }
}

/** The card's Retry: a page still loading is closed first, so the open is a
 * fresh one, probe first, like any cold open. */
export async function retry(problem: Problem) {
  if (problem.stalled || problem.blocked) {
    try {
      await closeInstancePage(problem.id)
      shell.loaded = shell.loaded.filter((id) => id !== problem.id)
    } catch (e) {
      shell.error = errorText(e)
      return
    }
  }
  await open(problem.id)
}

/** The card's Keep waiting: back to the page, which is still loading; the
 * shell gives it longer before the card comes back. */
export async function keepWaiting(problem: Problem) {
  shell.problem = null
  shell.recentId = problem.id
  try {
    await activateInstance(problem.id)
  } catch (e) {
    shell.error = errorText(e)
  }
}

/** The card's Show the page: the page as it is (a challenge, a sign-in gate),
 * with the shell's "← Instances" on it as the way back. */
export async function showPage(problem: Problem) {
  shell.problem = null
  shell.recentId = problem.id
  try {
    await showInstancePage(problem.id)
  } catch (e) {
    shell.error = errorText(e)
  }
}

/** Stop waiting on an open's probe; stay on the launcher. */
export function cancelOpen() {
  openSeq++
  shell.opening = null
}

export async function remove(id: string) {
  shell.error = ''
  if (shell.problem?.id === id) shell.problem = null
  try {
    shell.instances = await removeInstance(id)
    shell.recentId = mostRecent(shell.instances)
  } catch (e) {
    shell.error = errorText(e)
  }
}

/** The bell on a row: mute or unmute that instance's banners here. */
export async function toggleMuted(id: string) {
  shell.error = ''
  try {
    await setInstanceMuted(id, !shell.muted.includes(id))
  } catch (e) {
    shell.error = errorText(e)
  }
}

export async function setTitlebar(mode: TitlebarMode) {
  shell.error = ''
  try {
    shell.titlebar = (await setTitlebarMode(mode)).titlebar
  } catch (e) {
    shell.error = errorText(e)
  }
}

export async function setSuspend(minutes: number) {
  shell.error = ''
  try {
    shell.suspendAfterMinutes = (await setSuspendAfter(minutes)).suspendAfterMinutes
  } catch (e) {
    shell.error = errorText(e)
  }
}

/** The chip's Update. Rust shows the install running and, if it fails, why,
 * in every view; it restarts Talaria when it is done. A refusal it answers
 * without a push (a second click, an older shell) still says why here. */
export async function updateNow() {
  const offer = shell.update
  if (!offer) return
  shell.update = { ...offer, busy: true, error: undefined }
  try {
    await installUpdate()
  } catch (e) {
    const error = errorText(e)
    if (shell.update?.version === offer.version) shell.update = { ...offer, busy: false, error }
  }
}

/** The chip's Later: that version is put off for a day, in every view. */
export async function updateLater() {
  shell.update = null
  try {
    await snoozeUpdate()
  } catch (e) {
    shell.error = errorText(e)
  }
}

export function dismissNotice(notice: string) {
  dismissedNotices.add(notice)
  shell.notices = shell.notices.filter((n) => n !== notice)
}

/** Boot: subscribe, read the state, and reopen where the person left off —
 * the instance they were in, or nothing if they quit on the launcher. Rust
 * decides which (`reopen`); this probes it first like any cold open.
 *
 * `asBar` is the browse bar's boot: the same document serves both surfaces
 * (the bar is its own webview of this same app), and only the bar's boot
 * skips the reopen — the bar never reopens an instance; it renders the tab
 * strip of the view it is already in. */
export async function boot(asBar = false) {
  try {
    await onShellState(sync)
  } catch (e) {
    shell.error = errorText(e)
  }
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) void refresh()
  })
  let reopen: string | null = null
  try {
    const s = await getShellState()
    sync(s)
    reopen = s.reopen
  } catch (e) {
    shell.error = errorText(e)
    shell.ready = true
  }
  void readNotify()
  // In the browse view the bar shares this document: the launcher webview
  // (hidden behind the bar) must not reopen the last instance over the
  // browse view the person is in — `reopen` is offered only until the first
  // view change, and entering browse already made one, so view decides.
  if (!asBar && reopen && shell.view === 'welcome') await open(reopen)
}
