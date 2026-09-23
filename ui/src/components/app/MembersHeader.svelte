<script lang="ts">
  import { UserPlus, Users, X } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import { cn } from '@/lib/cn'
  import { fade } from '@/lib/motion'
  import { useSession } from '@/lib/session'

  // THE MEMBERS ROW both shared surfaces wear in their header: the plan chat
  // and the research run. The owner shares/removes, a collaborator can leave,
  // and the pickers swap in place.
  //
  // The surfaces genuinely differ in THREE places, and each is a prop rather
  // than a second copy of this markup:
  //   · `chipSurface` — the remove chip and the team pill sit on the plan
  //     header's raised tile, but on the research header's card.
  //   · `active` — only the plan pings presence, so only it draws the green
  //     ring (and says ", here now"). Absent = a surface that tracks nobody.
  //   · `hideWhenOwnerless` — an org-wide run was started by an agent and has
  //     no human owner, so it has nothing to share; a plan always has one.
  // `noun` is what the row belongs to, in the row's own copy ("Leave this
  // plan" / "Leave this research") — the two surfaces name different things.
  interface Member {
    userId: string
    name: string | null
    email: string | null
    role: 'owner' | 'collaborator'
  }
  interface MemberTeam {
    id: string
    name: string
  }
  /** The token a chip fills with, named for the surface it is drawn on. */
  const CHIP: Record<'raised' | 'card', string> = { raised: 'bg-raised', card: 'bg-card' }

  let {
    members,
    teams,
    active = new Set<string>(),
    isOwner,
    onShareUser,
    onShareTeam,
    onRemoveUser,
    onRemoveTeam,
    hideWhenOwnerless = false,
    loading = false,
    chipSurface = 'raised',
    noun,
    directory,
  }: {
    members: Member[]
    teams: MemberTeam[]
    /** Who is looking right now — the ring, and the `, here now` title. */
    active?: Set<string>
    isOwner: boolean
    onShareUser: (email: string) => void
    onShareTeam: (teamId: string) => void
    onRemoveUser: (userId: string) => void
    onRemoveTeam: (teamId: string) => void
    hideWhenOwnerless?: boolean
    /** Hold the avatar stack's footprint while the members load. */
    loading?: boolean
    /** The surface the chips are drawn on. */
    chipSurface?: 'raised' | 'card'
    /** What this row belongs to, in the copy: "Leave this <noun>". */
    noun: string
    /** Every team the org has, for the share-with-a-team picker. Only the ones
     *  not already on this row are offered. */
    directory: Array<{ id: string; name: string }>
  } = $props()

  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const teamOptions = $derived(
    directory.filter((t) => !teams.some((g) => g.id === t.id)).map((t) => ({ value: t.id, label: t.name })),
  )
  let adding = $state(false)
  let addingTeam = $state(false)
</script>

{#if loading}
  <!-- Hold the avatar-stack footprint while members load, so the header row
       (and the Draft-tickets button next to it) doesn't shift on resolve. -->
  <span class="flex items-center gap-1.5">
    <span class="flex -space-x-1.5">
      <Skeleton class="h-6 w-6 rounded-full ring-2 ring-surface" />
      <Skeleton class="h-6 w-6 rounded-full ring-2 ring-surface" />
    </span>
  </span>
{:else if !hideWhenOwnerless || (members.length > 0 && members.some((m) => m.role === 'owner'))}
  <span class="flex items-center gap-1.5">
    <span class="flex -space-x-1.5">
      {#each members as m (m.userId)}
        <span class="group relative" title={`${m.name ?? m.email}${m.role === 'owner' ? ' (owner)' : ''}${active.has(m.userId) ? ', here now' : ''}`}>
          <Avatar
            name={m.name ?? m.email ?? '?'}
            class={cn('h-6 w-6 text-[10px] ring-2 ring-surface', active.has(m.userId) && 'ring-success')}
          />
          {#if (isOwner && m.role !== 'owner') || (m.userId === session?.id && m.role === 'collaborator')}
            <button
              type="button"
              title={m.userId === session?.id ? `Leave this ${noun}` : `Remove ${m.name ?? m.email}`}
              onclick={() => onRemoveUser(m.userId)}
              class={cn(
                'absolute -right-1 -top-1 hidden h-3.5 w-3.5 place-items-center rounded-full text-muted shadow group-hover:grid hover:text-fg',
                CHIP[chipSurface],
              )}
            >
              <X size={9} />
            </button>
          {/if}
        </span>
      {/each}
    </span>
    {#each teams as t (t.id)}
      <span
        class={cn('group relative flex items-center gap-0.5 rounded-full border border-line px-1.5 py-0.5 font-mono text-[10px]', CHIP[chipSurface])}
        title={t.name}
      >
        <span class="max-w-24 truncate">{t.name}</span>
        {#if isOwner}
          <button
            type="button"
            title={`Remove ${t.name}`}
            onclick={() => onRemoveTeam(t.id)}
            class="hidden h-3.5 w-3.5 place-items-center rounded-full text-muted group-hover:grid hover:text-fg"
          >
            <X size={9} />
          </button>
        {/if}
      </span>
    {/each}
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
              onShareUser(u.email)
            }}
          />
        </div>
      {:else}
        <button
          type="button"
          title={`Share this ${noun} with a teammate`}
          onclick={() => (adding = true)}
          class="grid h-6 w-6 place-items-center rounded-full border border-dashed border-line text-muted hover:text-fg"
        >
          <UserPlus size={12} />
        </button>
      {/if}
      {#if addingTeam}
        <div in:fade={{ duration: 150 }}>
          <Combobox
            size="sm"
            class="w-48"
            options={teamOptions}
            selected={[]}
            placeholder="Share with a team"
            onChange={(next) => {
              addingTeam = false
              const id = next[0]
              if (!id) return
              onShareTeam(id)
            }}
          />
        </div>
      {:else}
        <button
          type="button"
          title={`Share this ${noun} with a team`}
          onclick={() => (addingTeam = true)}
          class="grid h-6 w-6 place-items-center rounded-full border border-dashed border-line text-muted hover:text-fg"
        >
          <Users size={12} />
        </button>
      {/if}
    {/if}
  </span>
{/if}