<script lang="ts">
  import { Star } from '@lucide/svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { cn } from '@/lib/cn'
  import { useSession } from '@/lib/session'
  import { useStars } from '@/lib/comms-stars.svelte'

  // A conversation header's star (beside the name). Starring lists
  // the conversation in the Comms rail's Starred section; the rail, this
  // button and the rows' menus share one store, so they move together.
  //
  // `starKey` is `channel:<channelId>` for a channel, relay or person DM, and
  // `agent:<model>` for an agent (lib/comms-stars.ts). Whose stars these are
  // comes from the session — stars are per person, per browser.
  let { starKey, class: className }: { starKey: string; class?: string } = $props()

  const session = useSession()
  const stars = useStars(() => session.data?.id ?? null)
  const starred = $derived(stars.has(starKey))
</script>

<IconButton
  size="sm"
  title={starred ? 'Unstar conversation' : 'Star conversation'}
  aria-pressed={starred}
  disabled={!session.data}
  class={cn(starred && 'text-accent hover:text-accent', className)}
  onclick={() => stars.toggle(starKey)}
>
  <Star size={15} fill={starred ? 'currentColor' : 'none'} />
</IconButton>
