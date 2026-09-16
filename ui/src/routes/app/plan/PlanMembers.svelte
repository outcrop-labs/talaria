<script lang="ts">
  import { useQueryClient, createQuery } from '@tanstack/svelte-query'
  import { UserPlus, Users, X } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import { alert } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { fade } from '@/lib/motion'
  import { getJson } from '@/lib/fetch-json'
  import { useSession } from '@/lib/session'
  import { sharePlan, sharePlanTeam, unsharePlan, unsharePlanTeam, usePlanMembers } from '@/lib/conversations.svelte'

  // Members + presence + share, in the plan chat header. Owner shares/removes;
  // a collaborator can leave. Green ring = viewing right now.
  let { planId }: { planId: string } = $props()

  const membersQuery = usePlanMembers(() => planId)
  const sessionQuery = useSession()
  const qc = useQueryClient()
  let adding = $state(false)
  let addingTeam = $state(false)
  const session = $derived(sessionQuery.data)
  const members = $derived(membersQuery.data?.members ?? [])
  const teams = $derived(membersQuery.data?.teams ?? [])
  const active = $derived(new Set(membersQuery.data?.active ?? []))
  const isOwner = $derived(!!session?.id && session.id === members.find((m) => m.role === 'owner')?.userId)
  const refresh = () => qc.invalidateQueries({ queryKey: ['plan-members', planId] })
  const dirQuery = createQuery(() => ({
    queryKey: ['teams-directory'],
    enabled: isOwner,
    queryFn: () => getJson<{ teams: Array<{ id: string; name: string }> }>('/api/teams/directory'),
  }))
  const teamOptions = $derived(
    (dirQuery.data?.teams ?? [])
      .filter((t) => !teams.some((g) => g.id === t.id))
      .map((t) => ({ value: t.id, label: t.name })),
  )
</script>

{#if membersQuery.isLoading}
  <!-- Hold the avatar-stack footprint while members load, so the header row
       (and the Draft-tickets button next to it) doesn't shift on resolve. -->
  <div class="flex items-center gap-1.5">
    <div class="flex -space-x-1.5">
      <Skeleton class="h-6 w-6 rounded-full ring-2 ring-surface" />
      <Skeleton class="h-6 w-6 rounded-full ring-2 ring-surface" />
    </div>
  </div>
{:else}
  <div class="flex items-center gap-1.5">
    <div class="flex -space-x-1.5">
      {#each members as m (m.userId)}
        <span class="group relative" title={`${m.name ?? m.email}${m.role === 'owner' ? ' (owner)' : ''}${active.has(m.userId) ? ', here now' : ''}`}>
          <Avatar
            name={m.name ?? m.email ?? '?'}
            class={cn('h-6 w-6 text-[10px] ring-2 ring-surface', active.has(m.userId) && 'ring-success')}
          />
          {#if (isOwner && m.role !== 'owner') || (m.userId === session?.id && m.role === 'collaborator')}
            <button
              type="button"
              title={m.userId === session?.id ? 'Leave this plan' : `Remove ${m.name ?? m.email}`}
              onclick={() => void unsharePlan(planId, m.userId).then(refresh)}
              class="absolute -right-1 -top-1 hidden h-3.5 w-3.5 place-items-center rounded-full bg-raised text-muted shadow group-hover:grid hover:text-fg"
            >
              <X size={9} />
            </button>
          {/if}
        </span>
      {/each}
    </div>
    {#each teams as t (t.id)}
      <span class="group relative flex items-center gap-0.5 rounded-full border border-line bg-raised px-1.5 py-0.5 font-mono text-[10px]" title={t.name}>
        <span class="max-w-24 truncate">{t.name}</span>
        {#if isOwner}
          <button
            type="button"
            title={`Remove ${t.name}`}
            onclick={() => void unsharePlanTeam(planId, t.id).then(refresh)}
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
              void sharePlan(planId, u.email)
                .then(refresh)
                .catch((e) => void alert({ title: 'Could not share', message: (e as Error).message }))
            }}
          />
        </div>
      {:else}
        <button
          type="button"
          title="Share this plan with a teammate"
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
              void sharePlanTeam(planId, id)
                .then(refresh)
                .catch((e) => void alert({ title: 'Could not share', message: (e as Error).message }))
            }}
          />
        </div>
      {:else}
        <button
          type="button"
          title="Share this plan with a team"
          onclick={() => (addingTeam = true)}
          class="grid h-6 w-6 place-items-center rounded-full border border-dashed border-line text-muted hover:text-fg"
        >
          <Users size={12} />
        </button>
      {/if}
    {/if}
  </div>
{/if}
