<script lang="ts">
  import { Pin } from '@lucide/svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { cn } from '@/lib/cn'
  import { useSession } from '@/lib/session'
  import { useStars } from '@/lib/comms-stars.svelte'

  // A conversation header's pin (beside the name). Pinning lists the
  // conversation in the Comms rail's Pinned section; the rail, this button
  // and the rows' menus share one server-held list, so they move together.
  //
  // `starKey` is `channel:<channelId>` for a channel, relay or person DM, and
  // `agent:<model>` for an agent (lib/comms-stars.ts — the code says "star"
  // for the history). Whose pins these are comes from the session.
  let { starKey, class: className }: { starKey: string; class?: string } = $props()

  const session = useSession()
  const stars = useStars(() => session.data?.id ?? null)
  const starred = $derived(stars.has(starKey))
</script>

<IconButton
  size="sm"
  title={starred ? 'Unpin conversation' : 'Pin conversation'}
  aria-pressed={starred}
  disabled={!session.data}
  class={cn(starred && 'text-accent hover:text-accent', className)}
  onclick={() => stars.toggle(starKey)}
>
  <Pin size={15} fill={starred ? 'currentColor' : 'none'} />
</IconButton>
