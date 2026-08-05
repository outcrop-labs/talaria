<script lang="ts">
  import { fade, QUICK } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'

  // The one activity/audit row: "actor · detail" + relative time, with an
  // optional quiet context line underneath. Shared by Home's activity feeds
  // (ActivityList, Fleet pulse) and the Observability overview's audit trail.
  // Rows carry their own hairline separator (spec §8 "tables/lists: hairline
  // row separators") so every consumer list is separated without needing
  // divide-y on the container.
  let {
    actor,
    detail,
    at,
    context,
    onClick,
  }: {
    actor: string
    detail: string
    at: string
    /** Second line under the row; omit to render the single-line (audit) form. */
    context?: string
    onClick: () => void
  } = $props()
</script>

<li in:fade={{ duration: 150 }} out:fade={QUICK} class="border-b border-line last:border-b-0">
  <button
    type="button"
    onclick={onClick}
    class="w-full rounded-md px-1.5 py-1 text-left transition-colors hover:bg-card2"
  >
    <div class="flex items-baseline gap-2">
      <span class="min-w-0 flex-1 truncate font-sans text-xs text-fg">
        <span class="font-medium">{actor}</span> · {detail}
      </span>
      <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(at)}</span>
    </div>
    {#if context !== undefined}
      <div class="truncate font-sans text-[11px] text-muted">{context}</div>
    {/if}
  </button>
</li>
