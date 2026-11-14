<script lang="ts">
  import { Check } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { useUiVoice, type UiVoice } from '@/lib/ui-voice'

  // The one step indicator for wizard-style flows: numbered dots joined by
  // hairlines, check-marked once passed. Mercury: passed = gold fill with
  // dark ground glyph, current = gold outline, labels mono uppercase chrome.
  // Keep flows to 2–4 steps.
  let { steps, current, voice }: { steps: readonly string[]; current: number; voice?: UiVoice } = $props()

  const areaVoice = useUiVoice()
  const sans = $derived((voice ?? areaVoice) === 'sans')
</script>

<div class="flex items-center gap-2">
  {#each steps as label, i (label)}
    <div class="flex items-center gap-2">
      {#if i > 0}<span aria-hidden="true" class="h-px w-6 bg-line"></span>{/if}
      <span
        class={cn(
          sans
            ? 'grid h-5 w-5 place-items-center rounded-full font-sans text-xs font-medium tabular-nums'
            : 'grid h-5 w-5 place-items-center rounded-full font-mono text-[10px] font-medium',
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
          sans ? 'font-sans text-[13px] leading-5' : 'font-mono text-[10px] uppercase tracking-[0.05em]',
          i === current ? 'font-medium text-fg' : 'text-muted',
        )}
      >
        {label}
      </span>
    </div>
  {/each}
</div>
