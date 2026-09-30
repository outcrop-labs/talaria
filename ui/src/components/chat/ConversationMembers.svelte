<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import MembersHeader from '@/components/app/MembersHeader.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import { useSession } from '@/lib/session'
  import { useTeamsDirectory } from '@/lib/teams'
  import {
    shareConversation,
    shareConversationTeam,
    unshareConversation,
    unshareConversationTeam,
    useConversationMembers,
  } from '@/lib/conversations.svelte'

  // Members + presence + share, in a shared conversation's header — a plan or a
  // work session. Owner shares/removes; a collaborator can leave. Green ring =
  // viewing right now. The row itself is the shared MembersHeader.
  //
  // It lived in routes/app/plan/ when only Plan had members. Two surfaces share
  // it now, so it lives with the other chat components; the membership plane it
  // reads (conversation_members / conversation_teams) was never plan-specific.
  let { conversationId }: { conversationId: string } = $props()

  const membersQuery = useConversationMembers(() => conversationId)
  const sessionQuery = useSession()
  const qc = useQueryClient()
  const session = $derived(sessionQuery.data)
  const members = $derived(membersQuery.data?.members ?? [])
  const teams = $derived(membersQuery.data?.teams ?? [])
  const active = $derived(new Set(membersQuery.data?.active ?? []))
  const isOwner = $derived(!!session?.id && session.id === members.find((m) => m.role === 'owner')?.userId)
  const refresh = () => qc.invalidateQueries({ queryKey: ['conversation-members', conversationId] })
  // Only the share picker reads the directory, and only the owner sees it.
  const directory = useTeamsDirectory(() => isOwner)
  // The server's own sentence for a refused share. A header has no room for an
  // inline error, and the picker has already closed by the time it lands.
  const shareFailed = (e: unknown) => void alert({ title: 'Could not share', message: (e as Error).message })
</script>

<MembersHeader
  {members}
  {teams}
  {active}
  {isOwner}
  loading={membersQuery.isLoading}
  directory={directory.data ?? []}
  noun="plan"
  onShareUser={(email) => void shareConversation(conversationId, email).then(refresh).catch(shareFailed)}
  onShareTeam={(teamId) => void shareConversationTeam(conversationId, teamId).then(refresh).catch(shareFailed)}
  onRemoveUser={(userId) => void unshareConversation(conversationId, userId).then(refresh)}
  onRemoveTeam={(teamId) => void unshareConversationTeam(conversationId, teamId).then(refresh)}
/>