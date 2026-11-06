<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import { cn } from '@/lib/cn'
  import { useClock } from '@/lib/clock.svelte'
  import type { LiveWorkSession } from '@/lib/work-session.svelte'
  import { agentName, elapsed, since, spendLine, stateChip, stepLine } from '@/lib/work-pulse'

  // A live work session at a glance: is the agent working or waiting, on
  // which step, for how long, what it last printed, what it has cost, and
  // what failed. Every fact is the server's (`activity`, `spend` on the
  // session); this only ticks the clocks against the server's `now`.
  //
  // `compact` is the board's two-line spelling: the state, the step's clock
  // and the spend, then the step itself. The full one is the ticket's.
  let {
    session,
    receivedAt,
    compact = false,
    class: className,
  }: {
    session: LiveWorkSession
    /** When this answer arrived (the query's dataUpdatedAt), for the skew. */
    receivedAt: number
    compact?: boolean
    class?: string
  } = $props()

  const clock = useClock()
  const a = $derived(session.activity ?? null)
  const agent = $derived(agentName(session.agentModel))
  // The server's clock minus ours, so a timer reads the same in every tab.
  const skew = $derived(session.now != null && receivedAt ? session.now - receivedAt : 0)
  const ago = (at: number | null | undefined) => since(at, clock.now, skew)

  const ofTurns = $derived(/turn \d+ of (\d+)/.exec(session.phase ?? '')?.[1] ?? null)
  const turn = $derived(a?.turn ?? session.turn ?? null)
  const turnFor = $derived(ago(a?.turnAt))
  const stepFor = $derived(ago(a?.step?.since))
  const quietFor = $derived(ago(a?.lastAt ?? a?.turnAt))
  const chip = $derived(a ? stateChip(a.state, agent, quietFor) : null)
  const spend = $derived(spendLine(session.spend))
  const failedAgo = $derived(ago(a?.failure?.at))
</script>

{#if compact}
  <div class={cn('min-w-0 text-xs', className)} data-testid="work-pulse-compact">
    <div class="flex min-w-0 items-center gap-1.5">
      {#if chip}<Chip tone={chip.tone} title={chip.title}>{chip.label}</Chip>{/if}
      {#if a?.step && stepFor != null}
        <span class="shrink-0 font-mono text-[11px] text-fg" title="this step has run {elapsed(stepFor)}">{elapsed(stepFor)}</span>
      {:else if turn}
        <span class="shrink-0 text-muted">turn {turn}{#if turnFor != null} · {elapsed(turnFor)}{/if}</span>
      {/if}
      {#if a?.failures}
        <span class="shrink-0 font-mono text-[11px] text-danger" title={a.failure?.text || undefined}>✕{a.failures}</span>
      {/if}
      {#if spend.text && !spend.muted}
        <span class="ml-auto shrink-0 font-mono text-[11px] text-muted" title={spend.title}>{spend.text}</span>
      {/if}
    </div>
    {#if a?.step}
      <div class="mt-0.5 truncate font-mono text-[11px] text-muted" title={stepLine(a.step)}>{stepLine(a.step)}</div>
    {:else if a?.output}
      <div class="mt-0.5 truncate font-mono text-[11px] text-muted" title={a.output}>{a.output}</div>
    {/if}
  </div>
{:else}
  <div class={cn('min-w-0 space-y-1 text-xs', className)} data-testid="work-pulse">
    <div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      {#if chip}<Chip tone={chip.tone} title={chip.title}>{chip.label}</Chip>{/if}
      {#if turn}
        <span class="text-muted">
          turn {turn}{#if ofTurns} of {ofTurns}{/if}{#if turnFor != null} · {elapsed(turnFor)} in this turn{/if}
        </span>
      {/if}
      {#if spend.text}
        <span class={cn('ml-auto font-mono text-[11px]', spend.muted ? 'text-muted' : 'text-fg')} title={spend.title}>
          {spend.text}
        </span>
      {/if}
    </div>

    {#if a?.step}
      <div class="flex min-w-0 items-baseline gap-2">
        <span class="min-w-0 flex-1 truncate font-mono text-[11px] text-fg" title={stepLine(a.step)}>▸ {stepLine(a.step)}</span>
        {#if stepFor != null}<span class="shrink-0 font-mono text-[11px] text-muted">{elapsed(stepFor)}</span>{/if}
      </div>
      {#if a.outer}
        <div class="truncate pl-3 font-mono text-[10px] text-muted" title={stepLine(a.outer)}>inside {stepLine(a.outer)}</div>
      {/if}
    {:else if a?.last}
      <div class="flex min-w-0 items-baseline gap-2">
        <span class="min-w-0 flex-1 truncate font-mono text-[11px] text-muted" title={stepLine(a.last)}>
          last: {stepLine(a.last)}
        </span>
        {#if a.last.ms != null}
          <span class={cn('shrink-0 font-mono text-[11px]', a.last.ok ? 'text-muted' : 'text-danger')}>
            {a.last.ok ? 'took' : 'failed after'} {elapsed(a.last.ms)}
          </span>
        {/if}
      </div>
    {:else if a?.state === 'sending'}
      <div class="text-muted">The turn is sent; {agent} has not started it yet.</div>
    {/if}

    {#if a?.output}
      <div class="truncate pl-3 font-mono text-[11px] text-muted" title={a.output}>{a.output}</div>
    {:else if a?.said}
      <div class="truncate pl-3 text-muted" title={a.said}>“{a.said}”</div>
    {/if}

    {#if a?.failures && a.failure}
      <div class="truncate text-danger" title={a.failure.text || undefined}>
        ✕ {a.failures} failed step{a.failures === 1 ? '' : 's'}{a.partial ? ' recently' : ' this turn'} · last:
        {stepLine({ tool: a.failure.tool, detail: a.failure.text })}{#if failedAgo != null} · {elapsed(failedAgo)} ago{/if}
      </div>
    {/if}
  </div>
{/if}
