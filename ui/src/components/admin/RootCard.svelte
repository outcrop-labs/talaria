<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { RootHealth } from '@/lib/secrets'
  import RowStatus from './RowStatus.svelte'

  // ── The root ─────────────────────────────────────────────────────────────────
  // Deliberately the first thing on the page, and deliberately not editable. It
  // lives in the process environment, so the app can only report it — and saying
  // it exists at all is most of the value, because nothing else in the product
  // mentions it until the day it stops matching.
  let { root }: { root: RootHealth } = $props()

  const tone = $derived(
    root.state === 'ok' ? 'ok' : root.state === 'fallback' ? 'warn' : ('danger' as const),
  )
  const word = $derived(
    root.state === 'ok'
      ? 'Healthy'
      : root.state === 'fallback'
        ? 'Borrowed'
        : root.state === 'absent'
          ? 'Not set'
          : 'Does not match',
  )
  const hint = $derived(
    root.state === 'ok'
      ? 'Back it up with the database. A restore without it cannot read any secret.'
      : root.state === 'fallback'
        ? 'AUTH_SECRET is doing this job. Set TALARIA_SECRET_KEY to the same value to pin it.'
        : root.state === 'absent'
          ? 'Nothing new can be stored until TALARIA_SECRET_KEY is set and the server restarts.'
          : (root.failure ?? 'The root key this server has is not the one these secrets were stored with.'),
  )
  const info = $derived(
    root.state === 'fallback'
      ? 'TALARIA_SECRET_KEY is not set, so AUTH_SECRET stands in for it. Changing AUTH_SECRET would then make every secret below unrecoverable.'
      : 'Every secret below is locked with a key that this value protects. It lives where the server is configured, so it can only be reported here, not changed.',
  )
  const lostVersions = $derived(root.storedVersions - root.loadedVersions.length)
</script>

<SettingsRow label="Root key" {hint} {info}>
  {#snippet meta()}
    {#if root.state === 'ok'}<code class="shrink-0 font-mono text-xs text-muted">{root.name}</code>{/if}
    {#if lostVersions > 0}
      <Chip tone="danger">{lostVersions} {lostVersions === 1 ? 'version' : 'versions'} unreadable</Chip>
    {/if}
  {/snippet}
  <span class="font-sans text-[13px] leading-5 tabular-nums text-muted">
    {root.activeVersion ? `Key version ${root.activeVersion}` : 'No key'}
  </span>
  <RowStatus status={tone}>{word}</RowStatus>
</SettingsRow>
