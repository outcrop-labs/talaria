<script lang="ts">
  import Avatar from '@/components/ui/Avatar.svelte'
  import { firstName, type ProfileSubject } from '@/lib/comms-profile'
  import { useConversationsWith } from '@/lib/comms-api'
  import type { CommsSelection } from '@/lib/comms-selection'
  import type { AgentModel } from '@/lib/agents'
  import type { DirectoryUser } from '@/lib/users'
  import SharedConversationList from './SharedConversationList.svelte'

  // /comms/with/<person|agent>/<id>: every conversation you share with one
  // person or agent — DMs, channels, relays and (for an agent) your threads
  // with it — newest activity first. The profile drawer's "See all
  // conversations" lands here; each row opens its conversation.
  let {
    subject,
    users,
    fleet,
    onOpen,
    onOpenProfile,
  }: {
    subject: ProfileSubject
    users: DirectoryUser[]
    fleet: AgentModel[]
    onOpen: (sel: CommsSelection) => void
    /** The header's name opens their profile drawer. */
    onOpenProfile: () => void
  } = $props()

  const person = $derived(subject.kind === 'person' ? (users.find((u) => u.id === subject.userId) ?? null) : null)
  const agent = $derived(subject.kind === 'agent' ? (fleet.find((a) => a.id === subject.model) ?? null) : null)
  const name = $derived(
    subject.kind === 'person' ? (person?.name || person?.email || 'someone') : (agent?.label ?? subject.model),
  )
  const first = $derived(
    subject.kind === 'person' ? firstName(person?.name, person?.email) : firstName(agent?.label ?? subject.model, null),
  )
  const conversationsQuery = useConversationsWith(() => subject)
</script>

<div class="flex h-full min-h-0 flex-col">
  <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
    <Avatar {name} src={person?.picture} class="h-5 w-5 text-[9px]" />
    <span class="text-sm font-semibold text-fg">
      Conversations with
      <button type="button" class="font-semibold hover:underline" onclick={onOpenProfile}>{name}</button>
    </span>
  </header>
  <div class="min-h-0 flex-1 overflow-y-auto">
    <SharedConversationList
      query={conversationsQuery}
      subjectName={first}
      subjectPicture={person?.picture ?? null}
      agentModel={subject.kind === 'agent' ? subject.model : null}
      {onOpen}
    />
  </div>
</div>
