<script lang="ts">
  import { ChevronRight, Clock3, ExternalLink } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import type { FocusAction, FocusItem } from '@/lib/inbox-focus.svelte'
  import { listStagger } from '@/lib/motion'
  import { INBOX_SNOOZE_OPTIONS } from './inbox-focus-shell'
  import { PIPELINE, metadataValue, priorityClass, sourceLabel, stageFor } from './focus-inbox'

  let {
    item,
    recommendedAction,
    busyAction,
    snoozeMs,
    onSnoozeMs,
    onAction,
    onSnooze,
    onSkip,
    canSkip,
  }: {
    item: FocusItem
    recommendedAction: FocusAction | null
    busyAction: string | null
    snoozeMs: number
    onSnoozeMs: (value: number) => void
    onAction: (action: FocusAction) => void
    onSnooze: () => void
    onSkip: () => void
    canSkip: boolean
  } = $props()

  const stage = $derived(stageFor(item))
  const otherActions = $derived(item.actions.filter((action) => action.id !== recommendedAction?.id))
  const meta = $derived(Object.entries(item.metadata).filter(([, value]) => value !== null).slice(0, 5))
</script>

<section aria-labelledby="focus-question" class="border-b border-line py-10 sm:py-12">
  <div class="mb-3 flex flex-wrap items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
    <span>Next up</span><span>·</span>
    <span class={priorityClass(item.priority)}>{item.priority}</span><span>·</span>
    <span>{item.statusLabel}</span><span>·</span>
    <span>{relativeTime(item.createdAt)}</span>
    {#if item.briefStatus === 'pending'}<span class="ml-auto text-ink-dim">Refining</span>{/if}
  </div>
  <h2 id="focus-question" class="max-w-[720px] font-sans text-[32px] font-light leading-[1.15] tracking-[-0.025em] text-fg sm:text-[40px] sm:leading-[48px]">
    {item.question}
  </h2>
  <p class="mt-3 max-w-[680px] font-sans text-sm leading-5 text-muted">{item.recommendation}</p>

  <div class="mt-5 flex flex-wrap gap-x-5 gap-y-1 font-mono text-[10px] uppercase tracking-[0.055em] text-ink-dim">
    <span>{sourceLabel(item)}</span>
    {#each meta as [key, value] (key)}<span>{key.replaceAll('_', ' ')}: <span class="text-muted">{metadataValue(value)}</span></span>{/each}
  </div>

  {#if item.evidence.length > 0}
    <div class="mt-5 grid gap-px overflow-hidden rounded-md border border-line bg-line sm:grid-cols-2" use:listStagger>
      {#each item.evidence.slice(0, 4) as evidence (`${evidence.label}:${evidence.text}`)}
        <div class="bg-panel px-3 py-3">
          <div class="mb-1 font-mono text-[9px] uppercase tracking-[0.08em] text-ink-dim">{evidence.label}</div>
          <p class="line-clamp-3 font-sans text-xs leading-[18px] text-muted">{evidence.text}</p>
        </div>
      {/each}
    </div>
  {/if}

  <ol aria-label="Decision progress" class="mt-6 grid grid-cols-5 gap-1.5">
    {#each PIPELINE as label, index (label)}
      <li class={cn('flex h-[30px] min-w-0 items-center justify-center rounded border font-mono text-[9px] uppercase tracking-[0.04em]', index === stage ? 'border-success bg-success/10 text-success' : index < stage ? 'border-line-strong text-muted' : 'border-line text-ink-dim')}>
        <span class="hidden sm:inline">0{index + 1} · </span>{label}
      </li>
    {/each}
  </ol>

  <div class="mt-6 flex flex-wrap items-center gap-2">
    {#if recommendedAction}
      <Button size="sm" onclick={() => onAction(recommendedAction)} disabled={busyAction !== null}>
        {busyAction === recommendedAction.id ? 'Working' : recommendedAction.label}
      </Button>
    {:else}
      <a href={item.sourceHref} class={buttonClasses({ size: 'sm' })}>Open source</a>
    {/if}
    {#each otherActions as action (action.id)}
      <Button size="sm" variant="outline" onclick={() => onAction(action)} disabled={busyAction !== null}>
        {action.label}
      </Button>
    {/each}
    {#if recommendedAction}
      <a href={item.sourceHref} class={buttonClasses({ variant: 'outline', size: 'sm' })}>
        <ExternalLink size={12} /> View source
      </a>
    {/if}
    <div class="flex items-center rounded-md border border-line bg-raised">
      <label class="sr-only" for="focus-snooze">Snooze duration</label>
      <select id="focus-snooze" value={snoozeMs} onchange={(event) => onSnoozeMs(Number(event.currentTarget.value))} class="h-8 bg-transparent pl-2 font-mono text-[10px] uppercase tracking-[0.04em] text-muted outline-none">
        {#each INBOX_SNOOZE_OPTIONS as option (option.value)}<option value={option.value}>{option.label}</option>{/each}
      </select>
      <button type="button" onclick={onSnooze} disabled={busyAction !== null} class="grid h-8 w-8 place-items-center text-muted hover:text-fg disabled:opacity-50" aria-label="Snooze item">
        <Clock3 size={12} />
      </button>
    </div>
    <Button variant="ghost" size="xs" class="ml-auto h-8 gap-1.5 disabled:opacity-40" onclick={onSkip} disabled={!canSkip || busyAction !== null}>
      Skip <ChevronRight size={12} />
    </Button>
  </div>
  <div class="mt-2 flex gap-4 font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim">
    <span>A · Primary</span><span>O · Open</span><span>S · Snooze</span>
  </div>
</section>
