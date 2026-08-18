<script lang="ts">
  import { cn } from '@/lib/cn'
  import Skeleton from './Skeleton.svelte'

  // A card-shaped placeholder on the §8 panel treatment: title bar + a couple
  // of text lines. Drop into the same grid the real cards will occupy.
  // (See Skeleton.svelte for the material.)
  let { class: className }: { class?: string } = $props()
</script>

<div aria-hidden="true" class={cn('space-y-3 rounded-lg border border-line bg-panel p-4', className)}>
  <Skeleton class="h-3 w-1/2 rounded-full" />
  <Skeleton class="h-2.5 w-full rounded-full" />
  <Skeleton class="h-2.5 w-2/3 rounded-full" />
</div>
