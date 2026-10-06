<script lang="ts">
  import { Clock, Mail, MessageSquare, X } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { DOT_COLOR } from '@/components/ui/chip'
  import { slide, GROW_X } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import { agentPresence, type Presence } from '@/lib/comms-sidebar'
  import { firstName, localTimeLabel, presenceLabel, type ProfileSubject } from '@/lib/comms-profile'
  import { useConversationsWith } from '@/lib/comms-api'
  import type { CommsSelection } from '@/lib/comms-selection'
  import type { AgentModel } from '@/lib/agents'
  import type { AgentStatus } from '@/lib/fleet'
  import type { DirectoryUser } from '@/lib/users'
  import SharedConversationList from './SharedConversationList.svelte'

  // The profile drawer (a right-side "Profile" panel, in Talaria's
  // grammar): who someone is — photo, name, title, presence, status, their
  // local time — a Message action, how to reach them, and the conversations
  // you share, with a way through to all of them. A person and an agent use
  // the same drawer; an agent has a role instead of a title and no contact
  // section.
  //
  // An OVERLAY anchored to the main pane's right edge, above an open thread
  // panel (whose state lives inside ChannelView, out of Comms' reach). It
  // grows in on ThreadPanel's GROW_X leg, with the same 380px resting width.
  let {
    subject,
    users,
    usersLoading,
    fleet,
    fleetStatus,
    onClose,
    onMessage,
    onOpen,
    onSeeAll,
  }: {
    subject: ProfileSubject
    users: DirectoryUser[]
    /** The directory has not answered yet — hold the header's shape. */
    usersLoading: boolean
    fleet: AgentModel[]
    fleetStatus: Map<string, AgentStatus>
    onClose: () => void
    /** Open (or start) the DM with them — Comms owns the navigation. */
    onMessage: () => void
    onOpen: (sel: CommsSelection) => void
    onSeeAll: () => void
  } = $props()

  const person = $derived(subject.kind === 'person' ? (users.find((u) => u.id === subject.userId) ?? null) : null)
  const agent = $derived(subject.kind === 'agent' ? (fleet.find((a) => a.id === subject.model) ?? null) : null)

  const name = $derived(
    subject.kind === 'person'
      ? (person?.name || person?.email || 'Someone')
      : (agent?.label ?? subject.model),
  )
  // An agent's role comes from its id's tail ("atlas-operations" → "operations"),
  // so it is capitalized to read like a title.
  const agentRole = $derived(agent?.role ? agent.role.charAt(0).toUpperCase() + agent.role.slice(1) : null)
  const subtitle = $derived(subject.kind === 'person' ? (person?.title ?? null) : agentRole)
  const presence: Presence = $derived(
    subject.kind === 'person' ? (person?.online ? 'online' : 'offline') : agentPresence(fleetStatus.get(subject.model)),
  )
  const first = $derived(
    subject.kind === 'person' ? firstName(person?.name, person?.email) : firstName(agent?.label ?? subject.model, null),
  )
  // Held while the directory is still in flight; a person it does not know
  // once it HAS answered is gone (or never signed in).
  const pending = $derived(subject.kind === 'person' && !person && usersLoading)
  const missing = $derived(subject.kind === 'person' && !person && !usersLoading)

  // Their clock, re-read every 30s while the drawer is open.
  let now = $state(new Date())
  $effect(() => {
    const t = setInterval(() => (now = new Date()), 30_000)
    return () => clearInterval(t)
  })
  const localTime = $derived(subject.kind === 'person' ? localTimeLabel(person?.timezone, now) : null)

  const conversationsQuery = useConversationsWith(() => subject)
  const agentModel = $derived(subject.kind === 'agent' ? subject.model : null)

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape' && !e.defaultPrevented) onClose()
  }
</script>

<svelte:window onkeydown={onKey} />

<aside
  transition:slide|global={GROW_X}
  aria-label={`Profile: ${name}`}
  class="absolute inset-y-0 right-0 z-20 border-l border-line bg-panel shadow-lg"
