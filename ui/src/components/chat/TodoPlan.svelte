<script lang="ts">
  import { Check, Circle } from '@lucide/svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { cn } from '@/lib/cn'
  import type { TodoItem } from '@/lib/chips'

  // THE AGENT'S PLAN, as a timeline. This is the one thing in the stream that
  // answers "is it doing what I asked, and how far through is it" — so it is
  // read at a glance or it is not worth the vertical space: one line per step,
  // the active one marked and emphasised, the done ones struck back.
  //
  // IT RENDERS THE CHIP'S OWN LIST, not the conversation's current plan. A
  // chip is a record of a moment, and a reader scrolling back wants the plan
  // as it stood then; re-reading the live plan into every old chip would
  // rewrite history and make the stream disagree with itself.
  let { items, label }: { items: TodoItem[]; label?: string } = $props()

  const done = $derived(items.filter((i) => i.status === 'done').length)
  // The mono chrome line says the count; `label` is the server's own summary
  // and wins when it is there, because it is the same sentence the agent and
  // the audit log saw.
  const heading = $derived(label || `${done}/${items.length} done`)
</script>

{#if items.length > 0}
  <div class="w-full rounded-lg border border-line bg-surface px-3 py-2">
    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
      Plan · {heading}
    </div>
    <ol class="mt-1.5 flex flex-col gap-1">
      {#each items as item, i (`${i}-${item.text}`)}
        <li class="flex items-start gap-2 font-sans text-[13px] leading-snug">
          <span class="mt-[2px] grid h-3.5 w-3.5 shrink-0 place-items-center" aria-hidden="true">
            {#if item.status === 'done'}
              <Check size={13} class="text-[color:var(--theme-ok)]" />
            {:else if item.status === 'active'}
              <!-- The pulse is the whole point of the active mark: it says
                   WORKING, which a static dot cannot. Same mark the rest of
                   the app uses for in-flight work. -->
              <WaitingMark site="todo/active" size={11} class="text-accent" />
            {:else}
              <Circle size={9} class="text-muted" />
            {/if}
          </span>
          <span
            class={cn(
              'min-w-0',
              item.status === 'done' && 'text-muted line-through decoration-line',
              item.status === 'active' && 'text-fg',
              item.status === 'pending' && 'text-muted',
            )}
          >
            {item.text}
          </span>
          <!-- The status in words, for a screen reader and for anyone who
               cannot tell the three marks apart. -->
          <span class="sr-only">{item.status}</span>
        </li>
      {/each}
    </ol>
  </div>
{/if}
