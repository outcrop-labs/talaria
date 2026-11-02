<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
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

<div data-notify-devices>
  {#if query.isPending}
    <SkeletonRows rows={2} />
  {:else if query.isError && !query.data}
    <QueryError variant="compact" error={query.error} title="Could not load your devices" onRetry={() => void query.refetch()} />
  {:else if devices.length === 0}
    <EmptyState
      variant="inline"
      title="No browser is registered for push"
      hint="Turn on desktop alerts in a browser to register it."
    />
  {:else}
    <ul class="divide-y divide-line-subtle">
      {#each devices as d (d.id)}
        {@const last = lastDelivery(d)}
        {@const r = results[d.id]}
        {@const line = r && r !== 'busy' ? testLine(r) : null}
        <li class="py-3" data-device={d.id} data-this-browser={d.id === mine ? 'true' : undefined}>
          <div class="flex flex-wrap items-center gap-2">
            <span class="text-sm text-fg">{d.label}</span>
            {#if d.id === mine}<Chip tone="accent">This browser</Chip>{/if}
            <span class="ml-auto flex items-center gap-3">
              <Button size="xs" variant="outline" disabled={r === 'busy'} onclick={() => void test(d.id)} data-device-test>
                {r === 'busy' ? 'Sending' : 'Send test'}
              </Button>
              <DangerLink onClick={() => void remove(d.id, d.label)}>Remove</DangerLink>
            </span>
          </div>
          <div class="mt-1 font-sans text-xs text-muted">
            Registered {relativeTime(d.createdAt)} · seen {relativeTime(d.lastSeenAt)}
            {#if last.ok !== null}
              · <span class={last.ok ? 'text-success' : 'text-danger'}>{last.text}</span> {relativeTime(d.lastAttemptAt)}
            {:else}
              · {last.text}
            {/if}
            {#if d.failures > 0}· {d.failures} failed in a row{/if}
          </div>
          {#if d.lastError}
            <div class="mt-0.5 font-sans text-xs text-danger" data-device-error>Push service said: {d.lastError}</div>
          {/if}
          {#if line}
            <div
              class="mt-1 font-sans text-xs {line.tone === 'ok' ? 'text-success' : line.tone === 'warn' ? 'text-warning' : 'text-danger'}"
              data-device-result={r && r !== 'busy' ? r.kind : undefined}
            >
              {line.text}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if removeError}<p class="mt-2 font-sans text-xs text-danger" role="alert">{removeError}</p>{/if}
</div>
