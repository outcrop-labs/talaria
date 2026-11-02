<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { relativeTime } from '@/lib/fleet'
  import {
    inDesktopShell,
    shellNotificationStatus,
    shellSendTestNotification,
    type ShellNotificationStatus,
  } from '@/lib/desktop-shell'
  import {
    ALERT_TARGET_LABEL,
    alertTarget,
    featureSet,
    thisBrowserId,
    useDevices,
    useNotifyPatch,
    useNotifySettings,
    usePresenceSnapshot,
  } from '@/lib/notify-center'
  import { dndView, safeZone, scheduleSummary } from '@/lib/notify-dnd'
  import { notifyStatus } from '@/lib/notify-status.svelte'
  import { p } from '@/router'
  import BrowserNotifications from './BrowserNotifications.svelte'
  import NotifyDevices from './NotifyDevices.svelte'

  // STATUS AND TROUBLESHOOTING (D9, EB-03): the one place that answers "why
  // didn't I get that?" without a support ticket. Five sections, each a fact
  // the person can check, not a guess:
  //
  //   1. This browser — permission (live), worker, subscription, whether the
  //      server has it on file, and a local test banner.
  //   2. Your devices — every browser registered for push, with what the push
  //      service last said, a per-device test and Remove.
  //   3. Live connection — the event stream, this tab's role, and which of
  //      your devices alerts go to right now (the presence winner, I-17).
  //   4. Talaria Desktop — in the desktop app, its own notification service
  //      and test, through the shell bridge.
  //   5. Pause — what Do Not Disturb is doing now.
  //
  // Each section gates on what the api serves (`features`) and on where it
  // runs (a browser or the shell), and says so rather than showing a dead row.
  const query = useNotifySettings()
  const features = $derived(featureSet(query.data))
  const shell = inDesktopShell()
  const devices = useDevices(() => features.has('devices'))
  const serverKnows = $derived(devices.data ? thisBrowserId(devices.data.devices) !== null : null)
  const presence = usePresenceSnapshot(() => features.has('presence-store'))
  const status = $derived(notifyStatus())
  const target = $derived(alertTarget(presence.data, status.presence.self))
  // Nothing has reported the stream: an app whose web runtime predates the
  // status hooks (they are W2-R's to call).
  const streamReported = $derived(status.stream.since !== null || status.stream.lastEventAt !== null || status.hubRole !== 'unknown')
  // The delivery engine (W2-E) holds push while a device is in use; before it,
  // every alert is pushed at once.
  const gatedPush = $derived(features.has('presence-gated-push'))

  const STREAM_LABEL: Record<string, string> = {
    open: 'Connected',
    connecting: 'Connecting',
    reconnecting: 'Reconnecting',
    closed: 'Closed',
  }
  const ROLE_LABEL: Record<string, string> = {
    leader: 'This tab holds the connection and posts the banners for this browser.',
    follower: 'Another Talaria tab in this browser holds the connection; this one follows it.',
    solo: 'This tab has its own connection.',
    unknown: '',
  }

  // 4. The shell's notification service, read when the panel mounts and when
  // the window comes back into view (a change in System Settings).
  let shellNote = $state<ShellNotificationStatus | null>(null)
  let shellMsg = $state<{ text: string; ok: boolean } | null>(null)
  let shellBusy = $state(false)
  const readShell = async () => {
    try {
      shellNote = await shellNotificationStatus()
    } catch (e) {
      shellMsg = { text: e instanceof Error ? e.message : String(e), ok: false }
    }
  }
  $effect(() => {
    if (!shell) return
    void readShell()
    const again = () => {
      if (!document.hidden) void readShell()
    }
    document.addEventListener('visibilitychange', again)
    return () => document.removeEventListener('visibilitychange', again)
  })
  const shellLine = $derived.by((): { text: string; status: 'ok' | 'warn' | 'danger' } | null => {
    const s = shellNote
    if (!s) return null
    const by = s.server ? `${s.server.name} ${s.server.version}` : s.backend === 'un' ? 'macOS Notification Center' : s.backend
    switch (s.permission) {
      case 'granted':
        return { text: `On, shown by ${by}`, status: 'ok' }
      case 'not-determined':
        return { text: 'Not allowed yet', status: 'warn' }
      case 'denied':
        return { text: 'Blocked in System Settings', status: 'danger' }
      default:
        return { text: `Notification service unavailable: ${(s.detail ?? 'no reason given').replace(/^unavailable:\s*/, '')}`, status: 'danger' }
    }
  })
  const shellTest = async () => {
    shellBusy = true
    shellMsg = null
    try {
      const r = await shellSendTestNotification()
      if (r) {
        shellNote = r.status
        shellMsg = r.ok ? { text: 'Sent. A banner should be on screen now.', ok: true } : { text: `Not shown: ${r.error ?? 'the notification service refused it'}`, ok: false }
      }
    } catch (e) {
      shellMsg = { text: e instanceof Error ? e.message : String(e), ok: false }
    } finally {
      shellBusy = false
    }
  }

  // 5. Pause.
  const patch = useNotifyPatch()
  const dnd = $derived(query.data?.dnd)
  const dndNow = $derived(dndView(dnd, Date.now(), safeZone(dnd?.timeZone)))
  let resumeMsg = $state<string | null>(null)
  const resume = async () => {
    resumeMsg = null
    const r = await patch({ dnd: { pausedUntil: null } })
    if ('error' in r) resumeMsg = r.error
  }

  const section = 'mt-5 border-t border-line-subtle pt-4 first:mt-0 first:border-t-0 first:pt-0'
  const label = 'mb-2 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted'
</script>

