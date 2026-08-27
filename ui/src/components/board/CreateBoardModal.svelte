<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate } from '@/router'
  import Modal from '@/components/ui/Modal.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import UserPicker from '@/components/app/UserPicker.svelte'
  import { useAgents } from '@/lib/agents'
  import { useTeams } from '@/lib/teams'
  import { createBoard, setBoardAgents, shareBoard } from '@/lib/boards.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'

  type Invite = { email: string; role: 'editor' | 'viewer' }

  // Create a board and configure everything up front: owner (personal/team),
  // which agents may work it (restrictive by default — opt into "all agents"),
  // and who to invite.
  let { open, onClose }: { open: boolean; onClose: () => void } = $props()

  const qc = useQueryClient()
  const fleetQuery = useAgents()
  const fleetLoading = $derived(fleetQuery.isLoading)
  // A failed /api/teams used to leave the owner picker holding "Personal" only
  // — which is a real, wrong answer ("you're on no teams"), not a blank.
  const teamsList = listQuery(useTeams(), { title: 'Could not load your teams', variant: 'inline' })
  const teams = $derived(teamsList.rows)
  const teamsLoading = $derived(teamsList.pending)
  const agentOptions = $derived((fleetQuery.data?.agents ?? []).map((a) => ({ value: a.id, label: a.label, sub: a.role })))

  let name = $state('')
  let teamId = $state('')
  let allowAll = $state(false)
  let agents = $state<string[]>([])
  let invites = $state<Invite[]>([])
  let role = $state<'editor' | 'viewer'>('editor')
  let busy = $state(false)

  const close = () => {
    name = ''
    teamId = ''
    allowAll = false
    agents = []
    invites = []
    onClose()
  }

  const addInvite = (email: string) => {
    const e = email.trim().toLowerCase()
    if (!e || invites.some((i) => i.email === e)) return
    invites = [...invites, { email: e, role }]
  }

  const create = async () => {
    const n = name.trim()
    if (!n) return
    busy = true
    try {
      const { board } = await createBoard(n, teamId || null)
      await setBoardAgents(board.id, allowAll, allowAll ? [] : agents)
      // An invite failing (bad address, already a member) must not abort the
      // board that just got created — swallow per-invite, deliberately.
      for (const inv of invites) await shareBoard(board.id, inv.email, inv.role).catch(() => {})
      await qc.invalidateQueries({ queryKey: ['boards'] })
      close()
      void navigate('/boards/:boardId', { params: { boardId: board.id } })
    } catch (e) {
      pushToast({ title: 'Could not create the board', body: errorMessage(e), tone: 'danger' })
    } finally {
      busy = false
    }
  }
</script>

<Modal {open} onClose={close} title="New board" width="max-w-lg">
  {#snippet footer()}
    <div class="flex items-center justify-between">
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{allowAll ? 'All agents allowed' : `${agents.length} agents`}</span>
      <div class="flex gap-2">
        <Button variant="ghost" size="sm" onclick={close}>Cancel</Button>
        <Button size="sm" onclick={() => void create()} disabled={busy || !name.trim()}>Create board</Button>
      </div>
    </div>
  {/snippet}
  <div class="space-y-4">
    <!-- (The React `Field` wrapper is inlined: label div + content.) -->
    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</div>
      <Input
        autofocus
        bind:value={name}
        onkeydown={(e) => {
          if (e.key === 'Enter') void create()
        }}
        placeholder="e.g. Q3 Launch"
        class="w-full"
      />
    </div>

    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Owner</div>
      {#if teamsLoading}
        <Skeleton class="h-11 w-full" />
      {:else}
        <Select bind:value={teamId} class="w-full">
          <option value="">Personal</option>
          {#each teams as t (t.id)}
            <option value={t.id}>{t.name}</option>
          {/each}
        </Select>
      {/if}
      {#if teamsList.notice}<QueryError {...teamsList.notice} />{/if}
    </div>

    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agents</div>
      <label class="mb-2 flex cursor-pointer items-center gap-2 font-sans text-sm text-fg">
        <input type="checkbox" bind:checked={allowAll} class="accent-[color:var(--theme-accent)]" />
        Allow all agents
      </label>
      {#if !allowAll}
        {#if fleetLoading}
          <Skeleton class="h-11 w-full" />
        {:else}
          <Combobox options={agentOptions} selected={agents} onChange={(next) => (agents = next)} multiple placeholder="Select agents" />
        {/if}
      {/if}
    </div>

    <div>
      <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Invite (optional)</div>
      <div class="flex items-center gap-2">
        <UserPicker class="min-w-0 flex-1" size="sm" onPick={(u) => u.email && addInvite(u.email)} />
        <Select bind:value={role} size="sm" class="shrink-0">
          <option value="editor">Editor</option>
          <option value="viewer">Viewer</option>
        </Select>
      </div>
      {#if invites.length > 0}
        <div class="mt-2 flex flex-wrap gap-1.5">
          {#each invites as i (i.email)}
            <span class="flex items-center gap-1 rounded-full border border-line px-2 py-0.5 font-mono text-[10px] tracking-[0.05em] text-muted">
              {i.email} · {i.role}
              <button onclick={() => (invites = invites.filter((x) => x.email !== i.email))} class="transition-colors hover:text-danger">✕</button>
            </span>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</Modal>
