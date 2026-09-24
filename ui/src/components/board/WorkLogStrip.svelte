<script lang="ts">
  import { Eye } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useWorkSessionHistory } from '@/lib/work-session.svelte'

  // The ticket's WORK LOG: every session it has seen, newest first — the
  // ticker covers the live one, this is the record work leaves behind.
  // One compact row per run: who worked it, how the run ended (or that it
  // is still going), when, and the eye that opens the run's detail modal
  // (turns and resources read the retained record). Renders nothing while
  // the history read is empty or in flight, so an unworked ticket stays
  // quiet. A failed read is not an empty log.
  let { taskId, onView }: { taskId: string; onView: (runId: string) => void } = $props()

  const history = useWorkSessionHistory(() => taskId)
  const sessions = $derived(history.data?.sessions ?? [])

  // The state badge's tone: live carries the accent (the work vocabulary),
  // the bad endings carry danger, everything settled is muted.
  const STATE_TONE: Record<string, string> = {
    live: 'border-accent/60 text-accent',
    running: 'border-accent/60 text-accent',
    finished: 'border-line-subtle text-muted',
    completed: 'border-line-subtle text-muted',
    cancelled: 'border-danger/40 text-danger',
    failed: 'border-danger/40 text-danger',
  }
  const stateTone = (state: string) => STATE_TONE[state] ?? 'border-line-subtle text-muted'
</script>

{#if history.isError && history.data === undefined}
  <QueryError
    variant="inline"
    class="mb-3"
    error={history.error}
    title="Could not load the work log"
    onRetry={() => void history.refetch()}
  />
{:else if sessions.length > 0}
  <div class="rounded-lg border border-line-subtle">
    <div class="px-3 pb-1 pt-2 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Work log</div>
    <ul class="divide-y divide-line-subtle">
      {#each sessions as s (s.runId)}
        <li class="flex items-center gap-2 px-3 py-1.5">
          <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{s.agentModel}</span>
          <span
            class={cn(
              'shrink-0 rounded border px-1 font-mono text-[9px] font-medium uppercase tracking-[0.05em]',
              stateTone(s.state),
            )}
          >
            {s.state}
          </span>
          <span class="shrink-0 font-mono text-[11px] text-muted" title={s.finishedAt ?? undefined}>
            {s.finishedAt ? relativeTime(s.finishedAt) : 'running'}
          </span>
          <button
            type="button"
            title="View log"
            aria-label="View log"
            onclick={() => onView(s.runId)}
            class="flex shrink-0 items-center rounded-md p-1 text-muted transition-colors hover:text-fg"
          >
            <Eye size={13} />
          </button>
        </li>
      {/each}
    </ul>
  </div>
{/if}
