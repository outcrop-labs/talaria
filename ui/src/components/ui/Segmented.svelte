<script lang="ts" generics="T extends string">
  import { cn } from '@/lib/cn'
  import { markCrossfade } from '@/lib/motion'
  import { useUiVoice } from '@/lib/ui-voice'
  import { sansVoice, type ButtonVoice } from './button'
  import { focusRing } from './control'
  import type { SegmentedOption } from './segmented'

  // The one segmented control — a bordered group of exclusive options. Use for
  // small mode switches (read/edit, list/grid); Tabs for page-level sections.
  // Mercury: the AUTO/MANUAL mono toggle from board `8A-0` — hairline group,
  // active cell raised + readout, inactive cells muted, mono uppercase labels.
  let {
    options,
    value,
    onChange,
    size = 'sm',
    variant: variantProp,
    voice: voiceProp,
    class: className,
  }: {
    options: ReadonlyArray<SegmentedOption<T>>
    value: T
    onChange: (id: T) => void
    /** `md` is as tall as a `sm` Input or Select, for a filter row that holds both. */
    size?: 'xs' | 'sm' | 'md'
    variant?: 'dither' | 'solid'
    /** Wins over the voice the surrounding area set (`lib/ui-voice`). */
    voice?: ButtonVoice
    class?: string
  } = $props()

  const areaVoice = useUiVoice()
  const voice = $derived(voiceProp ?? areaVoice)
  // The sans voice is matte: its thumb is the solid one unless a caller says otherwise.
  const variant = $derived(variantProp ?? (voice === 'sans' ? 'solid' : 'dither'))

  // The raised thumb slides between cells (ANIMATIONS.md `markCrossfade`);
  // one pair per control instance so neighboring Segmenteds never trade marks.
  const [sendMark, receiveMark] = markCrossfade()
</script>

<div class={cn('relative inline-flex shrink-0 rounded-md border border-line p-0.5', className)}>
  {#each options as o (o.id)}
    <button
      {...o.data}
      type="button"
      title={o.title}
      aria-pressed={value === o.id}
      onclick={() => onChange(o.id)}
      class={cn(
        'relative select-none rounded font-mono uppercase tracking-[0.05em] transition-colors',
        focusRing,
        size === 'xs' ? 'px-2 py-0.5 text-[10px] leading-3' : size === 'md' ? 'px-3 py-[7px] text-[10px] leading-4' : 'px-2.5 py-1 text-[10px] leading-4',
        voice === 'sans' &&
          cn(
            sansVoice,
            'font-medium',
            size === 'xs' ? 'px-2 py-[3px] text-xs leading-4' : size === 'md' ? 'px-3 py-[5px] leading-5' : 'px-2.5 py-[3px] leading-5',
          ),
        variant === 'dither' && 'dither-fill',
        value === o.id ? 'text-fg' : 'text-muted hover:text-fg',
      )}
    >
      {#if value === o.id}
        <span
          aria-hidden="true"
          in:receiveMark={{ key: 'mark' }}
          out:sendMark={{ key: 'mark' }}
          data-dither-band={variant === 'dither' ? '0' : undefined}
          class={cn(
            'absolute inset-0 rounded bg-raised',
            variant === 'dither' ? 'dither-mark' : 'border border-line-strong shadow-[var(--theme-shadow-1)]',
          )}
        ></span>
      {/if}
      <span class="relative">
        {#if typeof o.label === 'string'}{o.label}{:else}{@render o.label()}{/if}
      </span>
    </button>
  {/each}
</div>
