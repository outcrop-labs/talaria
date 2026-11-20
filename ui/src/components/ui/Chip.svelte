<script lang="ts">
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { useUiVoice } from '@/lib/ui-voice'
  import { SANS_TONES, TONES, type ChipProps } from './chip'

  // Bordered micro-chip for kinds/modes ("DOC", "Brief", "custom") — and, with
  // `onSelect`/`onRemove`, the one filter-pill and removable-token primitive.
  // Mono chrome voice; selected = raised tile + strong hairline + readout.
  let {
    children,
    class: className,
    title,
    tone = 'neutral',
    onSelect,
    selected,
    onRemove,
    voice,
  }: ChipProps = $props()

  const areaVoice = useUiVoice()
  const sans = $derived((voice ?? areaVoice) === 'sans')

  const base = $derived(
    sans
      ? cn(
          'inline-flex h-[22px] shrink-0 select-none items-center whitespace-nowrap rounded-md px-1.5 font-sans text-xs font-medium',
          selected ? 'bg-raised text-fg ring-1 ring-inset ring-line-strong' : SANS_TONES[tone],
          onSelect && !selected && 'transition-colors hover:text-fg',
          className,
        )
      : cn(
          'shrink-0 select-none rounded border px-1.5 font-mono text-[10px] font-medium uppercase tracking-[0.05em]',
          selected ? 'border-line-strong bg-raised text-fg' : TONES[tone],
          onSelect && !selected && 'transition-colors hover:text-fg',
          className,
        ),
  )
</script>

{#snippet body()}
  {@render children()}
  {#if onRemove}
    <button
      type="button"
      aria-label="Remove"
      onclick={(e) => {
        e.stopPropagation()
        onRemove()
      }}
      class="ml-1 text-muted transition-colors hover:text-danger"
    >
      ✕
    </button>
  {/if}
{/snippet}

{#if onSelect}
  <button type="button" {title} onclick={onSelect} class={cn(base, 'inline-flex items-center', focusGold)}>
    {@render body()}
  </button>
{:else}
  <span {title} class={cn(base, onRemove && 'inline-flex items-center')}>
    {@render body()}
  </span>
{/if}
