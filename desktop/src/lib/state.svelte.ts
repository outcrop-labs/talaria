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
  errorText,
  getShellState,
  onShellState,
  probeInstance,
  removeInstance,
  setTitlebarMode,
  type Instance,
  type LoadFailure,
  type ShellSnapshot,
  type TitlebarMode,
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
}

export const shell = $state({
  instances: [] as Instance[],
  view: 'welcome' as ShellSnapshot['view'],
  activeId: null as string | null,
  titlebar: 'themed' as TitlebarMode,
  /** Instances with a live webview: opening them needs no probe. */
  loaded: [] as string[],
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
  shell.activeId = s.activeId
  shell.titlebar = s.titlebar
  shell.loaded = s.loaded
  shell.notices = s.notices.filter((n) => !dismissedNotices.has(n))
  shell.recentId = mostRecent(s.instances)
  applyFailure(s.failure)
  if (shell.confirmRemove && !s.instances.some((i) => i.id === shell.confirmRemove)) {
    shell.confirmRemove = null
  }
  shell.ready = true
}

/** A page that failed to load (Rust closed its webview and showed us). */
function applyFailure(failure: LoadFailure | null) {
  if (!failure || failure.seq <= dismissedFailure) return
  dismissedFailure = failure.seq
  shell.problem = { id: failure.id, label: failure.label, message: failure.message, probe: false }
}

export async function refresh() {
  try {
    sync(await getShellState())
  } catch (e) {
    shell.error = errorText(e)
    shell.ready = true
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
 * Open an instance. One without a live webview is probed first, so an
 * unreachable one becomes a message with a retry here instead of a blank
 * window, or WebKit's error page with no way back. `fresh` skips the probe:
 * the add just made it, or the person chose "Open anyway" over its answer.
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
  if (!o.fresh && !shell.loaded.includes(id)) {
    shell.opening = { id, label: instance.label }
    settle = true
    try {
      await probeInstance(id)
    } catch (e) {
      if (seq !== openSeq) return
      shell.opening = null
      shell.problem = { id, label: instance.label, message: errorText(e), probe: true }
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

export async function setTitlebar(mode: TitlebarMode) {
  shell.error = ''
  try {
    shell.titlebar = (await setTitlebarMode(mode)).titlebar
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
 * decides which (`reopen`); this probes it first like any cold open. */
export async function boot() {
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
  if (reopen) await open(reopen)
}
