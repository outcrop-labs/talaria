<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { listStagger } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import { clockLabel, type BriefView } from './daily-brief.svelte'

  /**
   * How the day moved — the append log, made visible.
   *
   * THIS IS THE HALF THE OLD INBOX COULD NOT HAVE. A queue reorders itself
   * under the reader and therefore cannot answer "what changed since I looked";
   * a log answers it by being a log. Every batch here is one sweep: the
   * assistant's sentence about it, then the rows it appended.
   *
   * NEWEST FIRST, unlike the mirrored artifact, and the difference is about who
   * is reading. A shared document is read top-to-bottom as a narrative. A
   * person checking back at 14:00 wants the last thing that happened first.
   */
  let { brief }: { brief: BriefView } = $props()

  const VERB: Record<string, { label: string; tone: 'neutral' | 'accent' | 'success' }> = {
    item: { label: 'NEW', tone: 'accent' },
    change: { label: 'CHANGED', tone: 'neutral' },
    resolved: { label: 'DONE', tone: 'success' },
  }
</script>

{#if brief.updates.length === 0}
  <EmptyState
    variant="inline"
    title="Nothing has moved yet"
    hint="Your assistant is watching. Anything that changes across the platform gets appended here."
  />
{:else}
  <div use:listStagger class="space-y-5">
    {#each brief.updates as update (update.seq)}
      <section class={cn('relative pl-5', update.seq > brief.readSeq && 'text-fg')}>
        <!-- The spine. A single hairline down the batch, with the moment on
             it — the log's own shape, drawn once rather than per row. -->
        <span class="absolute left-[3px] top-2 h-[calc(100%-0.5rem)] w-px bg-line-subtle"></span>
        <span
          class={cn(
            'absolute left-0 top-[7px] size-[7px] rounded-full border-2 border-panel',
            update.seq > brief.readSeq ? 'bg-accent' : 'bg-line-strong',
          )}
        ></span>

        <div class="flex items-baseline gap-2">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            {clockLabel(update.at, brief.zone)}
          </span>
        </div>

        {#if update.note}
          <p class="mt-1 font-sans text-[13px] leading-5 text-fg">{update.note}</p>
        {/if}

        <div class="mt-2 space-y-1.5">
          {#each update.entries as entry (entry.id)}
            {@const verb = VERB[entry.kind] ?? VERB.item!}
            <div class="flex items-start gap-2">
              <Chip tone={verb.tone} class="mt-px">{verb.label}</Chip>
              <span class="min-w-0 font-sans text-[12.5px] leading-5 text-muted">{entry.title}</span>
            </div>
          {/each}
        </div>
      </section>
    {/each}
  </div>
{/if}
