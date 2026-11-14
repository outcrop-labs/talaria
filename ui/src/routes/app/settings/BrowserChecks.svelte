<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { closedTabsNote } from '@/lib/notify-help'
  import type { BrowserAlerts } from './browser-alerts.svelte'

  // The checklist a support conversation walks through: the permission, the
  // service worker, the push subscription, and whether the server has this
  // browser on file.
  let {
    alerts,
    serverKnows = null,
  }: {
    alerts: BrowserAlerts
    /** Whether the server has a device from this session (the devices list);
     *  null while unknown. */
    serverKnows?: boolean | null
  } = $props()

  const permLine = $derived.by((): { text: string; status: 'ok' | 'warn' | 'danger' | 'idle' } => {
    switch (alerts.perm) {
      case 'granted':
        return { text: alerts.on ? 'Allowed, and on' : 'Allowed, but switched off here', status: alerts.on ? 'ok' : 'warn' }
      case 'default':
        return { text: 'Not asked yet', status: 'warn' }
      case 'denied':
        return { text: `Blocked in ${alerts.browser?.name ?? 'this browser'}`, status: 'danger' }
      case 'insecure':
        return { text: 'Unavailable on plain http', status: 'idle' }
      default:
        return { text: 'Not supported by this browser', status: 'idle' }
    }
  })
</script>

<dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 font-sans text-[13px] leading-5" data-browser-checks>
  <dt class="text-muted">Permission</dt>
  <dd class="flex items-center gap-2 text-fg" data-check="permission" data-state={alerts.perm}>
    <StatusDot status={permLine.status} />{permLine.text}
  </dd>
  <dt class="text-muted">Service worker</dt>
  <dd class="flex items-center gap-2 text-fg" data-check="worker" data-state={alerts.worker}>
    <StatusDot status={alerts.worker === 'active' ? 'ok' : alerts.worker === 'starting' ? 'warn' : 'idle'} />
    {alerts.worker === 'active'
      ? 'Running'
      : alerts.worker === 'starting'
        ? 'Starting'
        : alerts.worker === 'unsupported'
          ? 'Not available here'
          : 'Not registered'}
  </dd>
  <dt class="text-muted">Push subscription</dt>
  <dd class="flex items-center gap-2 text-fg" data-check="subscription" data-state={alerts.subscribedTo ? 'yes' : 'no'}>
    <StatusDot status={alerts.subscribedTo ? 'ok' : 'idle'} />
    {alerts.subscribedTo ? `Subscribed through ${alerts.subscribedTo}` : 'None in this browser'}
  </dd>
  <dt class="text-muted">On the server</dt>
  <dd class="flex items-center gap-2 text-fg" data-check="server" data-state={serverKnows === null ? 'unknown' : serverKnows ? 'yes' : 'no'}>
    <StatusDot status={serverKnows ? 'ok' : serverKnows === null ? 'idle' : 'warn'} />
    {serverKnows === null
      ? 'Checking'
      : serverKnows
        ? 'Talaria has this browser on file for push'
        : alerts.subscribedTo
          ? 'Not on file. Switching off and on registers it again.'
          : 'Not on file'}
  </dd>
  {#if alerts.perm === 'granted' && alerts.on}
    <dt class="text-muted">With tabs closed</dt>
    <dd class="text-fg" data-push-status={alerts.push.state}>
      {#if alerts.push.state === 'registered'}
        Registered. {#if alerts.browser}{closedTabsNote(alerts.browser)}{/if}
      {:else if alerts.push.state === 'failed'}
        Registration failed: {alerts.push.reason}. Open-tab alerts still work. Switching off and on retries.
      {:else if alerts.push.state === 'unsupported'}
        Not supported by this browser. Open-tab alerts still work.
      {/if}
    </dd>
  {/if}
</dl>
