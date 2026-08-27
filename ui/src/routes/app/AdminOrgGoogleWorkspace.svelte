<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage, getJson, postJson } from '@/lib/fetch-json'

  // The provisioned workspace: the org calendar + Shared Drive everyone at the
  // domain can reach, and the address each org agent sends mail from. One GET
  // paints it all; the buttons run the idempotent provisioning pass.

  interface ProvisionState {
    readiness: { connected: boolean; email: string | null; calendarScope: boolean; driveScope: boolean }
    orgEmail: string | null
    calendarId: string | null
    sharedDriveId: string | null
    agents: Array<{ model: string; displayName: string; slug: string; alias: string | null; effective: string | null }>
  }
  type Outcome = { ok: true; state: string; id: string } | { ok: false; error: string; message: string }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['org-google-provision'],
    queryFn: (): Promise<ProvisionState> => getJson<ProvisionState>('/api/integrations/google/org/provision'),
  }))
  const data = $derived(query.data)

  let working = $state<'calendar' | 'drive' | null>(null)
  let results = $state<{ calendar?: Outcome; drive?: Outcome }>({})

  const run = async (what: 'calendar' | 'drive') => {
    working = what
    let j: { calendar?: Outcome; drive?: Outcome }
    try {
      j = await postJson<{ calendar?: Outcome; drive?: Outcome }>('/api/integrations/google/org/provision', { [what]: true })
    } catch (e) {
      // Per-item failures ride in a 200 body; a throw means the call itself
      // failed — show it as that item's outcome.
      const failed: Outcome = { ok: false, error: '', message: errorMessage(e) }
      j = what === 'calendar' ? { calendar: failed } : { drive: failed }
    }
    working = null
    results = { ...results, ...j }
    // Targets changed on success — the panel's connection/targets views follow.
    if (j[what]?.ok) await qc.invalidateQueries({ queryKey: ['org-google'] })
    await qc.invalidateQueries({ queryKey: ['org-google-provision'] })
  }

  const outcomeLine = (o: Outcome | undefined): { text: string; good: boolean } | null => {
    if (!o) return null
    if (o.ok) {
      const label = o.state === 'created' ? 'Created.' : o.state === 'shared' ? 'Shared with the domain.' : 'Already in place.'
      return { text: label, good: true }
    }
    return { text: o.message, good: false }
  }
  const driveOutcome = $derived(outcomeLine(results.drive))
  const calendarOutcome = $derived(outcomeLine(results.calendar))
</script>

{#if query.isPending}
  <div class="mt-4 space-y-1.5">
    <Skeleton class="h-3 w-44 rounded-full" />
    <Skeleton class="h-3 w-56 rounded-full" />
    <Skeleton class="h-3 w-40 rounded-full" />
  </div>
{:else if data}
  <div class="mt-4 space-y-1.5">
    <div class="flex items-center gap-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Workspace resources</span>
    </div>

    {#if data.readiness.connected && (!data.readiness.calendarScope || !data.readiness.driveScope)}
      <!-- The one-time wall: a connection granted before the provisioning scopes
           existed. Re-consent (the connect flow upserts over the live row). -->
      <div class="rounded-md border border-danger/40 px-3 py-2 text-xs text-danger">
        Reconnect once to grant provisioning scopes:
        <a href="/api/integrations/google/org/connect" class="text-accent hover:underline">reconnect the org account</a>
        and the calendar + shared Drive can be created here.
      </div>
    {/if}

    <!-- Org calendar -->
    <div class="flex items-center gap-2 text-xs">
      <StatusDot status={data.calendarId ? 'ok' : 'idle'} />
      <span class="text-fg">Org calendar</span>
      {#if data.calendarId}
        <a
          href={`https://calendar.google.com/calendar/u/0/r?cid=${encodeURIComponent(data.calendarId)}`}
          target="_blank"
          rel="noreferrer"
          class="truncate font-mono text-muted hover:text-accent hover:underline"
        >{data.calendarId} ↗</a>
      {:else}
        <span class="text-muted">not set up (everyone at the domain gets edit access)</span>
      {/if}
      {#if data.readiness.connected}
        <span class="ml-auto shrink-0">
          <Button variant="ghost" size="sm" onclick={() => void run('calendar')} disabled={working !== null}>
            {working === 'calendar' ? 'Setting up…' : data.calendarId ? 'Re-check' : 'Set up'}
          </Button>
        </span>
      {/if}
    </div>
    {#if calendarOutcome}
      <div class={cn('pl-3 text-xs', calendarOutcome.good ? 'text-success' : 'text-danger')}>
        {calendarOutcome.text || 'done'}
      </div>
    {/if}

    <!-- Shared Drive -->
    <div class="flex items-center gap-2 text-xs">
      <StatusDot status={data.sharedDriveId ? 'ok' : 'idle'} />
      <span class="text-fg">Shared Drive</span>
      {#if data.sharedDriveId}
        <a
          href={`https://drive.google.com/drive/folders/${data.sharedDriveId}`}
          target="_blank"
          rel="noreferrer"
          class="truncate font-mono text-muted hover:text-accent hover:underline"
        >open ↗</a>
      {:else}
        <span class="text-muted">not set up (team-owned files, domain-wide access)</span>
      {/if}
      {#if data.readiness.connected}
        <span class="ml-auto shrink-0">
          <Button variant="ghost" size="sm" onclick={() => void run('drive')} disabled={working !== null}>
            {working === 'drive' ? 'Setting up…' : data.sharedDriveId ? 'Re-check' : 'Set up'}
          </Button>
        </span>
      {/if}
    </div>
    {#if driveOutcome}
      <div class={cn('pl-3 text-xs', driveOutcome.good ? 'text-success' : 'text-danger')}>
        {driveOutcome.text || 'done'}
      </div>
    {/if}

    {#if data.agents.length}
      <div class="pt-1">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent addresses</span>
        <div class="mt-1 text-xs text-muted">
          Org agents send from their own address{data.orgEmail ? ` (a plus-address of ${data.orgEmail})` : ''}; override per agent in its summary tab.
        </div>
        <div class="mt-1.5 space-y-0.5">
          {#each data.agents.slice(0, 6) as a (a.model)}
            <div class="flex items-baseline gap-2 text-xs">
              <span class="shrink-0 text-fg">{a.displayName}</span>
              <span class="truncate font-mono text-muted">{a.effective ?? '—'}</span>
              {#if a.alias}<span class="shrink-0 font-sans text-[10px] text-ink-dim">override</span>{/if}
            </div>
          {/each}
          {#if data.agents.length > 6}
            <div class="text-xs text-muted">+ {data.agents.length - 6} more</div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/if}
