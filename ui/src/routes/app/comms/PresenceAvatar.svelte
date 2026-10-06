<script lang="ts">
  import Avatar from '@/components/ui/Avatar.svelte'
  import { DOT_COLOR } from '@/components/ui/chip'
  import { cn } from '@/lib/cn'
  import type { Presence } from '@/lib/comms-sidebar'

  // A 20px avatar wearing a 7px presence badge at its bottom-right (R10, R11):
  // FILLED in the healthy dot color when online, a HOLLOW ring when offline.
  // The badge's own ring is the surface color so it reads as cut out of the
  // avatar on whatever it sits on. No new colors (KTD1).
  let {
    name,
    src,
    presence,
    cutout = 'ring-sidebar',
    class: className,
  }: {
    name: string
    src?: string | null
    presence: Presence
    /** The ring that separates the badge from the avatar — the color of the
     *  surface underneath (the rail by default). */
    cutout?: string
    class?: string
  } = $props()
</script>

<span
  role="img"
  aria-label={`${name}, ${presence}`}
  class={cn('relative inline-flex h-5 w-5 shrink-0', className)}
>
  <Avatar {name} {src} class="h-5 w-5 text-[9px]" />
  <span
    aria-hidden="true"
    class={cn(
      'absolute -bottom-px -right-px h-[7px] w-[7px] rounded-full ring-[1.5px]',
      cutout,
      presence === 'online' ? '' : 'border border-muted bg-panel',
    )}
    style:background={presence === 'online' ? DOT_COLOR.ok : undefined}
  ></span>
</span>
