<script lang="ts">
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import { relativeTime } from '@/lib/fleet'
  import type { SecretRow } from '@/lib/secrets'
  import StateChip from './StateChip.svelte'

  // ── One row ──────────────────────────────────────────────────────────────────
  let { row, onClear, busy }: { row: SecretRow; onClear: (row: SecretRow) => void; busy: boolean } = $props()
</script>

<li class="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-3">
  <div class="min-w-0 flex-1">
    <div class="flex flex-wrap items-center gap-2">
      <span class="truncate text-sm text-fg">{row.label}</span>
      {#if row.owner}<span class="truncate font-mono text-[10px] text-ink-dim">{row.owner}</span>{/if}
      <StateChip {row} />
    </div>
    <!-- What breaks without it. The reason an operator can decide anything
         here without going and reading the code. -->
    <p class="mt-0.5 text-xs leading-relaxed text-muted">{row.unlocks}</p>
  </div>
  <div class="flex shrink-0 items-center gap-3 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
    {#if row.setAt}<span title={new Date(row.setAt).toLocaleString()}>set {relativeTime(row.setAt)}</span>{/if}
    {#if row.expiresAt}<span title={new Date(row.expiresAt).toLocaleString()}>expires {relativeTime(row.expiresAt)}</span>{/if}
    {#if row.lastUsedAt}<span title={new Date(row.lastUsedAt).toLocaleString()}>used {relativeTime(row.lastUsedAt)}</span>{/if}
    {#if row.href}
      <a href={row.href} class="underline-offset-2 transition-colors hover:text-fg hover:underline">
        {row.state === 'missing' ? 'Set up' : 'Replace'}
      </a>
    {:else}
      <!-- No deep link because the value belongs to a person: it is replaced
           in their own Settings, by them. Say so rather than offering a link
           that would land an admin on their own page. -->
      <span title={`Replaced by ${row.owner ?? 'the owner'} in ${row.surface}`}>{row.surface}</span>
    {/if}
    {#if row.clearable}
      <DangerLink onClick={() => onClear(row)} disabled={busy}>
        Clear
      </DangerLink>
    {/if}
  </div>
</li>
