<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { PIPELINE } from './focus-inbox'
</script>

<div class="space-y-4 py-12">
  <Skeleton class="h-3 w-44 rounded-full" />
  <Skeleton class="h-12 w-4/5 rounded-md" />
  <Skeleton class="h-4 w-3/5 rounded-full" />
  <div class="grid grid-cols-5 gap-2 pt-5">{#each PIPELINE as label (label)}<Skeleton class="h-[30px] rounded" />{/each}</div>
  <Skeleton class="mt-6 h-9 w-56 rounded-md" />
</div>
