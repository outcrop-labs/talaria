<script lang="ts">
  import { cn } from '@/lib/cn'
  import Kbd from './Kbd.svelte'

  // A "⏎ send" style hint beside a composer control. Renders always (so layout
  // never jumps) and fades with availability.
  let {
    keys,
    label,
    visible,
    class: className,
  }: {
    keys: string
    label?: string
    visible: boolean
    class?: string
  } = $props()
</script>

<span
  aria-hidden={!visible}
  class={cn(
    'flex select-none items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-opacity duration-200',
    visible ? 'opacity-100' : 'opacity-0',
    className,
  )}
>
  <Kbd>{keys}</Kbd>
  {label}
</span>
