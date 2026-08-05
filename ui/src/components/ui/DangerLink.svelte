<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'

  // Destructive trigger: a quiet mono link that turns safety-orange on hover —
  // never an orange fill, and never a button that pulls focus. Pair with the
  // confirm() dialog for the deliberate step.
  let {
    onClick,
    children,
    class: className,
    disabled,
  }: {
    onClick: () => void
    children: Snippet
    class?: string
    disabled?: boolean
  } = $props()
</script>

<button
  type="button"
  onclick={onClick}
  {disabled}
  class={cn(
    'font-mono text-[10px] uppercase tracking-[0.05em] text-muted underline-offset-2 transition-colors hover:text-danger hover:underline disabled:opacity-40',
    className,
  )}
>
  {@render children()}
</button>
