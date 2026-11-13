<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
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
    useNotifySettings,
    usePresenceSnapshot,
  } from '@/lib/notify-center'
  import { notifyStatus } from '@/lib/notify-status.svelte'
  import { p } from '@/router'
  import BrowserNotifications from './BrowserNotifications.svelte'
  import NotifyDevices from './NotifyDevices.svelte'

  // STATUS AND TROUBLESHOOTING (D9, EB-03): the one place that answers "why
  // didn't I get that?" without a support ticket. Four sections, each a fact
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
  //
  // What Do Not Disturb is doing now is said in its own card, above this one.
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

</script>

<div class="space-y-10" data-notify-status-panel>
  <div data-status-section="browser">
    <SettingsSection title="This browser" description="Start here when something did not arrive.">
      <SettingsGroup>
        <BrowserNotifications serverKnows={features.has('devices') ? serverKnows : null} />
      </SettingsGroup>
    </SettingsSection>
  </div>

  {#if features.has('devices')}
    <div data-status-section="devices">
      <SettingsSection title="Your devices" description="Browsers that get a push with Talaria closed.">
        <SettingsGroup><NotifyDevices /></SettingsGroup>
      </SettingsSection>
    </div>
  {/if}

  <div data-status-section="live">
    <SettingsSection title="Live connection">
      <SettingsGroup>
        <dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 py-4 font-sans text-[13px] leading-5">
          <dt class="text-muted">Event stream</dt>
          <dd class="flex flex-wrap items-center gap-2 text-fg" data-live="stream" data-state={streamReported ? status.stream.state : 'unknown'}>
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
            <dt class="text-muted">This tab</dt>
            <dd class="text-fg" data-live="role" data-state={status.hubRole}>{ROLE_LABEL[status.hubRole]}</dd>
          {/if}
          {#if features.has('presence-store')}
            <dt class="text-muted">Alerts go to</dt>
            <dd class="flex items-center gap-2 text-fg" data-live="target" data-state={target.target}>
              {#if presence.isPending}
                <StatusDot status="idle" />Checking
              {:else if presence.isError && !presence.data}
                <StatusDot status="warn" />Could not read where you are active
              {:else}
                <StatusDot status={target.target === 'none' ? 'idle' : 'ok'} />{ALERT_TARGET_LABEL[target.target]}
              {/if}
            </dd>
            {#if presence.data}
              <dt class="text-muted">Clients reporting</dt>
              <dd class="text-fg" data-live="clients">
                {presence.data.clients.length === 0
                  ? 'None in the last two minutes'
                  : presence.data.clients.map((c) => `${c.kind === 'desktop' ? 'Talaria Desktop' : 'a browser tab'} (${c.state})`).join(', ')}
              </dd>
            {/if}
          {/if}
        </dl>
      </SettingsGroup>
      <!-- Each sentence is said only where this instance does it: the client
           side by the web runtime (it reports here), the push side by the
           delivery engine (`presence-gated-push`). -->
      <p class="mt-2 max-w-[72ch] font-sans text-[13px] leading-5 text-muted" data-live-explain={gatedPush ? 'gated' : 'immediate'}>
        {#if streamReported}The device you are looking at shows the banner, and your other devices stay quiet.{/if}
        {#if gatedPush}
          Push waits a minute while you are using one of your devices.
        {:else}
          This instance pushes every alert to every registered browser at once.
        {/if}
      </p>
    </SettingsSection>
  </div>

  {#if shell}
    <div data-status-section="desktop">
      <SettingsSection title="Talaria Desktop">
        <SettingsGroup>
          <SettingsRow label="Notification service">
            {#snippet hint()}
              {#if shellLine}
                <span class="flex items-center gap-2" data-shell-status={shellNote?.permission}>
                  <StatusDot status={shellLine.status} />{shellLine.text}
                </span>
                {#if shellNote?.lastError}<span class="block text-danger">Last error: {shellNote.lastError}</span>{/if}
              {:else}
                This version of the desktop app does not report its notification service.
              {/if}
              {#if shellMsg}
                <span class="block {shellMsg.ok ? 'text-muted' : 'text-danger'}" data-shell-test-result>{shellMsg.text}</span>
              {/if}
            {/snippet}
            <a class="font-sans text-[13px] text-accent underline-offset-2 hover:underline" href={p('/settings')}>Desktop settings</a>
            {#if shellNote}
              <Button size="sm" variant="outline" disabled={shellBusy} onclick={() => void shellTest()} data-shell-test>Send test</Button>
            {/if}
          </SettingsRow>
        </SettingsGroup>
      </SettingsSection>
    </div>
  {/if}
</div>
