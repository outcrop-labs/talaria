<script lang="ts">
  import MessageTime from './MessageTime.svelte'

  // The avatar column of a continuation line (lib/message-groups): the block's
  // first row shows the avatar, the name and the time; this row keeps the
  // column and fills it with its own time, shown while the row is hovered or
  // focused (the row's group/message contract, the same one that reveals the
  // action bar). Opacity, not display, hides it, so the time and the
  // visually hidden author name stay in the accessibility tree: a screen
  // reader hears who said it and when on every line, not only the first.
  let { name, at, timeZone }: { name: string; at?: string; timeZone?: string | null } = $props()
</script>

<div class="relative w-6 shrink-0">
  <span class="sr-only">{name}</span>
  <MessageTime
    {at}
    {timeZone}
    variant="gutter"
    class="absolute right-0 top-[5px] whitespace-nowrap leading-none opacity-0 transition-opacity group-hover/message:opacity-100 group-focus-within/message:opacity-100"
  />
</div>
