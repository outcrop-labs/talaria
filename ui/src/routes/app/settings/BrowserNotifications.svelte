<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
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
      // devices list (and "On the server" below) reads it afresh rather than
      // keeping a read that raced it.
      if (s.state === 'registered') void qc.invalidateQueries({ queryKey: ['push-devices'] })
    }),
  )

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

<!-- Rows for the This browser card. -->
<div data-notify-settings={shell ? 'shell' : perm} class="[&>*+*]:border-t [&>*+*]:border-line-subtle">
  <SettingsRow label="Desktop alerts">
    {#snippet hint()}
      {#if shell}
        Desktop app notifications are set in Settings, under Desktop.
      {:else if perm === 'insecure'}
        {insecureExplanation(location.origin)}
      {:else if perm === 'unsupported'}
        This browser does not support desktop alerts.
      {:else if perm === 'denied'}
        Blocked in {browser?.name ?? 'this browser'}.
      {:else if perm === 'default' || !on}
        Get an alert when someone needs you while Talaria is in another tab or closed.
      {:else}
        Alerts pop up while Talaria is in another tab or closed.
      {/if}
      {#if note}<span class="block">{note}</span>{/if}
    {/snippet}
    {#if !shell && perm === 'default'}
      <Button size="sm" variant="outline" onclick={turnOn}>Turn on</Button>
    {:else if !shell && perm === 'granted'}
      <Toggle checked={on} onChange={(v) => set(v)} title="Desktop alerts in this browser" />
    {/if}
  </SettingsRow>
  {#if !shell && perm === 'denied' && steps}
    <div class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">
      <ul class="list-disc space-y-1 pl-4" data-notify-steps>
        {#each [...steps.browser, ...steps.os] as step (step)}<li>{step}</li>{/each}
      </ul>
      <div class="mt-2">{steps.closing}</div>
    </div>
  {/if}

  {#if !shell}
    <div>
      <dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 py-4 font-sans text-[13px] leading-5" data-browser-checks>
        <dt class="text-muted">Permission</dt>
        <dd class="flex items-center gap-2 text-fg" data-check="permission" data-state={perm}>
          <StatusDot status={permLine.status} />{permLine.text}
        </dd>
        <dt class="text-muted">Service worker</dt>
        <dd class="flex items-center gap-2 text-fg" data-check="worker" data-state={worker}>
          <StatusDot status={worker === 'active' ? 'ok' : worker === 'starting' ? 'warn' : 'idle'} />
          {worker === 'active' ? 'Running' : worker === 'starting' ? 'Starting' : worker === 'unsupported' ? 'Not available here' : 'Not registered'}
        </dd>
        <dt class="text-muted">Push subscription</dt>
        <dd class="flex items-center gap-2 text-fg" data-check="subscription" data-state={subscribedTo ? 'yes' : 'no'}>
          <StatusDot status={subscribedTo ? 'ok' : 'idle'} />
          {subscribedTo ? `Subscribed through ${subscribedTo}` : 'None in this browser'}
        </dd>
        <dt class="text-muted">On the server</dt>
        <dd class="flex items-center gap-2 text-fg" data-check="server" data-state={serverKnows === null ? 'unknown' : serverKnows ? 'yes' : 'no'}>
          <StatusDot status={serverKnows ? 'ok' : serverKnows === null ? 'idle' : 'warn'} />
          {serverKnows === null
            ? 'Checking'
            : serverKnows
              ? 'Talaria has this browser on file for push'
              : subscribedTo
                ? 'Not on file. Switching off and on registers it again.'
                : 'Not on file'}
        </dd>
        {#if perm === 'granted' && on}
          <dt class="text-muted">With tabs closed</dt>
          <dd class="text-fg" data-push-status={push.state}>
            {#if push.state === 'registered'}
              Registered. {#if browser}{closedTabsNote(browser)}{/if}
            {:else if push.state === 'failed'}
              Registration failed: {push.reason}. Open-tab alerts still work. Switching off and on retries.
            {:else if push.state === 'unsupported'}
              Not supported by this browser. Open-tab alerts still work.
            {/if}
          </dd>
        {/if}
      </dl>
      {#if perm === 'granted' && on}
        <div class="flex flex-wrap items-center gap-3 px-5 pb-4">
          <Button size="sm" variant="outline" onclick={() => void testHere()} data-browser-test>Send test alert</Button>
          {#if testMsg}
            <span class="font-sans text-[13px] leading-5 {testMsg.ok ? 'text-muted' : 'text-danger'}" data-browser-test-result>{testMsg.text}</span>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>
