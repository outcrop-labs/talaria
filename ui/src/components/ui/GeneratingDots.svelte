<script lang="ts">
  import { cn } from '@/lib/cn'
  import GeneratingBars from './GeneratingBars.svelte'
  import type { GeneratingVariant } from './generating'

  // The inline activity signal on its own (status rows, buttons): three gold
  // bars on the fast weave burst.
  let { class: className, variant = 'weave' }: { class?: string; variant?: GeneratingVariant } = $props()
</script>

<GeneratingBars bars={3} {variant} step={0.15} class={cn('text-accent', className)} />
