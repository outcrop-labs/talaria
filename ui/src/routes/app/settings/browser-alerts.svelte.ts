import { useQueryClient } from '@tanstack/svelte-query'
import {
  browserNotifyEnabled,
  currentPushStatus,
  ensurePushSubscription,
  onPushStatus,
  permissionState,
  setBrowserNotifyPref,
  showOsNotification,
  stopPushSubscription,
  turnOnDesktopAlerts,
  watchPermission,
  type PermissionState,
  type PushStatus,
} from '@/lib/browser-notify'
import { inDesktopShell } from '@/lib/desktop-shell'
import { saveNotifyPrompt } from '@/lib/notifications'
import { blockedSteps, detectBrowser } from '@/lib/notify-help'
import { turnOnResult } from '@/lib/notify-prompt'

// THIS BROWSER'S DESKTOP ALERTS, as one live reading shared by the switch
// (BrowserNotifications) and the checklist behind Troubleshooting
// (BrowserChecks). One reading, so the checklist can never disagree with the
// switch above it. The permission is LIVE: a grant or a block made in the
// browser's own site settings shows up without a reload.
//
// In the desktop shell the webview's permission reads denied and there is no
// site-settings page; the shell posts natively and owns its own switch
// (Settings → Desktop).

export type ServiceWorkerState = 'active' | 'starting' | 'none' | 'unsupported'
export type BrowserAlerts = ReturnType<typeof useBrowserAlerts>

/** Call during component init. */
export function useBrowserAlerts() {
  const qc = useQueryClient()
  const shell = inDesktopShell()
  const browser = typeof navigator !== 'undefined' ? detectBrowser(navigator.userAgent, { brave: 'brave' in navigator }) : null
  const steps = browser ? blockedSteps(browser) : null

  let perm = $state<PermissionState>(permissionState())
  let on = $state(browserNotifyEnabled())
  let push = $state<PushStatus>(currentPushStatus())
  let note = $state<string | null>(null)

  $effect(() =>
    watchPermission((p) => {
      perm = p
      on = browserNotifyEnabled()
      if (p === 'granted' && on) void ensurePushSubscription()
    }),
  )
  $effect(() =>
    onPushStatus((s) => {
      push = s
      // The boot re-files this browser's subscription; once it lands, the
      // devices list (and "On the server") reads it afresh rather than
      // keeping a read that raced it.
      if (s.state === 'registered') void qc.invalidateQueries({ queryKey: ['push-devices'] })
    }),
  )

  // The worker and the subscription, as the browser reports them now —
  // re-read whenever the permission or the push status moves.
  let worker = $state<ServiceWorkerState>('none')
  let subscribedTo = $state<string | null>(null)
  const readWorker = async () => {
    if (shell || typeof navigator === 'undefined' || !('serviceWorker' in navigator)) {
      worker = 'unsupported'
      subscribedTo = null
      return
    }
    try {
      const reg = await navigator.serviceWorker.getRegistration()
      worker = reg?.active ? 'active' : reg ? 'starting' : 'none'
      const sub = await reg?.pushManager?.getSubscription()
      subscribedTo = sub ? new URL(sub.endpoint).host : null
    } catch {
      worker = 'none'
      subscribedTo = null
    }
  }
  $effect(() => {
    void perm
    void push
    void readWorker()
  })

  function turnOn() {
    note = null
    // Asked synchronously, inside this click (browser-notify.ts, rule 1).
    void turnOnDesktopAlerts().then(async (after) => {
      perm = after
      on = browserNotifyEnabled()
      const r = turnOnResult(after)
      if (r.kind === 'on') {
        await saveNotifyPrompt('granted')
        void qc.invalidateQueries({ queryKey: ['notifications'] })
        void qc.invalidateQueries({ queryKey: ['push-devices'] })
      } else if (r.kind === 'closed' || r.kind === 'unavailable') note = r.message
    })
  }

  const set = (wanted: boolean) => {
    if (wanted) {
      setBrowserNotifyPref(true)
      void ensurePushSubscription().then(() => qc.invalidateQueries({ queryKey: ['push-devices'] }))
    } else {
      // Browsers give no way to hand a grant back from a page; our own pref
      // is what makes "off" mean off — for the tab's copies AND the closed
      // browser's, whose subscription this retires at both ends.
      setBrowserNotifyPref(false)
      void stopPushSubscription().then(() => qc.invalidateQueries({ queryKey: ['push-devices'] }))
    }
    on = browserNotifyEnabled()
  }

  // A banner from this page, through the same registration a push lands in.
  // No server, no other device: "does this browser show Talaria's banners?"
  let testMsg = $state<{ text: string; ok: boolean } | null>(null)
  const testHere = async () => {
    testMsg = null
    const shown = await showOsNotification(
      { id: '', tag: 'talaria-test', title: 'Test notification', body: 'This is how Talaria alerts look in this browser.' },
      () => {},
    )
    testMsg = shown
      ? { text: 'Sent to the browser. If no banner appeared, check your system settings and Do Not Disturb.', ok: true }
      : { text: 'The browser refused the banner. Check that desktop alerts are allowed for this site.', ok: false }
  }

  return {
    shell,
    browser,
    steps,
    get perm() {
      return perm
    },
    get on() {
      return on
    },
    get push() {
      return push
    },
    get note() {
      return note
    },
    get worker() {
      return worker
    },
    get subscribedTo() {
      return subscribedTo
    },
    get testMsg() {
      return testMsg
    },
    turnOn,
    set,
    testHere,
  }
}
