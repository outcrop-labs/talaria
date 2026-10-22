<script lang="ts">
  // THE LIVING DOCUMENT AS A PANE — the document half of a plan session's stage,
  // with the plumbing the surface used to keep around it. The document itself
  // is LivingDoc (and the doc id lookup is its own business); what moved out
  // here is what stood AROUND it on the Plan surface: the skeleton that holds
  // the document's shape while the surface is still deciding which
  // conversation stands here, the turn signal that asks for a fresh body when
  // an agent turn lands, and the resolved document id handed back out.
  //
  // IT FILLS THE BOX IT IS GIVEN, the same way FilePane does: the caller owns
  // the column — how wide it is, which breakpoint it appears at, and the
  // divider beside it.
  import LivingDoc from '@/components/chat/LivingDoc.svelte'
  import PlanDocSkeleton from '@/components/chat/PlanDocSkeleton.svelte'

  let {
    conversationId,
    syncSignal = 0,
    loading = false,
    onDocId,
  }: {
    /** The conversation this document belongs to — one document per
     *  conversation, found-or-created server-side on first open. */
    conversationId: string
    /** Bumped by the host when an agent turn lands; the document re-reads
     *  itself on it, which is how the pane stays current while you talk. */
    syncSignal?: number
    /** The HOST is still resolving which conversation stands here (its list
     *  query has not landed yet). The pane holds the document's shape rather
     *  than flashing an empty column, so nothing re-layouts when it does. */
    loading?: boolean
    /** The artifact this document resolved to, forwarded straight out of
     *  LivingDoc — nulls included, because the null is the reset a host needs:
     *  it hangs comments off the id it last saw, and a switch of conversation
     *  must not leave those pointing at the previous document. */
    onDocId?: (id: string | null) => void
  } = $props()
</script>

{#if loading || !conversationId}
  <PlanDocSkeleton />
{:else}
  <LivingDoc {conversationId} {syncSignal} {onDocId} />
{/if}
