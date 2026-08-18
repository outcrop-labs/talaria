<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { PIPELINE } from './focus-inbox'

  // The focus surface's silhouette while the workspace query is in flight:
  // the active card's sketch (source line, question, context, pipeline,
  // action row), then the "In queue" preview — heading + item-shaped rows
  // mirroring QueuePreview's grid (priority · question · age). FocusInbox
  // renders this inside <Materialize>, so the real card and queue rows
  // stagger in over it with no layout jump.
  const rowW = ['w-3/5', 'w-4/5', 'w-1/2', 'w-2/3']
</script>

<div aria-hidden="true">
  <div class="space-y-4 py-12">
    <Skeleton class="h-3 w-44 rounded-full" />
    <Skeleton class="h-12 w-4/5 rounded-md" />
    <Skeleton class="h-4 w-3/5 rounded-full" />
    <div class="grid grid-cols-5 gap-2 pt-5">{#each PIPELINE as label (label)}<Skeleton class="h-[30px] rounded" />{/each}</div>
    <Skeleton class="mt-6 h-9 w-56 rounded-md" />
  </div>
  <div class="py-8">
    <div class="mb-2 flex h-4 items-center"><Skeleton class="h-2.5 w-20 rounded-full" /></div>
    <div class="divide-y divide-line border-y border-line">
      {#each [0, 1, 2, 3] as i (i)}
        <div class="grid min-h-11 grid-cols-[2.5rem_minmax(0,1fr)_auto] items-center gap-3 py-2">
          <Skeleton class="h-2.5 w-8 rounded" />
          <div class="flex h-5 items-center">
            <Skeleton class={`h-3 rounded-full ${rowW[i % rowW.length]}`} />
          </div>
          <Skeleton class="h-2.5 w-12 rounded" />
        </div>
      {/each}
    </div>
  </div>
</div>
