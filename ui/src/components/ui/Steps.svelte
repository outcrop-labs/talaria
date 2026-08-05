<script lang="ts">
  import { Check } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // The one step indicator for wizard-style flows: numbered dots joined by
  // hairlines, check-marked once passed. Mercury: passed = gold fill with
  // dark ground glyph, current = gold outline, labels mono uppercase chrome.
  // Keep flows to 2–4 steps.
  let { steps, current }: { steps: readonly string[]; current: number } = $props()
</script>

<div class="flex items-center gap-2">
  {#each steps as label, i (label)}
    <div class="flex items-center gap-2">
      {#if i > 0}<span aria-hidden="true" class="h-px w-6 bg-line"></span>{/if}
      <span
        class={cn(
          'grid h-5 w-5 place-items-center rounded-full font-mono text-[10px] font-medium',
          i < current
            ? 'bg-accent text-surface'
            : i === current
              ? 'border border-accent text-accent'
              : 'border border-line text-muted',
        )}
      >
        {#if i < current}<Check size={11} />{:else}{i + 1}{/if}
      </span>
      <span
        class={cn(
          'font-mono text-[10px] uppercase tracking-[0.05em]',
          i === current ? 'font-medium text-fg' : 'text-muted',
        )}
      >
        {label}
      </span>
    </div>
  {/each}
</div>
