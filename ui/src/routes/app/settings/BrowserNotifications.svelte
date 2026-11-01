<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
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
  import { blockedSteps, closedTabsNote, detectBrowser, insecureExplanation } from '@/lib/notify-help'
  import { turnOnResult } from '@/lib/notify-prompt'

  // THIS BROWSER — the first section of the status panel. Desktop alerts: an
  // OS banner when Talaria is not what the person is looking at. In a browser
  // that is the Notification API plus Web Push for a closed browser, and this
  // row says which of the five states this browser is in — LIVE (a grant or a
  // block made in the browser's own site settings shows up here without a
  // reload) — with the one control that state allows, never a dead one:
  //
  //   default     → Turn on (the native prompt runs inside the click)
  //   granted     → On / Off (this browser's own switch) and whether this
  //                 browser is registered for alerts with every Talaria tab
  //                 closed — which need the browser itself running (on
  //                 Linux and Windows, its last window quits it)
  //   denied      → where to unblock it: the browser's switch and the OS's
  //   insecure    → why a plain-http dev stack cannot alert, no control
  //   unsupported → that it cannot
  //
  // Below it, the checklist a support conversation walks through: the
  // permission, the service worker, the push subscription, whether the server
  // has this browser on file, and a local test banner (no server involved).
  //
  // In the desktop shell the webview's permission reads denied and there is
  // no site-settings page; the shell posts natively and owns its own switch
  // (Settings → Desktop).
  let {
    serverKnows = null,
  }: {
    /** Whether the server has a device from this session (the devices list);
     *  null while unknown. */
    serverKnows?: boolean | null
  } = $props()

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

  // The worker and the subscription, as the browser reports them now —
  // re-read whenever the permission or the push status moves.
  type Worker = 'active' | 'starting' | 'none' | 'unsupported'
  let worker = $state<Worker>('none')
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
      ? { text: 'Sent to the browser. If no banner appeared, check the steps for your system below, and Do Not Disturb.', ok: true }
      : { text: 'The browser refused the banner. Check that desktop alerts are allowed for this site.', ok: false }
  }

  const permLine = $derived.by((): { text: string; status: 'ok' | 'warn' | 'danger' | 'idle' } => {
    switch (perm) {
      case 'granted':
        return { text: on ? 'Allowed, and on' : 'Allowed, but switched off here', status: on ? 'ok' : 'warn' }
      case 'default':
        return { text: 'Not asked yet', status: 'warn' }
      case 'denied':
        return { text: `Blocked in ${browser?.name ?? 'this browser'}`, status: 'danger' }
      case 'insecure':
        return { text: 'Unavailable on plain http', status: 'idle' }
      default:
        return { text: 'Not supported by this browser', status: 'idle' }
    }
  })
</script>

<div data-notify-settings={shell ? 'shell' : perm}>
  <div class="flex items-center gap-4">
    <div class="min-w-0 flex-1">
      <div class="text-sm text-fg">Desktop alerts in this browser</div>
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
          another tab or window, or every Talaria tab is closed.
        {:else if on}
          On. An alert pops up when something lands while Talaria is in another tab or window, or every Talaria tab is
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
        This browser is registered for alerts with Talaria's tabs closed.
        {#if browser}{closedTabsNote(browser)}{/if}
      {:else if push.state === 'failed'}
        Registration for alerts with Talaria's tabs closed failed: {push.reason}. Open-tab alerts are unaffected;
        switching Off and On retries.
      {:else if push.state === 'unsupported'}
        This browser can't receive alerts with Talaria's tabs closed; open-tab alerts still work.
      {/if}
    </div>
  {/if}

  {#if !shell}
    <dl class="mt-3 grid grid-cols-[9rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 font-sans text-xs" data-browser-checks>
      <dt class="text-ink-dim">Permission</dt>
      <dd class="flex items-center gap-1.5 text-fg" data-check="permission" data-state={perm}>
        <StatusDot status={permLine.status} />{permLine.text}
      </dd>
      <dt class="text-ink-dim">Service worker</dt>
      <dd class="flex items-center gap-1.5 text-fg" data-check="worker" data-state={worker}>
        <StatusDot status={worker === 'active' ? 'ok' : worker === 'starting' ? 'warn' : 'idle'} />
        {worker === 'active' ? 'Running' : worker === 'starting' ? 'Starting' : worker === 'unsupported' ? 'Not available here' : 'Not registered'}
      </dd>
      <dt class="text-ink-dim">Push subscription</dt>
      <dd class="flex items-center gap-1.5 text-fg" data-check="subscription" data-state={subscribedTo ? 'yes' : 'no'}>
        <StatusDot status={subscribedTo ? 'ok' : 'idle'} />
        {subscribedTo ? `Subscribed through ${subscribedTo}` : 'None in this browser'}
      </dd>
      <dt class="text-ink-dim">On the server</dt>
      <dd class="flex items-center gap-1.5 text-fg" data-check="server" data-state={serverKnows === null ? 'unknown' : serverKnows ? 'yes' : 'no'}>
        <StatusDot status={serverKnows ? 'ok' : serverKnows === null ? 'idle' : 'warn'} />
        {serverKnows === null
          ? 'Checking'
          : serverKnows
            ? 'Talaria has this browser on file for push'
            : subscribedTo
              ? 'Not on file. Switching Off and On registers it again.'
              : 'Not on file'}
      </dd>
    </dl>
    {#if perm === 'granted' && on}
      <div class="mt-3 flex flex-wrap items-center gap-3">
        <Button size="xs" variant="outline" onclick={() => void testHere()} data-browser-test>Test this browser</Button>
        {#if testMsg}
          <span class="font-sans text-xs {testMsg.ok ? 'text-muted' : 'text-danger'}" data-browser-test-result>{testMsg.text}</span>
        {/if}
      </div>
    {/if}
  {/if}
</div>