>
  <div class="flex h-full w-[380px] flex-col">
    <div class="flex items-center gap-2 border-b border-line px-4 py-2.5">
      <span class="font-sans text-sm font-semibold text-fg">Profile</span>
      <button
        type="button"
        onclick={onClose}
        class="ml-auto grid h-7 w-7 place-items-center rounded-md text-muted transition-colors dither-fill hover:text-fg"
        title="Close profile"
      >
        <X size={14} />
      </button>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto px-5 py-5">
      {#if pending}
        <div aria-hidden="true" class="space-y-4">
          <Skeleton class="aspect-square w-full max-w-[220px] rounded-lg" />
          <Skeleton class="h-4 w-40 rounded-full" />
          <Skeleton class="h-3 w-28 rounded-full" />
          <Skeleton class="h-3 w-20 rounded-full" />
          <Skeleton class="h-8 w-28 rounded-md" />
        </div>
      {:else if missing}
        <EmptyState
          variant="compact"
          icon="◎"
          title="Profile not found"
          hint="This person may have left the workspace."
        />
      {:else}
        <Avatar
          src={person?.picture}
          {name}
          class="aspect-square h-auto w-full max-w-[220px] rounded-lg text-4xl"
        />

        <h2 class="mt-4 font-sans text-xl font-semibold text-fg">{name}</h2>
        {#if subtitle}<p class="mt-0.5 font-sans text-sm text-muted">{subtitle}</p>{/if}

        <div class="mt-3 space-y-1.5 font-sans text-sm">
          <div class="flex items-center gap-2 text-fg">
            <span
              aria-hidden="true"
              class={cn('h-2 w-2 shrink-0 rounded-full', presence === 'online' ? '' : 'border border-muted bg-panel')}
              style:background={presence === 'online' ? DOT_COLOR.ok : undefined}
            ></span>
            <span>{presenceLabel(presence)}</span>
          </div>
          {#if person?.statusEmoji || person?.statusText}
            <div class="flex min-w-0 items-center gap-2 text-muted">
              {#if person.statusEmoji}<span class="w-4 shrink-0 text-center">{person.statusEmoji}</span>{/if}
              {#if person.statusText}<span class="min-w-0 truncate">{person.statusText}</span>{/if}
            </div>
          {/if}
          {#if localTime}
            <div class="flex items-center gap-2 text-muted">
              <Clock size={14} class="shrink-0" />
              <span>{localTime}</span>
            </div>
          {/if}
        </div>

        <div class="mt-4 flex items-center gap-2">
          <Button variant="outline" size="sm" onclick={onMessage}>
            <MessageSquare size={14} />
            Message
          </Button>
        </div>

        {#if person?.email}
          <section class="mt-6 border-t border-line-subtle pt-4">
            <h3 class="mb-2 text-[10px] font-semibold uppercase tracking-wide text-muted">Contact information</h3>
            <a
              href={`mailto:${person.email}`}
              class="flex min-w-0 items-center gap-2.5 rounded-md py-1 font-sans text-sm text-fg hover:underline"
            >
              <span class="grid h-7 w-7 shrink-0 place-items-center rounded-md border border-line bg-raised text-muted">
                <Mail size={14} />
              </span>
              <span class="min-w-0">
                <span class="block text-xs text-muted">Email address</span>
                <span class="block truncate text-accent">{person.email}</span>
              </span>
            </a>
          </section>
        {/if}

        <section class="mt-6 border-t border-line-subtle pt-4">
          <h3 class="mb-1 text-[10px] font-semibold uppercase tracking-wide text-muted">Conversations</h3>
          <SharedConversationList
            query={conversationsQuery}
            limit={3}
            subjectPicture={person?.picture ?? null}
            {agentModel}
            subjectName={first}
            {onOpen}
            dense
          />
          {#if (conversationsQuery.data?.length ?? 0) > 0}
            <Button variant="link" class="mt-2" onclick={onSeeAll}>
              See all conversations with {first}
            </Button>
          {/if}
        </section>
      {/if}
    </div>
  </div>
</aside>
