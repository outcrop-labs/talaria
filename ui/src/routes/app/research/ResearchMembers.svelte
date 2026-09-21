<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import MembersHeader from '@/components/app/MembersHeader.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import { delJson, errorMessage, postJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { useSession } from '@/lib/session'
  import { useTeamsDirectory } from '@/lib/teams'
  import { useResearchMembers } from '@/lib/research'

  // Members + share, in the run header. Mirrors plan sharing: the owner adds
  // teammates (they get the run AND its report), a collaborator can leave. The
  // row itself is the shared MembersHeader.
  let { runId }: { runId: string } = $props()

  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const qc = useQueryClient()
  // Already rejected on non-2xx; this just routes it through the shared door so
  // the thrown message is the server's own. The avatars stay hidden on failure
  // — the run header has no room for an error, and hiding a SHARE control is
  // the safe direction (it grants nothing and claims nothing).
  // ONE DEFINITION, in lib/research.ts, because the discussion pane reads the
  // same list for its @mentions — and a second spelling would eventually offer
  // a mention to somebody who cannot open the report being discussed.
  const query = useResearchMembers(() => runId)
  const members = $derived(query.data?.members ?? [])
  const teams = $derived(query.data?.teams ?? [])
  const isOwner = $derived(!!session?.id && session.id === members.find((m) => m.role === 'owner')?.userId)
  const refresh = () => qc.invalidateQueries({ queryKey: ['research-members', runId] })
  // Only the share picker reads the directory, and only the owner sees it.
  const directory = useTeamsDirectory(() => isOwner)
  const shareFailed = (e: unknown) => void alert({ title: 'Could not share', message: errorMessage(e) })
  const remove = (userId: string) =>
    delJson(`/api/research/${runId}/members`, { userId })
      // Fire-and-forget from a hover chip in the run header — a toast is the
      // only surface this row has for a failed leave/remove.
      .catch((e) => toastError('Remove failed', e))
      .then(refresh)
  const removeTeam = (teamId: string) =>
    delJson(`/api/research/${runId}/teams`, { teamId })
      .catch((e) => toastError('Remove failed', e))
      .then(refresh)
</script>

<!-- Org-wide runs (no owner) have nothing to share. -->
<MembersHeader
  {members}
  {teams}
  {isOwner}
  hideWhenOwnerless
  chipSurface="card"
  directory={directory.data ?? []}
  noun="research"
  onShareUser={(email) => void postJson(`/api/research/${runId}/members`, { email }).catch(shareFailed).then(refresh)}
  onShareTeam={(teamId) => void postJson(`/api/research/${runId}/teams`, { teamId }).catch(shareFailed).then(refresh)}
  onRemoveUser={remove}
  onRemoveTeam={removeTeam}
/>