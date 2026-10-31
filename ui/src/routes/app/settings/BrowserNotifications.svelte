<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import {
    browserNotifyEnabled,
    currentPushStatus,
    ensurePushSubscription,
    onPushStatus,
    permissionState,
    setBrowserNotifyPref,
    stopPushSubscription,
    turnOnDesktopAlerts,
    watchPermission,
    type PermissionState,
    type PushStatus,
  } from '@/lib/browser-notify'
  import { inDesktopShell } from '@/lib/desktop-shell'
  import { saveNotifyPrompt } from '@/lib/notifications'
  import { blockedSteps, detectBrowser, insecureExplanation } from '@/lib/notify-help'
  import { turnOnResult } from '@/lib/notify-prompt'

  // Desktop alerts: an OS banner when Talaria is not what the person is
  // looking at. In a browser that is the Notification API plus Web Push for
  // a closed browser, and this row says which of the five states this
  // browser is in — LIVE (a grant or a block made in the browser's own site
  // settings shows up here without a reload) — with the one control that
  // state allows, never a dead one:
  //
  //   default     → Turn on (the native prompt runs inside the click)
  //   granted     → On / Off (this browser's own switch) and whether this
  //                 browser is registered for closed-browser alerts
  //   denied      → where to unblock it: the browser's switch and the OS's
  //   insecure    → why a plain-http dev stack cannot alert, no control
  //   unsupported → that it cannot
  //
  // In the desktop shell the webview's permission reads denied and there is
  // no site-settings page; the shell posts natively and owns its own switch
  // (Settings → Desktop).
  const qc = useQueryClient()
  const shell = inDesktopShell()
  const OPTIONS = [
    { id: 'on' as const, label: 'On' },
    { id: 'off' as const, label: 'Off' },
  ]
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
  $effect(() => onPushStatus((s) => (push = s)))

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
      } else if (r.kind === 'closed' || r.kind === 'unavailable') note = r.message
    })
  }

  const set = (wanted: boolean) => {
    if (wanted) {
      setBrowserNotifyPref(true)
      void ensurePushSubscription()
    } else {
      // Browsers give no way to hand a grant back from a page; our own pref
      // is what makes "off" mean off — for the tab's copies AND the closed
      // browser's, whose subscription this retires at both ends.
      setBrowserNotifyPref(false)
      void stopPushSubscription()
    }
    on = browserNotifyEnabled()
  }
</script>

<div class="mt-5 border-t border-line-subtle pt-4" data-notify-settings={shell ? 'shell' : perm}>
  <div class="flex items-center gap-4">
    <div class="min-w-0 flex-1">
      <div class="text-sm text-fg">Desktop alerts</div>
      <div class="font-sans text-xs text-muted">
        {#if shell}
          Desktop app notifications are set in Settings → Desktop.
        {:else if perm === 'insecure'}
          {insecureExplanation(location.origin)}
        {:else if perm === 'unsupported'}
          This browser doesn't support desktop alerts.
        {:else if perm === 'denied'}
          Desktop alerts are blocked in {browser?.name ?? 'this browser'}.
        {:else if perm === 'default'}
          Off. Turn on to get a desktop alert when someone needs you — mentions, DMs, approvals — while Talaria is in
          another tab or window, and when this browser is closed.
        {:else if on}
          On. An alert pops up when something lands while Talaria is in another tab or window, and when this browser is
          closed. The in-app toast shows while you're looking at Talaria.
        {:else}
          Off in this browser. Turn on to get desktop alerts again.
        {/if}
        {#if note}<div class="mt-1">{note}</div>{/if}
      </div>
    </div>
    {#if !shell && perm === 'default'}
      <Button size="sm" onclick={turnOn}>Turn on</Button>
    {:else if !shell && perm === 'granted'}
      <Segmented options={OPTIONS} value={on ? 'on' : 'off'} onChange={(v) => set(v === 'on')} />
    {/if}
  </div>
  {#if !shell && perm === 'denied' && steps}
    <ul class="mt-2 list-disc space-y-1 pl-4 font-sans text-xs text-muted" data-notify-steps>
      {#each [...steps.browser, ...steps.os] as step (step)}<li>{step}</li>{/each}
    </ul>
    <div class="mt-1.5 font-sans text-xs text-muted">{steps.closing}</div>
  {/if}
  {#if !shell && perm === 'granted' && on}
    <div class="mt-2 font-sans text-xs text-muted" data-push-status={push.state}>
      {#if push.state === 'registered'}
        This browser is registered for closed-browser alerts.
      {:else if push.state === 'failed'}
        Registration for closed-browser alerts failed: {push.reason}. Open-tab alerts are unaffected; switching Off and On
        retries.
      {:else if push.state === 'unsupported'}
        This browser can't receive alerts while it is closed; open-tab alerts still work.
      {/if}
    </div>
  {/if}
</div>
