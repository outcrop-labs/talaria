<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Select from '@/components/ui/Select.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import { addTeamMember, removeTeamMember, useTeamMembers, type TeamRole } from '@/lib/teams'

  // Members panel of TeamsModal.svelte (module-private there in React; its own
  // file here because Svelte is one component per file).
  let { teamId, canManage }: { teamId: string; canManage: boolean } = $props()

  const qc = useQueryClient()
  // A failed member read used to render an empty roster next to a live "Add" —
  // the reading is "this team has nobody in it", and the fix people reach for
  // is re-adding people who are already there.
  const membersList = listQuery(useTeamMembers(() => teamId), { title: 'Could not load this team’s members', variant: 'compact' })
  const members = $derived(membersList.rows)
  const membersLoading = $derived(membersList.pending)
  let role = $state<TeamRole>('member')
  let err = $state<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['team-members', teamId] })

  const add = async (email: string) => {
    err = null
    const res = await addTeamMember(teamId, email, role)
    const data = (await res.json().catch(() => ({}))) as { error?: string }
    if (!res.ok || data.error) {
      err = data.error ?? 'Could not add'
      return
    }
    void refresh()
    void qc.invalidateQueries({ queryKey: ['teams'] })
  }
</script>

<div>
  {#if canManage}
    <div class="flex items-center gap-2">
      <UserPicker
        class="min-w-0 flex-1"
        size="sm"
        exclude={members.map((m) => m.userId)}
        onPick={(u) => {
          if (u.email) void add(u.email)
        }}
      />
      <Select bind:value={role} size="sm" class="shrink-0">
        <option value="member">Member</option>
        <option value="owner">Owner</option>
      </Select>
    </div>
  {/if}
  {#if err}<div class="mt-1 font-sans text-xs text-danger">{err}</div>{/if}
  {#if membersLoading}<SkeletonRows rows={3} avatar class="mt-4 px-1 py-2" />{/if}
  {#if membersList.notice}<div class="mt-4"><QueryError {...membersList.notice} /></div>{/if}
  <ul class="mt-4 space-y-1.5">
    {#each members as m (m.userId)}
      <li class="flex items-center gap-2 rounded-md px-1 py-2">
        <Avatar name={m.name ?? m.email} class="h-6 w-6" />
        <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">
          {m.name ?? m.email ?? m.userId}
          {#if m.name && m.email}<span class="ml-1.5 font-mono text-[11px] text-muted">{m.email}</span>{/if}
        </span>
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{m.role}</span>
        {#if canManage && m.role !== 'owner'}
          <button
            onclick={() => void removeTeamMember(teamId, m.userId).then(refresh)}
            class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger"
          >
            Remove
          </button>
        {/if}
      </li>
    {/each}
  </ul>
</div>
