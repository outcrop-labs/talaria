<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { getJson, getList } from '@/lib/fetch-json'
  import AdminGithubGuideModal from './AdminGithubGuideModal.svelte'
  import AdminRepoCreationSection from './AdminRepoCreationSection.svelte'
  import AdminRepoFlowSection from './AdminRepoFlowSection.svelte'

  /** The Workbench's GitHub connection — one calm panel: status, the minimal
   *  connect controls, repo flow. The full field-by-field walkthrough lives in
   *  a setup-guide modal so the panel itself stays readable. */
  interface GhStatus {
    mode: 'app' | 'pat' | null
    configured: boolean
    account: string | null
    error: string | null
    app: { appId: string; installationIds: string[]; keySet: boolean }
    patSet: boolean
    repoCreationOrgs?: string[]
  }

  const qc = useQueryClient()
  const statusQuery = createQuery(() => ({
    queryKey: ['github-status'],
    queryFn: async (): Promise<GhStatus> => (await getJson<{ status: GhStatus }>('/api/workbench/github')).status,
  }))
  const status = $derived(statusQuery.data)
  let mode = $state<'app' | 'pat' | ''>('')
  let pat = $state('')
  let appId = $state('')
  let privateKey = $state('')
  let error = $state<string | null>(null)
  let busy = $state(false)
  let guideOpen = $state(false)
  const effMode = $derived(mode || status?.mode || '')

  const save = async (body: unknown) => {
    busy = true
    error = null
    const r = await fetch('/api/workbench/github', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    busy = false
    if (!r.ok) error = ((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'failed'
    await qc.invalidateQueries({ queryKey: ['github-status'] })
    await qc.invalidateQueries({ queryKey: ['github-installations'] })
  }
  const disconnect = async () => {
    if (!(await confirm({ title: 'Disconnect GitHub', message: 'Agents lose workbench repo access until reconnected.', confirmLabel: 'Disconnect', danger: true }))) return
    await fetch('/api/workbench/github', { method: 'DELETE', credentials: 'same-origin' })
    await qc.invalidateQueries({ queryKey: ['github-status'] })
  }

  // An empty install list makes the panel say "install the app on GitHub first"
  // — advice that is wrong, and acted on, if the list simply failed to load.
  const installsQuery = createQuery(() => ({
    queryKey: ['github-installations'],
    enabled: status?.mode === 'app' && !!status.app.keySet,
    queryFn: (): Promise<Array<{ id: number; account: string }>> =>
      getList<{ id: number; account: string }>('/api/workbench/github?installations=1', 'installations'),
  }))
  const installations = $derived(installsQuery.data ?? [])
  // A failed BACKGROUND refetch keeps the chips it already has — only a failure
  // with nothing behind it turns the row into an error.
  const installsFailed = $derived(installsQuery.isError && installsQuery.data === undefined)
</script>

{#if statusQuery.isPending}
  <Panel class="mt-4">
    <Skeleton class="mb-3 h-4 w-24 rounded-full" />
    <SkeletonRows rows={2} />
  </Panel>
{:else if !status}
  <!-- "Not connected — pick a method" below is a verdict on the stored GitHub
       credentials. A failed read never delivered one, and following that advice
       means re-pasting an App key that was already there. -->
  <Panel class="mt-4">
    <QueryError
      variant="compact"
      error={statusQuery.error}
      title="Could not load the GitHub connection"
      onRetry={() => void statusQuery.refetch()}
    />
  </Panel>
{:else}
  <Panel class="mt-4">
    <SectionHeader
      title="GitHub · Workbench"
      info="Lets granted agents work real repositories through their sandboxed workbench. Connect once; which agent may touch which repo stays an explicit per-agent grant."
    >
      {#snippet action()}
        <Button size="sm" variant="outline" onclick={() => (guideOpen = true)}>
          Setup guide
        </Button>
      {/snippet}
    </SectionHeader>
    <div class="space-y-4">
      <!-- Status -->
      {#if status?.configured}
        <div class="flex items-center gap-2 text-sm">
          <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-success"></span>
          <span class="text-fg">Connected{status.account ? ` as ${status.account}` : ''}</span>
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">via {status.mode === 'app' ? 'GitHub App' : 'access token'}</span>
          <button type="button" onclick={() => void disconnect()} class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger">
            Disconnect
          </button>
        </div>
      {:else}
        <div class="flex items-center gap-2 text-sm">
          <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-line"></span>
          <span class="text-muted">Not connected — pick a method and follow the setup guide.</span>
        </div>
      {/if}
      {#if status?.error}<div class="text-xs text-warning">{status.error}</div>{/if}

      <!-- Connect controls — minimal; the guide holds the walkthrough -->
      <div class="space-y-2 rounded-md border border-line bg-raised/40 px-4 py-3">
        <div class="flex items-center gap-3">
          <label class="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Method</label>
          <Segmented
            options={[
              { id: 'app', label: 'GitHub App' },
              { id: 'pat', label: 'Access token' },
            ] as const}
            value={effMode === 'pat' ? 'pat' : 'app'}
            onChange={(m) => (mode = m)}
          />
          {#if effMode === 'app'}<span class="text-xs text-muted">recommended — short-lived tokens, per-repo installs</span>{/if}
        </div>

        {#if effMode !== 'pat'}
          <div class="flex items-center gap-3">
            <label class="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">App ID</label>
            <Input size="sm" bind:value={appId} placeholder={status?.app.appId || 'e.g. 1234567'} class="w-40" />
            <label class="ml-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Key</label>
            <Input size="sm" type="password" bind:value={privateKey} placeholder={status?.app.keySet ? 'set — paste .pem to replace' : 'paste the whole .pem'} class="min-w-0 flex-1" />
            <Button
              size="sm"
              disabled={busy || (!appId.trim() && !privateKey.trim())}
              onclick={() =>
                void save({ mode: 'app', app: { ...(appId.trim() ? { appId: appId.trim() } : {}), ...(privateKey.trim() ? { privateKey: privateKey.trim() } : {}) } }).then(() => {
                  appId = ''
                  privateKey = ''
                })}
            >
              Save
            </Button>
          </div>
          <div class="flex items-start gap-3">
            <label class="w-20 shrink-0 pt-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Installed on</label>
            <div class="flex flex-wrap items-center gap-1.5">
              {#each installations as i (i.id)}
                {@const selected = (status?.app.installationIds ?? []).includes(String(i.id))}
                <button
                  type="button"
                  disabled={!status?.app.keySet}
                  onclick={() =>
                    void save({
                      mode: 'app',
                      app: {
                        installationIds: selected
                          ? (status?.app.installationIds ?? []).filter((x) => x !== String(i.id))
                          : [...(status?.app.installationIds ?? []), String(i.id)],
                      },
                    })}
                  class={cn(
                    'rounded-md border px-2.5 py-0.5 font-mono text-[11px] transition-colors',
                    focusGold,
                    selected ? 'border-[var(--theme-accent-border)] bg-accent/10 text-fg' : 'border-line-subtle text-muted hover:border-line hover:text-fg',
                  )}
                >
                  {i.account} (#{i.id})
                </button>
              {/each}
              {#if status?.app.keySet && installsFailed}
                <QueryError
                  variant="inline"
                  error={installsQuery.error}
                  title="Could not list installations"
                  onRetry={() => void installsQuery.refetch()}
                />
              {/if}
              {#if status?.app.keySet && !installsFailed && installations.length === 0}
                <span class="text-xs text-muted">install the app on GitHub first — the guide's step 3</span>
              {/if}
              {#if !status?.app.keySet}<span class="text-xs text-muted">save the App ID + key first</span>{/if}
              {#if (status?.app.installationIds?.length ?? 0) > 1}<span class="text-xs text-muted">repos pool across all selected orgs</span>{/if}
            </div>
          </div>
        {:else}
          <div class="flex items-center gap-3">
            <label class="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Token</label>
            <Input size="sm" type="password" bind:value={pat} placeholder={status?.patSet ? '••••••••  set — paste to replace' : 'github_pat_…'} class="min-w-0 flex-1" />
            <Button size="sm" disabled={!pat.trim() || busy} onclick={() => void save({ mode: 'pat', pat: { token: pat.trim() } }).then(() => (pat = ''))}>
              Connect
            </Button>
          </div>
        {/if}
        {#if error}<div class="text-xs text-danger">{error}</div>{/if}
      </div>

      {#if status?.configured}
        <AdminRepoCreationSection {status} {save} />
        <AdminRepoFlowSection />
      {/if}
    </div>

    <AdminGithubGuideModal open={guideOpen} onClose={() => (guideOpen = false)} mode={effMode === 'pat' ? 'pat' : 'app'} />
  </Panel>
{/if}