<Panel as="section" data-notify-status-panel>
  <SectionHeader
    class="mb-4"
    title="Status and troubleshooting"
    info="Everything that decides whether a notification reaches you, checked live. Start here when something did not arrive."
  />

  <div class={section} data-status-section="browser">
    <div class={label}>This browser</div>
    <BrowserNotifications serverKnows={features.has('devices') ? serverKnows : null} />
  </div>

  {#if features.has('devices')}
    <div class={section} data-status-section="devices">
      <div class={label}>Your devices</div>
      <NotifyDevices />
    </div>
  {/if}

  <div class={section} data-status-section="live">
    <div class={label}>Live connection</div>
    <dl class="grid grid-cols-[9rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 font-sans text-xs">
      <dt class="text-ink-dim">Event stream</dt>
      <dd class="flex items-center gap-1.5 text-fg" data-live="stream" data-state={streamReported ? status.stream.state : 'unknown'}>
        {#if streamReported}
          <StatusDot status={status.stream.state === 'open' ? 'ok' : status.stream.state === 'closed' ? 'danger' : 'warn'} />
          {STREAM_LABEL[status.stream.state] ?? status.stream.state}
          {#if status.stream.since}<span class="text-muted">since {relativeTime(new Date(status.stream.since).toISOString())}</span>{/if}
          {#if status.stream.lastEventAt}<span class="text-muted">· last event {relativeTime(new Date(status.stream.lastEventAt).toISOString())}</span>{/if}
        {:else}
          <StatusDot status="idle" />Not reported by this version of the app
        {/if}
      </dd>
      {#if ROLE_LABEL[status.hubRole]}
        <dt class="text-ink-dim">This tab</dt>
        <dd class="text-fg" data-live="role" data-state={status.hubRole}>{ROLE_LABEL[status.hubRole]}</dd>
      {/if}
      {#if features.has('presence-store')}
        <dt class="text-ink-dim">Alerts go to</dt>
        <dd class="flex items-center gap-1.5 text-fg" data-live="target" data-state={target.target}>
          {#if presence.isPending}
            <StatusDot status="idle" />Checking
          {:else if presence.isError && !presence.data}
            <StatusDot status="warn" />Could not read where you are active
          {:else}
            <StatusDot status={target.target === 'none' ? 'idle' : 'ok'} />{ALERT_TARGET_LABEL[target.target]}
          {/if}
        </dd>
        {#if presence.data}
          <dt class="text-ink-dim">Clients reporting</dt>
          <dd class="text-muted" data-live="clients">
            {presence.data.clients.length === 0
              ? 'None in the last two minutes'
              : presence.data.clients.map((c) => `${c.kind === 'desktop' ? 'Talaria Desktop' : 'a browser tab'} (${c.state})`).join(', ')}
          </dd>
        {/if}
      {/if}
    </dl>
    <!-- Each sentence is said only where this instance does it: the client
         side by the web runtime (it reports here), the push side by the
         delivery engine (`presence-gated-push`). -->
    <p class="mt-2 max-w-xl font-sans text-xs text-muted" data-live-explain={gatedPush ? 'gated' : 'immediate'}>
      {#if streamReported}The device you are looking at shows the banner, and your other devices stay quiet.{/if}
      {#if gatedPush}
        Push waits a minute while you are using one of your devices, and goes to every registered browser when you are
        not.
      {:else}
        This instance pushes every alert that has Push on to every registered browser at once, whichever device you are
        using.
      {/if}
    </p>
  </div>

  {#if shell}
    <div class={section} data-status-section="desktop">
      <div class={label}>Talaria Desktop</div>
      {#if shellLine}
        <p class="flex items-center gap-1.5 font-sans text-sm text-fg" data-shell-status={shellNote?.permission}>
          <StatusDot status={shellLine.status} />{shellLine.text}
        </p>
        {#if shellNote?.lastError}<p class="mt-1 font-sans text-xs text-danger">Last error: {shellNote.lastError}</p>{/if}
      {:else}
        <p class="font-sans text-sm text-muted">This version of the desktop app does not report its notification service.</p>
      {/if}
      <div class="mt-2 flex flex-wrap items-center gap-3">
        {#if shellNote}
          <Button size="xs" variant="outline" disabled={shellBusy} onclick={() => void shellTest()} data-shell-test>Send test notification</Button>
        {/if}
        <a class="font-sans text-xs text-accent underline-offset-2 hover:underline" href={p('/settings')}>Settings → Desktop</a>
        {#if shellMsg}
          <span class="font-sans text-xs {shellMsg.ok ? 'text-muted' : 'text-danger'}" data-shell-test-result>{shellMsg.text}</span>
        {/if}
      </div>
    </div>
  {/if}

  <div class={section} data-status-section="pause">
    <div class={label}>Pause</div>
    <p class="flex flex-wrap items-center gap-2 font-sans text-sm text-fg" data-status-pause={dndNow.active ? dndNow.reason : 'off'}>
      {#if dndNow.active}
        <StatusDot status="warn" />{dndNow.label}
        {#if dndNow.reason === 'paused'}
          <Button size="xs" variant="outline" onclick={() => void resume()}>Resume</Button>
        {/if}
      {:else}
        <StatusDot status="ok" />Not paused
        {#if dnd?.schedule?.enabled}<span class="text-xs text-muted">· working hours {scheduleSummary(dnd.schedule)}</span>{/if}
      {/if}
    </p>
    {#if dndNow.active && dnd?.allowNeedsYou !== false}
      <p class="mt-1 font-sans text-xs text-muted">Needs you still comes through.</p>
    {/if}
    {#if resumeMsg}<p class="mt-1 font-sans text-xs text-danger" role="alert">{resumeMsg}</p>{/if}
  </div>
</Panel>
