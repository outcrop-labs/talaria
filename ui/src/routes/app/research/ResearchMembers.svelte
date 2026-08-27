<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { UserPlus, X } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import { fade } from '@/lib/motion'
  import { delJson, errorMessage, postJson } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import { useSession } from '@/lib/session'
  import { useResearchMembers } from '@/lib/research'

  // Members + share, in the run header. Mirrors plan sharing: the owner adds
  // teammates (they get the run AND its report), a collaborator can leave.
  let { runId }: { runId: string } = $props()

  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const qc = useQueryClient()
  let adding = $state(false)
  // Already rejected on non-2xx; this just routes it through the shared door so
  // the thrown message is the server's own. The avatars stay hidden on failure
  // — the run header has no room for an error, and hiding a SHARE control is
  // the safe direction (it grants nothing and claims nothing).
  // ONE DEFINITION, in lib/research.ts, because the discussion pane reads the
  // same list for its @mentions — and a second spelling would eventually offer
  // a mention to somebody who cannot open the report being discussed.
  const query = useResearchMembers(() => runId)
  const members = $derived(query.data?.members ?? [])
  const isOwner = $derived(!!session?.id && session.id === members.find((m) => m.role === 'owner')?.userId)
  const refresh = () => qc.invalidateQueries({ queryKey: ['research-members', runId] })
  const remove = (userId: string) =>
    delJson(`/api/research/${runId}/members`, { userId })
      // Fire-and-forget from a hover chip in the run header — a toast is the
      // only surface this row has for a failed leave/remove.
      .catch((e) => pushToast({ title: 'Remove failed', body: errorMessage(e), tone: 'danger' }))
      .then(refresh)
</script>

<!-- Org-wide runs (no owner) have nothing to share. -->
{#if members.length > 0 && members.some((m) => m.role === 'owner')}
  <span class="flex items-center gap-1.5">
    <span class="flex -space-x-1.5">
      {#each members as m (m.userId)}
        <span class="group relative" title={`${m.name ?? m.email}${m.role === 'owner' ? ' (owner)' : ''}`}>
          <Avatar name={m.name ?? m.email ?? '?'} class="h-6 w-6 text-[10px] ring-2 ring-surface" />
          {#if (isOwner && m.role !== 'owner') || (m.userId === session?.id && m.role === 'collaborator')}
            <button
              type="button"
              title={m.userId === session?.id ? 'Leave this research' : `Remove ${m.name ?? m.email}`}
              onclick={() => void remove(m.userId)}
              class="absolute -right-1 -top-1 hidden h-3.5 w-3.5 place-items-center rounded-full bg-card text-muted shadow group-hover:grid hover:text-fg"
            >
              <X size={9} />
            </button>
          {/if}
        </span>
      {/each}
    </span>
    {#if isOwner}
      {#if adding}
        <!-- Fade, not slide: an in-place swap in a horizontal row — a height
             slide would jiggle the header. -->
        <div in:fade={{ duration: 150 }}>
          <UserPicker
            size="sm"
            class="w-48"
            placeholder="Share with"
            exclude={members.map((m) => m.userId)}
            onPick={(u) => {
              adding = false
              if (!u.email) return
              void postJson(`/api/research/${runId}/members`, { email: u.email })
                .catch((e) => {
                  void alert({ title: 'Could not share', message: errorMessage(e) })
                })
                .then(refresh)
            }}
          />
        </div>
      {:else}
        <button
          type="button"
          title="Share this research with a teammate"
          onclick={() => (adding = true)}
          class="grid h-6 w-6 place-items-center rounded-full border border-dashed border-line text-muted hover:text-fg"
        >
          <UserPlus size={12} />
        </button>
      {/if}
    {/if}
  </span>
{/if}
