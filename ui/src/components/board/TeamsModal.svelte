<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Modal from '@/components/ui/Modal.svelte'
  import InlineCreate from '@/components/ui/InlineCreate.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { cn } from '@/lib/cn'
  import { createTeam, useTeams } from '@/lib/teams'
  import TeamMembers from './TeamMembers.svelte'

  // Create teams and manage their members. Team members can access all of a team's
  // boards.
  let { open, onClose }: { open: boolean; onClose: () => void } = $props()

  const qc = useQueryClient()
  // `{ data: teams = [] }` turned a 500 on /api/teams into "Create a team to
  // get started" — an invitation to duplicate teams that already exist.
  const teamsList = listQuery(useTeams(), { title: 'Could not load your teams', variant: 'compact' })
  const teams = $derived(teamsList.rows)
  const teamsLoading = $derived(teamsList.pending)
  let selected = $state<string | null>(null)

  const team = $derived(teams.find((t) => t.id === selected) ?? teams[0] ?? null)
  const activeId = $derived(team?.id ?? null)

  const create = async (name: string) => {
    const { team: t } = await createTeam(name)
    await qc.invalidateQueries({ queryKey: ['teams'] })
    selected = t.id
  }
</script>

<Modal {open} {onClose} title="Teams" width="max-w-2xl">
  <div class="flex min-h-[24rem] gap-7">
    <!-- Team list -->
    <div class="flex w-60 shrink-0 flex-col border-r border-line-subtle pr-6">
      <div class="space-y-1.5">
        {#if teamsLoading}<SkeletonRows rows={3} class="px-2 py-1.5" />{/if}
        {#if teamsList.notice}<QueryError {...teamsList.notice} />{/if}
        {#each teams as t (t.id)}
          <button
            onclick={() => (selected = t.id)}
            class={cn(
              'flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left font-sans text-sm transition-colors hover:bg-hover',
              t.id === activeId ? 'bg-raised text-fg' : 'text-muted',
            )}
          >
            <span class="min-w-0 flex-1 truncate">{t.name}</span>
            <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{t.memberCount}</span>
          </button>
        {/each}
      </div>
      <InlineCreate label="New team" placeholder="New team" onSubmit={(n) => void create(n)} class="mt-auto pt-3" />
    </div>

    <!-- Members of the selected team -->
    <div class="min-w-0 flex-1">
      {#if team}
        <TeamMembers teamId={activeId!} canManage={team.role === 'owner'} />
      {:else if teamsLoading}
        <SkeletonRows rows={3} avatar class="pt-1" />
      {:else if teamsList.failed}
        <!-- The team list is unknown, so "create one" is the wrong prompt —
             the left column already carries the error and its Retry. -->
        <div class="grid h-40 place-items-center px-4 text-center text-sm text-muted">
          We couldn’t read your teams, so there’s nothing to show here yet.
        </div>
      {:else}
        <div class="grid h-40 place-items-center font-sans text-sm text-muted">Create a team to get started.</div>
      {/if}
    </div>
  </div>
</Modal>
