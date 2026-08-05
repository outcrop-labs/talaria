<script lang="ts">
  import { ExternalLink, RotateCcw } from '@lucide/svelte'
  import MessageAttachments from '@/components/chat/MessageAttachments.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Markdown from '@/components/ui/Markdown.svelte'
  import { cn } from '@/lib/cn'
  import { fade, QUICK } from '@/lib/motion'
  import type { InboxTimelineEntry } from '@/lib/inbox-focus.svelte'

  type ActivityEntry = Extract<InboxTimelineEntry, { kind: 'activity' }>

  let {
    entry,
    readOnly,
    onConfirm,
    onCancel,
    onRetry,
    onUndo,
  }: {
    entry: InboxTimelineEntry
    readOnly: boolean
    onConfirm: (entry: ActivityEntry) => void
    onCancel: (entry: ActivityEntry) => void
    onRetry: (entry: ActivityEntry) => void
    onUndo: (entry: ActivityEntry) => void
  } = $props()

  const ACTIVITY_LABELS = {
    proposal: 'Proposed action',
    confirmation: 'Confirmation required',
    completion: 'Completed',
    failure: 'Action failed',
    cancellation: 'Cancelled',
    undo: 'Undone',
  } as const

  function exactPreview(value: unknown): string {
    if (typeof value === 'string') return value
    try {
      return JSON.stringify(value, null, 2)
    } catch {
      return String(value ?? '')
    }
  }
</script>

{#if entry.kind === 'context'}
  <div class="flex items-center gap-3 py-1">
    <span class="h-px flex-1 bg-line"></span>
    <a href={entry.focus.sourceHref} class="max-w-[72%] truncate font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim hover:text-muted">{entry.focus.question}</a>
    <span class="h-px flex-1 bg-line"></span>
  </div>
{:else if entry.kind === 'message'}
  {#if entry.role === 'user'}
    <div class="ml-auto max-w-[86%] rounded-xl rounded-br-sm border border-line bg-raised px-3 py-2.5 font-sans text-[13px] leading-5 text-fg">
      <Markdown children={entry.content} />
      <MessageAttachments items={entry.attachments} />
    </div>
  {:else}
    <div class="border-t border-line pt-4">
      {#if entry.delegateModel}<div class="mb-2 font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">Consulted {entry.delegateModel}</div>{/if}
      {#if entry.status === 'error'}
        <p class="font-sans text-xs text-danger">Your assistant did not finish this response.</p>
      {:else}
        <div class="font-sans text-[13px] leading-5 text-fg"><Markdown children={entry.content} /></div>
      {/if}
    </div>
  {/if}
{:else}
  {@const label = ACTIVITY_LABELS[entry.activity]}
  <section class={cn('rounded-lg border bg-panel p-3', entry.activity === 'failure' ? 'border-danger/45' : entry.activity === 'confirmation' ? 'border-accent/55' : 'border-line')} aria-label={label}>
    <div class="font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">{label}</div>
    <h3 class="mt-1 font-sans text-[13px] font-medium text-fg">{entry.title}</h3>
    {#if entry.message}<p class="mt-1.5 font-sans text-[11px] leading-4 text-muted">{entry.message}</p>{/if}
    {#if entry.activity === 'confirmation'}
      <pre class="mt-3 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md border border-line bg-surface p-2.5 font-mono text-[10px] leading-4 text-muted">{exactPreview(entry.details)}</pre>
    {/if}
    <div class="mt-3 flex flex-wrap items-center gap-2">
      {#if !readOnly && entry.activity === 'confirmation' && entry.confirmationToken}
        <!-- fade, not slide: these live in a horizontal flex row, so a height
             slide has no gap to announce — grammar's row fade fits better. -->
        <div in:fade={{ duration: 150 }} out:fade={QUICK} class="flex flex-wrap items-center gap-2">
          <Button size="sm" onclick={() => onConfirm(entry)}>Confirm exact action</Button>
          <Button size="sm" variant="ghost" onclick={() => onCancel(entry)}>Cancel</Button>
        </div>
      {/if}
      {#if !readOnly && entry.activity === 'failure' && entry.actionId}<Button size="sm" variant="outline" onclick={() => onRetry(entry)}>Retry</Button>{/if}
      {#if !readOnly && entry.activity === 'completion' && entry.undoExpiresAt}
        <div in:fade={{ duration: 150 }} out:fade={QUICK}>
          <Button size="sm" variant="ghost" onclick={() => onUndo(entry)}><RotateCcw size={12} /> Undo</Button>
        </div>
      {:else if entry.activity === 'completion'}
        <a href={entry.focus.sourceHref} class={buttonClasses({ variant: 'ghost', size: 'sm' })}><ExternalLink size={12} /> View result</a>
      {/if}
      {#if entry.activity === 'failure' || entry.activity === 'cancellation' || (readOnly && (entry.activity === 'proposal' || entry.activity === 'confirmation'))}
        <a href={entry.focus.sourceHref} class={buttonClasses({ variant: 'ghost', size: 'sm' })}>Open source</a>
      {/if}
    </div>
  </section>
{/if}
