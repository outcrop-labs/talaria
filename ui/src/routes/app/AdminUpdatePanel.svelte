<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'

  // In-app updates. Manual by default: an admin presses the button, Talaria
  // pulls, builds, and restarts itself. The auto toggle opts into the
  // scheduled check, which is the only path that updates without a person.
  interface RevInfo {
    rev: string
    short: string
    subject: string
    at: string | null
  }
  interface UpdateStatus {
    mode: 'server' | 'dev' | 'off'
    current: RevInfo | null
    autoUpdate: boolean
    lastCheck: { at: string; behind: number; current: RevInfo; latest: RevInfo } | null
    lastRun: { at: string; from: string; to: string; by: 'manual' | 'auto'; state: 'running' | 'done' | 'failed'; error?: string | null } | null
  }

  const qc = useQueryClient()
  // `kicked` is local: it remembers that THIS browser pressed Update, so the
  // polling keeps going through the restart itself, when every request fails
  // and the query has nothing fresh to say.
  let kicked = $state(false)
  const query = createQuery(() => ({
    queryKey: ['admin-update'],
    queryFn: (): Promise<UpdateStatus> => getJson('/api/admin/update'),
    retry: false,
  }))
  const status = $derived(query.data)
  const running = $derived(status?.lastRun?.state === 'running')

  // Poll on an effect rather than refetchInterval: the interval has to ride
  // through FAILED fetches (the server is restarting; every request errors)
  // and read reactive state the options object cannot reach without
  // self-referencing the query it is creating.
  $effect(() => {
    if (!(kicked || running)) return
    const timer = setInterval(() => void query.refetch(), 3_000)
    return () => clearInterval(timer)
  })

  let busy = $state<'check' | 'apply' | null>(null)
  let error = $state<string | null>(null)

  const check = async () => {
    busy = 'check'
    error = null
    try {
      await postJson<{ ok: true }>('/api/admin/update', { action: 'check' })
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-update'] })
  }

  const apply = async () => {
    const ok = await confirm({
      title: 'Update now?',
      message: 'Talaria downloads the latest release, builds it, and restarts. Everyone picks right back up in a minute or two.',
      confirmLabel: 'Update now',
    })
    if (!ok) return
    busy = 'apply'
    error = null
    kicked = true
    try {
      await postJson<{ ok: true }>('/api/admin/update', { action: 'apply' })
    } catch (e) {
      kicked = false
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-update'] })
  }

  const setAuto = async (on: boolean) => {
    error = null
    try {
      await putJson<{ ok: true }>('/api/admin/update', { autoUpdate: on })
    } catch (e) {
      error = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['admin-update'] })
  }

  // The update finished once the server is back and its own bookkeeping says
  // so; only then may the restart polling stop.
  $effect(() => {
    if (kicked && status && !running) kicked = false
  })

  const behind = $derived(status?.lastCheck?.behind ?? 0)
</script>

<Panel>
  <SectionHeader
    title="Updates"
    info="Keep this Talaria current from right here. No server shell needed: it downloads the latest release, builds it, and restarts on its own."
  />

  {#if query.isPending}
    <Skeleton class="h-4 w-64" />
  {:else if status === undefined}
    <QueryError variant="inline" error={query.error} title="Could not load update status" onRetry={() => void query.refetch()} />
  {:else}
    <div class="space-y-3">
      {#if status.mode === 'dev'}
        <!-- Dev reloads on file change, so an update button would be a no-op
             with a restart attached. Say the truth instead of hiding it. -->
        <p class="text-sm text-muted">
          You're in dev, which picks up code changes on its own. Updates are for server installs.
        </p>
      {:else if status.mode === 'off'}
        <p class="text-sm text-muted">Updates are switched off on this install (TALARIA_UPDATER=off).</p>
      {:else if running}
        <div class="flex items-center gap-2 text-sm text-fg">
          <span class="h-2 w-2 shrink-0 animate-pulse rounded-full bg-[var(--theme-accent)]"></span>
          Updating. Talaria restarts when it's ready, and this page comes back on its own.
        </div>
      {:else if kicked}
        <!-- Requests are failing against a server that is restarting: the
             honest line is the restart, not an error card. -->
        <div class="flex items-center gap-2 text-sm text-fg">
          <span class="h-2 w-2 shrink-0 animate-pulse rounded-full bg-[var(--theme-accent)]"></span>
          Restarting with the new build. Hang tight.
        </div>
      {:else}
        {#if status.current}
          <p class="text-sm text-muted">
            You're on
            <span class="font-mono text-[13px] text-fg">{status.current.short}</span>
            {status.current.subject}
            <span class="whitespace-nowrap">({relativeTime(status.current.at)})</span>
          </p>
        {/if}
        {#if behind > 0 && status.lastCheck}
          <p class="text-sm text-fg">
            {behind} update{behind === 1 ? '' : 's'} available. Latest: {status.lastCheck.latest.subject}
          </p>
          <div class="flex items-center gap-2">
            <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void apply()}>
              {busy === 'apply' ? 'Starting...' : 'Update now'}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void check()}>
              {busy === 'check' ? 'Checking...' : 'Check again'}
            </Button>
          </div>
        {:else if status.lastCheck}
          <div class="flex items-center gap-2">
            <p class="text-sm text-muted">You're up to date.</p>
            <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void check()}>
              {busy === 'check' ? 'Checking...' : 'Check for updates'}
            </Button>
          </div>
        {:else}
          <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void check()}>
            {busy === 'check' ? 'Checking...' : 'Check for updates'}
          </Button>
        {/if}
      {/if}

      {#if status.mode === 'server' && !running}
        <div class="border-t border-line pt-3">
          <Checkbox checked={status.autoUpdate} onChange={(v) => void setAuto(v)} label="Update automatically" />
          <p class="mt-1 text-xs text-muted">
            Checks every few hours and installs updates on its own. Off by default.
          </p>
        </div>
      {/if}

      {#if status.lastRun?.state === 'failed' && status.lastRun.error}
        <p class="text-sm text-danger">{status.lastRun.error}</p>
      {:else if status.lastRun?.state === 'done'}
        <p class="text-xs text-muted">Last updated {relativeTime(status.lastRun.at)}.</p>
      {/if}

      {#if error}
        <p class="text-sm text-danger">{error}</p>
      {/if}
    </div>
  {/if}
</Panel>
