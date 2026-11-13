<script lang="ts">
  import { cn } from '@/lib/cn'
  import { ago, isMachineText, ticketStatus } from '@/lib/inbox-board'
  import { wordClamp } from '@/lib/word-clamp'
  import StateGlyph from './StateGlyph.svelte'
  import { BREAK, LABEL } from './inbox-board.svelte'
  import type { DrawerDetail, DrawerTicketRow } from './drawer-detail.svelte'

  let { detail, now }: { detail: DrawerDetail; now: number } = $props()
</script>

{#snippet ticketRow(row: DrawerTicketRow)}
  {@const status = ticketStatus(row.status)}
  <li class="flex min-w-0 items-start gap-2.5">
    {#if row.relation}<span class="w-[4.5rem] shrink-0 text-muted">{row.relation}</span>{/if}
    <span class="mt-0.5" title={status.word}><StateGlyph glyph={status.glyph} /></span>
    {#if row.ref}<span class="shrink-0 font-mono text-[12px] text-muted">{row.ref}</span>{/if}
    <span use:wordClamp={row.title} class="line-clamp-2 min-w-0 flex-1 break-words text-fg"></span>
    <span class="w-28 shrink-0 text-right text-muted">{status.word}</span>
  </li>
{/snippet}

{#if detail.subtasks.length > 0}
  <section class={BREAK}>
    <h3 class={LABEL}>Sub-tasks<span class="ml-1.5 tabular-nums text-muted">{detail.subtasks.length}</span></h3>
    <ul class="mt-2.5 space-y-2 font-sans text-[13px] leading-5">
      {#each detail.subtasks as row (row.id)}{@render ticketRow(row)}{/each}
    </ul>
  </section>
{/if}

<section class={BREAK}>
  <h3 class={LABEL}>Activity</h3>
  {#if detail.activity.length > 0}
    <ol class="mt-3 space-y-2.5 pl-4">
      {#each detail.activity as step, i (i)}
        {@const last = i === detail.activity.length - 1}
        <li class="relative flex justify-between gap-4 font-sans text-[13px] leading-5">
          <span class={cn('absolute -left-4 top-[7px] size-1.5 rounded-full', last ? 'bg-fg' : 'bg-line-strong')}></span>
          {#if !last}<span aria-hidden="true" class="absolute -bottom-[17px] -left-[13.5px] top-[14px] w-px bg-line"></span>{/if}
          <span class="block min-w-0 break-words text-muted">
            {#if step.actor}<span class="font-medium text-fg">{step.actor}</span>{/if}
            {step.label}
          </span>
          {#if step.at}<time datetime={step.at} class="shrink-0 tabular-nums text-muted">{ago(now - Date.parse(step.at))}</time>{/if}
        </li>
      {/each}
    </ol>
  {:else}
    <p class="mt-2 font-sans text-[13px] text-muted">No activity yet</p>
  {/if}
</section>

{#if detail.evidence.length > 0}
  <section class={BREAK}>
    <h3 class={LABEL}>Details</h3>
    <dl class="mt-2.5 space-y-2 font-sans text-[13px] leading-5">
      {#each detail.evidence as row (row.label + row.text)}
        <div class="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-4">
          <dt class={cn('text-muted', isMachineText(row.label) && 'pt-1.5')}>{row.label}</dt>
          {#if isMachineText(row.label)}
            <dd class="min-w-0">
              <code class="block whitespace-pre-wrap break-words rounded-md bg-surface px-2.5 py-1.5 font-mono text-[12px] leading-[18px] text-fg">{row.text}</code>
            </dd>
          {:else}
            <dd class="whitespace-pre-wrap break-words text-fg">{row.text}</dd>
          {/if}
        </div>
      {/each}
    </dl>
  </section>
{/if}

{#if detail.linked.length > 0}
  <section class={BREAK}>
    <h3 class={LABEL}>Linked tickets</h3>
    <ul class="mt-2.5 space-y-2 font-sans text-[13px] leading-5">
      {#each detail.linked as row (row.relation! + row.id)}{@render ticketRow(row)}{/each}
    </ul>
  </section>
{/if}
