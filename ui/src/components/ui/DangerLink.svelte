<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { useUiVoice, type UiVoice } from '@/lib/ui-voice'

  // Destructive trigger: a quiet mono link that turns safety-orange on hover —
  // never an orange fill, and never a button that pulls focus. Pair with the
  // confirm() dialog for the deliberate step.
  let {
    onClick,
    children,
    class: className,
    disabled,
    voice,
  }: {
    onClick: () => void
    children: Snippet
    class?: string
    disabled?: boolean
    /** Wins over the voice the surrounding area set (`lib/ui-voice`). */
    voice?: UiVoice
  } = $props()

  const areaVoice = useUiVoice()
  const sans = $derived((voice ?? areaVoice) === 'sans')
</script>

<button
  type="button"
  onclick={onClick}
  {disabled}
  class={cn(
    sans
      ? 'font-sans text-[13px] leading-5 text-muted underline-offset-2 transition-colors hover:text-danger hover:underline disabled:opacity-40'
      : 'font-mono text-[10px] uppercase tracking-[0.05em] text-muted underline-offset-2 transition-colors hover:text-danger hover:underline disabled:opacity-40',
    className,
  )}
>
  {@render children()}
</button>
