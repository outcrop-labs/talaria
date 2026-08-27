<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'

  /**
   * The brief's own loading shape — hero, then two sections of rows.
   *
   * ITS OWN FILE, not a shared skeleton, because the surfaces are different
   * shapes: the old focus queue was one big card over a short list, and a brief
   * is a wide header over several titled groups. A skeleton that does not match
   * what replaces it makes the swap jump, which is the one thing a skeleton
   * exists to prevent.
   *
   * No stagger and no delay: static has no direction, and a sweep would imply a
   * completion the fetch cannot promise (docs/UI-CONVENTIONS.md, Loading).
   */
</script>

<div class="space-y-8">
  <div class="rounded-lg border border-line px-7 py-7">
    <Skeleton class="h-6 w-56 rounded-full" />
    <div class="mt-4 max-w-[52ch]"><SkeletonRows rows={2} /></div>
    <Skeleton class="mt-5 h-2.5 w-64 rounded-full" />
  </div>

  {#each [3, 2] as rows, i (i)}
    <div>
      <Skeleton class="mb-4 h-2.5 w-24 rounded-full" />
      <div class="space-y-4">
        {#each Array.from({ length: rows }) as _, r (r)}
          <div class="flex gap-3 px-3">
            <!-- NOT a Skeleton, and not for a material reason.
                 A skeleton stands in for content whose SHAPE is unknown until
                 it arrives. This dot is the row's rail: every line has one, in
                 the same place, at the same size, whatever the data turns out
                 to be — so rendering it as signal static claims uncertainty
                 that does not exist, and at 6px it is two cells of a
                 statistical material trying to be a bullet. A flat line-toned
                 dot is the honest stand-in, and it does not imply a status the
                 way a real `StatusDot` would. -->
            <div class="mt-1.5 size-1.5 shrink-0 rounded-full bg-line"></div>
            <div class="min-w-0 flex-1">
              <Skeleton class="h-3.5 w-2/3 rounded-full" />
              <div class="mt-2"><SkeletonRows rows={1} /></div>
            </div>
          </div>
        {/each}
      </div>
    </div>
  {/each}
</div>
