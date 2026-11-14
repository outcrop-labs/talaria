<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { lastDelivery, removeDevice, testDevice, testLine, thisBrowserId, useDevices, type TestOutcome } from '@/lib/notify-center'

  // YOUR DEVICES: every browser registered for push on this account
  // (`GET /api/push/devices`), the one this tab runs in marked "This browser",
  // what the last push to each did — the push service's status and its error,
  // filtered to plain text by the server — and a per-device Send test (a real
  // push through the same path a notification takes; 5 per 10 minutes, O-20)
  // and Remove. The question this answers: "I got nothing on my laptop — is it
  // even registered, and what did the push service say?"
  //
  // Both actions are in the row's menu. A browser whose last push failed also
  // shows Fix, which sends the test: it is how a person rechecks it.
  const qc = useQueryClient()
  const query = useDevices()
  const devices = $derived(query.data?.devices ?? [])
  const mine = $derived(thisBrowserId(devices))
  let results = $state<Record<string, TestOutcome | 'busy'>>({})
  let removeError = $state<string | null>(null)

  const test = async (id: string) => {
    results = { ...results, [id]: 'busy' }
    const out = await testDevice(id)
    results = { ...results, [id]: out }
    if (out.kind === 'sent' || out.kind === 'gone') void qc.invalidateQueries({ queryKey: ['push-devices'] })
  }

  const remove = async (id: string, label: string) => {
    removeError = null
    if (
      !(await confirm({
        title: `Remove ${label}`,
        message:
          'This browser stops getting push notifications with Talaria closed. Opening Talaria in it with desktop alerts on registers it again.',
        confirmLabel: 'Remove',
      }))
    )
      return
    try {
      await removeDevice(id)
    } catch (e) {
      removeError = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['push-devices'] })
  }
</script>

<!-- Rows for the Your devices card: one registered browser each. -->
<div data-notify-devices class="[&>*+*]:border-t [&>*+*]:border-line-subtle">
  {#if query.isPending}
    <div class="px-5 py-3.5"><SkeletonRows rows={2} /></div>
  {:else if query.isError && !query.data}
    <div class="px-5 py-3.5">
      <QueryError variant="compact" error={query.error} title="Could not load your devices" onRetry={() => void query.refetch()} />
    </div>
  {:else if devices.length === 0}
    <div class="px-5 py-3.5">
      <EmptyState
        variant="inline"
        title="No browser is registered for push"
        hint="Turn on desktop alerts in a browser to register it."
      />
    </div>
  {:else}
    {#each devices as d (d.id)}
      {@const last = lastDelivery(d)}
      {@const r = results[d.id]}
      {@const line = r && r !== 'busy' ? testLine(r) : null}
      <div data-device={d.id} data-this-browser={d.id === mine ? 'true' : undefined} data-device-busy={r === 'busy' ? 'true' : undefined}>
        <SettingsRow label={d.label}>
          {#snippet meta()}
            {#if d.id === mine}<Chip tone="accent">This browser</Chip>{/if}
          {/snippet}
          {#snippet hint()}
            Seen {relativeTime(d.lastSeenAt)}
            {#if last.ok !== null}
              · last push {last.ok ? '' : 'failed '}{relativeTime(d.lastAttemptAt)}{last.ok ? '' : ` (HTTP ${d.lastStatus})`}
            {/if}
            {#if d.failures > 0}· {d.failures} failed in a row{/if}
            {#if d.lastError}
              <span class="block text-danger" data-device-error>Push service said: {d.lastError}</span>
            {/if}
            {#if line}
              <span
                class="block {line.tone === 'ok' ? 'text-success' : line.tone === 'warn' ? 'text-warning' : 'text-danger'}"
                data-device-result={r && r !== 'busy' ? r.kind : undefined}
              >{line.text}</span>
            {/if}
          {/snippet}
          {#snippet status()}
            {#if last.ok === null}
              <StatusDot status="idle" />No pushes yet
            {:else}
              <StatusDot status={last.ok ? 'ok' : 'danger'} />{last.ok ? 'Reachable' : 'Unreachable'}
            {/if}
          {/snippet}
          {#if last.ok === false}
            <SettingsRowFix label={`Fix ${d.label}: send a test push`} disabled={r === 'busy'} onclick={() => void test(d.id)} />
          {/if}
          <SettingsRowMenu
            label={`Actions for ${d.label}`}
            items={[
              { label: 'Send test', disabled: r === 'busy', onSelect: () => void test(d.id) },
              'sep',
              { label: 'Remove browser', danger: true, onSelect: () => void remove(d.id, d.label) },
            ]}
          />
        </SettingsRow>
      </div>
    {/each}
  {/if}
  {#if removeError}<p class="px-5 py-3 font-sans text-[13px] leading-5 text-danger" role="alert">{removeError}</p>{/if}
</div>
