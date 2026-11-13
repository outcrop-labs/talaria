<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { GitBranch } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { decideInstallRequest, fetchInstallRequests, type AppInstallRequest, type KeptDataConflict } from './apps'

  // Agents' requests to install the app they built (`request_app_install`).
  // THIS is where an admin decides: Install runs the same credentialed
  // install as Install from Git and records the plugin (the agent maintains
  // it, the ticket's person owns it, the ticket's board takes its
  // maintenance tickets). The app lands disabled — enabling it and granting
  // its surfaces stay here, on its card. Every outcome is a line on the
  // requesting ticket.
  const qc = useQueryClient()
  // A failed poll must never read as "nothing waiting on you".
  const query = createQuery(() => ({
    queryKey: ['app-install-requests'],
    queryFn: fetchInstallRequests,
    refetchInterval: 60_000,
  }))
  const requests = $derived(query.data ?? [])
  /** The app's short name: the last segment of its repository path. */
  const nameOf = (r: AppInstallRequest) => r.repo.split('/').pop() || r.repo
  let busy = $state<string | null>(null)
  let rejecting = $state<string | null>(null)
  let rejectNote = $state('')
  /** The last decision's outcome, per request, as it was written on the ticket. */
  let outcome = $state<{ ok: boolean; text: string } | null>(null)
  /** Kept data from another repository stands in the way: adopt or wipe. */
  let conflicts = $state<Record<string, KeptDataConflict>>({})

  const decide = async (r: AppInstallRequest, action: 'approve' | 'reject', keptData?: 'adopt' | 'wipe') => {
    busy = r.id
    outcome = null
    try {
      const body =
        action === 'approve'
          ? { id: r.id, action, ...(keptData ? { keptData } : {}) }
          : { id: r.id, action, ...(rejectNote.trim() ? { note: rejectNote.trim() } : {}) }
      const res = await decideInstallRequest(body)
      if (res.conflict) {
        conflicts[r.id] = res.conflict
        return
      }
      delete conflicts[r.id]
      outcome = { ok: res.ok, text: res.warning ? `${res.line} (${res.warning})` : res.line }
      if (action === 'reject') {
        rejecting = null
        rejectNote = ''
      }
    } catch (e) {
      outcome = { ok: false, text: errorMessage(e) }
    } finally {
      busy = null
      await qc.invalidateQueries({ queryKey: ['app-install-requests'] })
      await qc.invalidateQueries({ queryKey: ['admin-apps'] })
      await qc.invalidateQueries({ queryKey: ['apps'] })
    }
  }
</script>

{#if query.isError && query.data === undefined}
  <QueryError
    variant="inline"
    error={query.error}
    title="Could not load install requests"
    onRetry={() => void query.refetch()}
  />
{/if}
{#if requests.length > 0 || outcome}
  <div data-install-requests>
    <SettingsSection title="Install requests" description="Apps your agents built and want installed. An approved app starts disabled.">
      {#if requests.length > 0}
        <SettingsGroup>
          {#each requests as r (r.id)}
            {@const conflict = conflicts[r.id]}
            <div data-install-request>
              <SettingsRow label={nameOf(r)}>
                {#snippet icon()}<GitBranch size={16} />{/snippet}
                {#snippet meta()}
                  <span class="min-w-0 truncate font-mono text-xs text-muted" title={r.url}>{r.repo} @ {r.ref ?? 'default branch'} on {r.hostId}</span>
                {/snippet}
                {#snippet hint()}
                  {#if r.why}<div>{r.why}</div>{/if}
                  <div>
                    Requested by <span title="The agent that asked, and will maintain it">{r.agentName ?? r.agentModel}</span>{#if r.ownerName}{' '}for <span title="Who the app is for">{r.ownerName}</span>{/if}{#if r.ticketRef}<span
                        class="ml-2 font-mono text-xs"
                        title={`The ticket it was built for, on ${r.boardName ?? 'its board'}. Maintenance tickets go there.`}>{r.ticketRef}</span
                      >{/if}
                  </div>
                {/snippet}
                <Button size="sm" variant="outline" disabled={busy === r.id} onclick={() => void decide(r, 'approve')}>
                  {busy === r.id ? 'Installing' : 'Install'}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy === r.id}
                  onclick={() => {
                    rejecting = rejecting === r.id ? null : r.id
                    rejectNote = ''
                  }}>Reject</Button
                >
              </SettingsRow>
              {#if conflict}
                <div transition:slide={{ duration: 150 }}>
                  <div class="flex flex-wrap items-center gap-3 border-t border-line-subtle px-5 py-4" data-install-conflict>
                    <span class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-warning">
                      The name "{conflict.slug}" still holds data from {conflict.keptFrom ?? 'an earlier install that did not record its repository'}.
                    </span>
                    <Button size="sm" variant="outline" disabled={busy === r.id} onclick={() => void decide(r, 'approve', 'adopt')}>Keep that data</Button>
                    <Button size="sm" variant="ghost" disabled={busy === r.id} onclick={() => void decide(r, 'approve', 'wipe')}>Delete it first</Button>
                  </div>
                </div>
              {/if}
              {#if rejecting === r.id}
                <div transition:slide={{ duration: 150 }}>
                  <div class="flex items-center gap-3 border-t border-line-subtle px-5 py-4">
                    <Input
                      size="sm"
                      bind:value={rejectNote}
                      maxlength={500}
                      placeholder="Reason, posted on the ticket (optional)"
                      class="min-w-0 flex-1"
                    />
                    <Button size="md" variant="outline" disabled={busy === r.id} onclick={() => void decide(r, 'reject')}>Reject request</Button>
                  </div>
                </div>
              {/if}
            </div>
          {/each}
        </SettingsGroup>
      {/if}
      {#if outcome}
        <div
          transition:slide={{ duration: 150 }}
          class={`font-sans text-[13px] leading-5 ${requests.length > 0 ? 'mt-2' : ''} ${outcome.ok ? 'text-muted' : 'text-danger'}`}
          data-install-outcome
        >
          {outcome.text}
        </div>
      {/if}
    </SettingsSection>
  </div>
{/if}
