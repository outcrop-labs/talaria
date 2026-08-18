<script lang="ts">
  import { cn } from '@/lib/cn'
  import { ditherSurface } from '@/lib/dither-surface'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import type { WaitingRole } from '@/lib/waiting/registry'

  /**
   * THE ONE CHAT LOADER: a mark in a dithered bubble.
   *
   * Every surface that streams an agent's words was rendering a bare
   * `<WaitingMark>` with its own padding and its own site key, so the same
   * moment — an agent about to speak — looked slightly different depending on
   * which pane you were in. This is that moment, once.
   *
   * A DIFFERENT MARK EVERY GENERATION, and it costs nothing to arrange. The
   * rotation deals one state per SITE per session, deliberately: two marks
   * alive at once must agree, and a mark must not change identity if its
   * component remounts mid-wait. Both still hold here. What varies is the KEY
   * — it carries the id of the turn being waited on — and since the inline
   * path hashes the key with the session seed, each generation draws its own
   * state and holds it for as long as that generation lasts. Nothing in
   * `lib/waiting/` had to change; this is the descriptor path it already
   * offers.
   */
  let {
    /** The turn or message being waited on. Its identity is what re-rolls. */
    id,
    /** What the wait means — sets the tempo. See spec §9. */
    role = 'submitting',
    class: className,
  }: {
    id: string
    role?: WaitingRole
    class?: string
  } = $props()

  // THE DITHER IS THE BUBBLE. There is no border and no fill behind it — the
  // field's own centre-weighted falloff is the entire form, dense in the
  // middle and dissolving before the edge, so the shape is something the
  // texture describes rather than something drawn around it.
  //
  // Denser than a control's field for that reason: a hover fill only has to
  // suggest a surface that already has a border and a tile, where this has to
  // BE the surface. Still short of the mark's own weight, because the mark is
  // the moving thing and has to stay the brightest part of this.
  const field = ditherSurface({ always: () => true, density: 0.72, weight: 0.5 })
</script>

<!-- A FIXED WIDTH, because the mark inside is not one. The rotation deals from
     thirty states across two families — braille fields run two to six cells
     wide and a dot grid is a square box — so a bubble that hugged its content
     would change size every generation, which is a loader drawing attention to
     the wrong thing. 80px clears the widest (six mono cells at 14px, about
     50px) with room either side, and everything narrower centres in it.

     The corners are uniform: the dither reads the element's radius to shape
     itself, and a single radius is all it reads — so a one-cornered tail would
     be drawn by the border that is no longer there and ignored by the field
     that replaced it. -->
<span
  {@attach field}
  class={cn('inline-flex w-20 items-center justify-center rounded-2xl py-1.5', className)}
>
  <WaitingMark site={{ key: `chat/turn:${id}`, role, slot: 'inline' }} class="text-accent" />
</span>
