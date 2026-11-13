<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { errorMessage, getList, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  /** Agent repo creation: the pending request queue across every git host
   *  (agents propose, THIS is where humans ratify — approval creates the repo
   *  on the host the request names, at the visibility chosen here, protects
   *  its default branch against force-push and deletion unless unticked, and
   *  grants it to the requester; a rejection posts its reason on the
   *  requesting ticket). Which orgs a host takes requests for is set on the
   *  host's own card. */
  const qc = useQueryClient()
  type Visibility = 'private' | 'public'
  interface RepoReq {
    id: string
    host: string
    agentModel: string
    org: string
    name: string
    why: string
    visibility?: Visibility
    appSlug?: string | null
    createdAt: string
  }
  // This is the humans-ratify queue. An empty render means "nothing waiting on
  // you" — a failed poll must never say that, or an agent's repo request sits
  // unseen for ever.
  const requestsQuery = createQuery(() => ({
    queryKey: ['repo-requests'],
    queryFn: (): Promise<RepoReq[]> => getList<RepoReq>('/api/workbench/repo-requests', 'requests'),
    refetchInterval: 60_000,
  }))
  const requests = $derived(requestsQuery.data ?? [])
  const requestsFailed = $derived(requestsQuery.isError && requestsQuery.data === undefined)
  // The approver's visibility per request; absent = what the agent asked for.
  let chosen = $state<Record<string, Visibility>>({})
  const visibilityOf = (r: RepoReq): Visibility => chosen[r.id] ?? r.visibility ?? 'private'
  // Default-branch protection (no force-push, no deletion — nothing else) is
  // on unless the approver unticks it.
  let unprotected = $state<Record<string, boolean>>({})
  let rejecting = $state<string | null>(null)
  let rejectNote = $state('')
  let decideError = $state<string | null>(null)
  /** The last approval's word on the default branch: a host may refuse to
   *  protect it (GitHub does for a private repo on a free plan), and the repo
   *  exists anyway. */
  let decided = $state<string | null>(null)
  const decide = async (r: RepoReq, action: 'approve' | 'reject') => {
    decideError = null
    decided = null
    try {
      const body =
        action === 'approve'
          ? { id: r.id, action, visibility: visibilityOf(r), protect: !unprotected[r.id] }
          : { id: r.id, action, ...(rejectNote.trim() ? { note: rejectNote.trim() } : {}) }
      const res = await putJson<{ ok: true; repo?: string; protected?: boolean; protection?: string | null }>(
        '/api/workbench/repo-requests',
        body,
      )
      if (action === 'approve' && res.repo) {
        decided = res.protected
          ? `${res.repo} created; its default branch is protected.`
          : `${res.repo} created; its default branch is not protected: ${res.protection ?? 'not asked for'}.`
      }
      if (action === 'reject') {
        rejecting = null
        rejectNote = ''
      }
    } catch (e) {
      decideError = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['repo-requests'] })
    await qc.invalidateQueries({ queryKey: ['workbench-flow'] })
  }
</script>

<SettingsSection
  title="Repository requests"
  description="New repositories your agents asked for, created and shared with the agent when you approve."
  info="Agents can ask only in the organizations a host allows, which you set on the host. A rejection posts its reason on the ticket that asked."
>
  <div class="space-y-3">
    {#if requestsFailed}
      <SettingsGroup>
        <QueryError
          variant="compact"
          error={requestsQuery.error}
          title="Could not load pending repo requests"
          onRetry={() => void requestsQuery.refetch()}
        />
      </SettingsGroup>
    {:else if requests.length === 0}
      <SettingsGroup>
        <SettingsRow label="No requests waiting" hint="A request shows here when an agent needs a new repository." />
      </SettingsGroup>
    {/if}
    {#each requests as r (r.id)}
      <SettingsGroup data-testid="repo-request">
        <SettingsRow>
          {#snippet label()}<span class="font-mono text-[13px] font-normal">{r.org}/{r.name}</span>{/snippet}
          {#snippet meta()}{#if r.appSlug}<Chip title="The Talaria app this repository will hold">App {r.appSlug}</Chip>{/if}{/snippet}
          {#snippet hint()}
            <span class="whitespace-normal">Asked for by {r.agentModel} on <span class="font-mono text-xs">{r.host}</span>. {r.why}</span>
          {/snippet}
        </SettingsRow>
        <SettingsRow label="Visibility" hint="Who can see the repository on the host." as="label">
          <Select
            size="sm"
            aria-label="Visibility"
            value={visibilityOf(r)}
            onchange={(e) => (chosen[r.id] = e.currentTarget.value as Visibility)}
            class="w-[280px]"
          >
            <option value="private">Private</option>
            <option value="public">Public</option>
          </Select>
        </SettingsRow>
        <SettingsRow label="Protect the default branch" hint="Blocks force-pushes and deletion, with no required reviews or checks.">
          <Toggle checked={!unprotected[r.id]} onChange={(on) => (unprotected[r.id] = !on)} title="Protect the default branch" />
        </SettingsRow>
        {#if rejecting === r.id}
          <div transition:slide={{ duration: 150 }}>
            <SettingsRow label="Reason" hint="Optional, and posted on the ticket that asked." as="label">
              <Input size="sm" bind:value={rejectNote} maxlength={500} placeholder="Why not?" class="w-[280px]" />
              <Button size="md" variant="outline" onclick={() => void decide(r, 'reject')}>Reject request</Button>
            </SettingsRow>
          </div>
        {/if}
        <div class="flex items-center justify-end gap-2 px-5 py-3">
          <Button
            size="sm"
            variant="ghost"
            onclick={() => {
              rejecting = rejecting === r.id ? null : r.id
              rejectNote = ''
            }}>{rejecting === r.id ? 'Keep request' : 'Reject'}</Button
          >
          <Button size="sm" variant="outline" onclick={() => void decide(r, 'approve')}>Create repository</Button>
        </div>
      </SettingsGroup>
    {/each}
    {#if decideError || decided}
      <div transition:slide={{ duration: 150 }}>
        <SettingsGroup>
          {#if decideError}<NoticeRow status="danger">{decideError}</NoticeRow>{/if}
          {#if decided}<NoticeRow status="ok" data-testid="repo-request-decided">{decided}</NoticeRow>{/if}
        </SettingsGroup>
      </div>
    {/if}
  </div>
</SettingsSection>
