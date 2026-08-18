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

  // Quiet on purpose: the bubble is a container for the mark, not a message.
  // The mark is the moving thing and has to stay the brightest part of this.
  const field = ditherSurface({ always: () => true, density: 0.5, weight: 0.3 })
</script>

<!-- `rounded-bl-sm` is the tail — the one corner a chat bubble does not round,
     which is enough to read as speech without drawing a pointer. -->
<span
  {@attach field}
  class={cn(
    'inline-flex items-center gap-1.5 rounded-2xl rounded-bl-sm border border-line px-2.5 py-1.5',
    className,
  )}
>
  <WaitingMark site={{ key: `chat/turn:${id}`, role, slot: 'inline' }} class="text-accent" />
</span>
