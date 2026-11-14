<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm, alert } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { navigate, p } from '@/router'
  import { delJson, errorMessage, postJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { fetchAdminApps, fetchAppUpdates, post, requestChange, shortSha, type AppPlugin, type AppTriggerInfo, type AppUpdateResult, type InstalledApp } from './apps'
  import AppsApprovals from './AppsApprovals.svelte'
  import AppsMark from './AppsMark.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'

  let { isAdmin }: { isAdmin: boolean } = $props()

  const qc = useQueryClient()
  // Polled so the trigger lines follow their schedules while the tab is open.
  const query = createQuery(() => ({ queryKey: ['admin-apps'], queryFn: () => fetchAdminApps(false), refetchInterval: 30_000 }))
  // Whether each app's source has a newer commit: `git ls-remote` against its
  // recorded ref, admin only (a check mints the app's GitHub read token; a
  // source read with a workspace secret is not checked, `notChecked`).
  const updates = createQuery(() => ({
    queryKey: ['admin-apps-updates'],
    queryFn: fetchAppUpdates,
    enabled: isAdmin,
    staleTime: 60_000,
  }))
  /** A trigger's hover: when it fires next, and its newest runs. */
  function triggerTitle(t: AppTriggerInfo): string {
    const head = t.cron ? `Runs on the schedule ${t.cron} (UTC)${t.nextAt ? `; next ${new Date(t.nextAt).toLocaleString()}` : ''}` : `Runs on every ${t.event}`
    const runs = t.runs.map((r) => `${new Date(r.queuedAt).toLocaleString()}: ${r.state}${r.error ? `: ${r.error}` : r.detail ? `: ${r.detail}` : ''}`)
    return [head, ...runs].join('\n')
  }
  let busy = $state<string | null>(null)
  /** The last update's outcome, per app, shown on its row. */
  let results = $state<Record<string, { ok: boolean; text: string }>>({})

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ['admin-apps'] })
    await qc.invalidateQueries({ queryKey: ['admin-apps-updates'] })
    await qc.invalidateQueries({ queryKey: ['apps'] })
    await qc.invalidateQueries({ queryKey: ['session'] })
  }

  const update = async (a: InstalledApp) => {
    busy = a.slug
    delete results[a.slug]
    try {
      const r = await putJson<AppUpdateResult>('/api/admin/apps', { app: a.slug, update: true })
      results[a.slug] = r.upToDate
        ? { ok: true, text: `Already up to date (${shortSha(r.to)})` }
        : r.ok
          ? { ok: true, text: `Updated ${shortSha(r.from)} → ${shortSha(r.to)}` }
          : r.rolledBack
            ? { ok: false, text: `Update to ${shortSha(r.to)} failed and was rolled back; still running ${shortSha(r.from)}: ${r.error ?? 'unknown error'}` }
            : { ok: false, text: `Update to ${shortSha(r.to)} failed and could not be rolled back: apps/${a.slug} now holds ${shortSha(r.to)}, which did not load: ${r.error ?? 'unknown error'}` }
      await refresh()
    } catch (e) {
      results[a.slug] = { ok: false, text: `Could not update: ${errorMessage(e)}` }
    } finally {
      busy = null
    }
  }

  const toggle = async (a: InstalledApp) => {
    busy = a.slug
    try {
      if (!a.enabled) {
        const health = await postJson<{ ok: boolean; error?: string }>(`/api/app-host/${a.slug}/rebuild`, {})
        if (!health.ok) {
          void alert({ title: `${a.name} cannot be enabled`, message: health.error ?? 'app failed to compile or load' })
          await refresh()
          return
        }
      }
      const r = await post({ app: a.slug, enabled: !a.enabled })
      if (r.error) void alert({ title: 'Could not update app', message: r.error })
      await refresh()
    } catch (e) {
      void alert({ title: 'Could not update app', message: errorMessage(e) })
    } finally {
      busy = null
    }
  }

  const rebuild = async (a: InstalledApp) => {
    busy = a.slug
    try {
      const health = await postJson<{ ok: boolean; error?: string }>(`/api/app-host/${a.slug}/rebuild`, {})
      if (!health.ok) void alert({ title: `${a.name} is still broken`, message: health.error ?? 'app failed to compile or load' })
      await refresh()
    } catch (e) {
      void alert({ title: 'Could not rebuild', message: errorMessage(e) })
    } finally {
      busy = null
    }
  }

  const uninstall = async (a: InstalledApp) => {
    const ok = await confirm({
      title: `Uninstall ${a.name}?`,
      message: 'Removes the app codebase and its build from this deployment and stops its database. Agents lose its tools and every grant on it.',
      confirmLabel: 'Uninstall',
      danger: true,
    })
    if (!ok) return
    // Data is kept unless the admin says otherwise: a reinstall of the same
    // repository reattaches it, and the grants on the app stay with it.
    const wipeData = await confirm({
      title: `Delete the data ${a.name} stored?`,
      message: 'Keep it, and reinstalling the same repository picks it up again, with everyone it was granted to. Delete it, and every record the app stored, and every grant on it, is gone for good.',
      confirmLabel: 'Delete data',
      cancelLabel: 'Keep data',
      danger: true,
    })
    busy = a.slug
    try {
      await delJson<{ ok: true; data: 'wiped' | 'kept' | 'none'; grantsRemoved: number }>('/api/admin/apps', { app: a.slug, wipeData }).catch((e) => {
        void alert({ title: 'Could not uninstall', message: errorMessage(e) })
      })
      await refresh()
    } finally {
      busy = null
    }
  }

  // A plugin's update policy: approve (every update waits for an admin) or
  // auto (the maintenance sweep applies it when it sees it). Admin only.
  const setPolicy = async (a: InstalledApp, updatePolicy: AppPlugin['updatePolicy']) => {
    if (a.plugin?.updatePolicy === updatePolicy) return
    busy = a.slug
    try {
      await putJson<{ ok: true }>('/api/admin/apps', { app: a.slug, updatePolicy })
      await refresh()
    } catch (e) {
      void alert({ title: 'Could not change the update policy', message: errorMessage(e) })
    } finally {
      busy = null
    }
  }

  // "Request a change": a ticket for the plugin's maintaining agent on its
  // board, filed by whoever asks (the api checks they could file one there).
  let changing = $state<string | null>(null)
  let changeText = $state('')
  let filed = $state<Record<string, { ok: boolean; text: string; href?: string }>>({})
  const sendChange = async (a: InstalledApp) => {
    const text = changeText.trim()
    if (!text) return
    busy = a.slug
    try {
      const r = await requestChange(a.slug, text)
      filed[a.slug] = {
        ok: true,
        text: `Filed ${r.ticketRef ?? 'a ticket'} for ${a.plugin?.maintainer?.name ?? a.plugin?.maintainer?.model ?? 'its maintainer'}`,
        href: `/boards/${r.boardId}/${r.taskId}`,
      }
      changing = null
      changeText = ''
    } catch (e) {
      filed[a.slug] = { ok: false, text: `Could not file it: ${errorMessage(e)}` }
    } finally {
      busy = null
    }
  }

  const apps = $derived(query.data?.apps ?? [])

  /** The rows whose details (source, maintainer, triggers) are open. */
  let detailsOpen = $state<Record<string, boolean>>({})

  const POLICY_LABEL: Record<AppPlugin['updatePolicy'], string> = { approve: 'Need approval', auto: 'Automatic' }

  const triggersOf = (a: InstalledApp): AppTriggerInfo[] => (a.broken ? [] : (a.triggers ?? []))
  const failedRuns = (t: AppTriggerInfo) => t.runs.filter((r) => r.state === 'failed')
  const hasDetails = (a: InstalledApp) => !!a.git || !!a.plugin || triggersOf(a).length > 0
  const canOpen = (a: InstalledApp) => a.enabled && !!a.surfaces.work

  const statusOf = (a: InstalledApp): { dot: DotStatus; text: string; title?: string; pulse?: boolean } => {
    if (busy === a.slug) return { dot: 'warn', text: 'Working', pulse: true }
    if (a.broken) return { dot: 'danger', text: 'Broken manifest', title: a.broken }
    if (a.build?.status === 'building') return { dot: 'warn', text: 'Compiling', pulse: true }
    if (a.build?.status === 'failed') return { dot: 'danger', text: 'Build failed', title: a.build.error }
    return a.enabled ? { dot: 'ok', text: 'Enabled' } : { dot: 'idle', text: 'Disabled' }
  }

  const open = (a: InstalledApp) => void navigate('/x/:app', { params: { app: a.slug } })

  /** The one action a row shows. A failed build needs a person, so it reads
   *  Fix; an app that is off is a choice, so it offers Enable in the same
   *  quiet treatment. Open is the row's click, and both are in its menu too. */
  const fixOf = (a: InstalledApp): 'rebuild' | 'enable' | null => {
    if (isAdmin && !a.broken && a.build?.status === 'failed') return 'rebuild'
    if (isAdmin && !a.broken && !a.enabled) return 'enable'
    return null
  }

  const menuOf = (a: InstalledApp): ContextMenuEntry[] => {
    const fix = fixOf(a)
    const working = busy === a.slug
    const items: ContextMenuEntry[] = []
    if (canOpen(a)) items.push({ label: 'Open', onSelect: () => open(a) })
    if (hasDetails(a))
      items.push({
        label: detailsOpen[a.slug] ? 'Hide details' : 'Show details',
        onSelect: () => (detailsOpen[a.slug] = !detailsOpen[a.slug]),
      })
    if (a.plugin)
      items.push({
        label: 'Request a change',
        onSelect: () => {
          changing = changing === a.slug ? null : a.slug
          changeText = ''
        },
      })
    if (!isAdmin) return items
    if (a.git && !a.broken) items.push({ label: 'Update', disabled: working, onSelect: () => void update(a) })
    if (a.plugin)
      items.push({
        label: 'Updates',
        disabled: working,
        children: (['approve', 'auto'] as const).map((policy) => ({
          label: POLICY_LABEL[policy],
          checked: a.plugin?.updatePolicy === policy,
          onSelect: () => void setPolicy(a, policy),
        })),
      })
    if (fix === 'rebuild') items.push({ label: 'Rebuild', disabled: working, onSelect: () => void rebuild(a) })
    if (!a.broken) items.push({ label: a.enabled ? 'Disable' : 'Enable', disabled: working, onSelect: () => void toggle(a) })
    if (items.length) items.push('sep')
    items.push({ label: 'Uninstall', danger: true, disabled: working, onSelect: () => void uninstall(a) })
    return items
  }

  const addsOf = (a: InstalledApp): Array<{ text: string; title: string }> => {
    const parts: Array<{ text: string; title: string }> = []
    if (a.surfaces.work) parts.push({ text: 'Work view', title: `Work view: ${a.surfaces.work}` })
    if (a.surfaces.manage) parts.push({ text: 'Manage view', title: `Manage view: ${a.surfaces.manage}` })
    if (a.surfaces.settings) parts.push({ text: 'Settings panel', title: `Settings panel: ${a.surfaces.settings}` })
    if (a.mcp) parts.push({ text: 'Agent tools', title: 'Gives agents tools through its own tool server. Choose who may use them under Tools.' })
    const registered = a.build?.harnesses?.registered ?? []
    if (registered.length > 0) {
      const n = `${registered.length} harness${registered.length === 1 ? '' : 'es'}`
      parts.push(
        a.enabled
          ? { text: n, title: `Activity harnesses scored in Models, under Fitness:\n${registered.join('\n')}` }
          : {
              text: `${n}, not registered`,
              title: `Not registered while ${a.name} is disabled: enable it to score them in Models, under Fitness.\n${registered.join('\n')}`,
            },
      )
    }
    return parts
  }

  const note = 'font-sans text-[13px] leading-5'
  const identifier = 'font-mono text-xs text-muted'
</script>

{#if isAdmin}<AppsApprovals {apps} />{/if}

<!-- The app's version and what it adds, as one quiet line under its purpose:
     every app has these, so they are words and not chips, and the title line
     is the name alone. The activity harnesses it ships (harnesses/*.ts) end
     the line, as the host last checked them against the enabled apps. A
     disabled app registers none: its count is what it would register once
     enabled. -->
{#snippet adds(a: InstalledApp)}
  {@const parts = addsOf(a)}
  {#if a.version || parts.length > 0}
    <div class="font-sans text-xs leading-5 text-muted">
      {#if a.version}<span class="font-mono">v{a.version}</span>{parts.length > 0 ? ' · ' : ''}{/if}{#each parts as part, i (part.text)}{i > 0 ? ' · ' : ''}<span title={part.title}>{part.text}</span>{/each}
    </div>
  {/if}
{/snippet}

<!-- A chip only for what is unusual about this row. -->
{#snippet exceptions(a: InstalledApp)}
  {@const skipped = a.build?.harnesses?.problems ?? []}
  {@const check = a.git ? updates.data?.[a.slug] : undefined}
  {#if check?.available}
    <Chip tone="accent" title={`The source now points at ${shortSha(check.latest)}`}>Update available</Chip>
  {/if}
  {#if skipped.length > 0 && a.build?.status !== 'failed'}
    <Chip tone="warn" title={skipped.join('\n')}>{skipped.length} harness{skipped.length === 1 ? '' : 'es'} skipped</Chip>
  {/if}
{/snippet}

<!-- What an admin must see without opening anything: why the app is broken,
     a harness the platform skipped (it never disables the app, so this is the
     only place that says one is missing from the fitness matrix), a trigger
     that failed (it never turns the app off either), and the last outcome. -->
{#snippet attention(a: InstalledApp)}
  {#if a.broken}
    <div class={`${note} wrap-anywhere text-danger`}>{a.broken}</div>
  {:else if a.build?.status === 'failed' && a.build.error}
    <div class={`${note} wrap-anywhere text-danger`}>{a.build.error}</div>
  {/if}
  {#if a.build?.status !== 'failed'}
    {#each a.build?.harnesses?.problems ?? [] as problem (problem)}
      <div class={`${note} wrap-anywhere text-warning`} data-harness-problem>{problem}</div>
    {/each}
    <!-- Registered, and will not run as its author meant: a pin that names no
         platform agent resolves to nothing. -->
    {#each a.build?.harnesses?.warnings ?? [] as warning (warning)}
      <div class={`${note} wrap-anywhere text-warning`} data-harness-warning>{warning}</div>
    {/each}
  {/if}
  {#each triggersOf(a) as t (t.id)}
    {@const last = t.runs[0]}
    {@const failed = failedRuns(t)}
    {#if last?.state === 'failed'}
      <div class={`${note} wrap-anywhere text-danger`}>
        <span class="font-mono text-xs">{t.id}</span> failed {relativeTime(last.finishedAt ?? last.startedAt ?? last.queuedAt)}{#if last.error}: {last.error}{/if}
      </div>
    {:else if failed.length > 0}
      <div class={`${note} text-danger`} title={failed.map((r) => r.error ?? '').join('\n')}>
        <span class="font-mono text-xs">{t.id}</span> failed {failed.length} of its last {t.runs.length} runs
      </div>
    {/if}
  {/each}
  {#if a.git && updates.data?.[a.slug]?.error}
    <div class={`${note} text-danger`} title={updates.data[a.slug]?.error}>Update check failed</div>
  {/if}
  {#if results[a.slug]}
    <div class={`${note} wrap-anywhere ${results[a.slug]!.ok ? 'text-fg' : 'text-danger'}`} data-app-update-result>
      {results[a.slug]!.text}
    </div>
  {/if}
  {#if filed[a.slug]}
    {@const f = filed[a.slug]!}
    <div class={`${note} ${f.ok ? 'text-muted' : 'text-danger'}`} data-request-change-result>
      {#if f.href}<a href={f.href} class="text-fg underline-offset-2 hover:underline">{f.text}</a>{:else}{f.text}{/if}
    </div>
  {/if}
{/snippet}

<!-- One level down: where the code came from, who maintains a plugin and how
     its updates land, and each trigger with its last run. The lines WRAP
     rather than truncate: on a phone the run's sentence is most of what
     there is to read. -->
{#snippet details(a: InstalledApp)}
  <dl class={`grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 border-t border-line-subtle px-5 py-4 ${note}`}>
    {#if a.git}
      {@const check = updates.data?.[a.slug]}
      <dt class="text-muted">Source</dt>
      <dd class={`flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5 ${identifier}`} data-app-source>
        <span class="min-w-0 break-all" title={a.git.url}>{a.git.repo}</span>
        <span class="shrink-0 whitespace-nowrap">@ {a.git.ref ?? 'default branch'}</span>
        <span class="shrink-0" title={a.git.sha ?? 'installed before commits were recorded'}>{shortSha(a.git.sha)}</span>
        {#if check?.notChecked}<span class="shrink-0 font-sans text-[13px]" title={check.notChecked}>Update not checked</span>{/if}
      </dd>
    {/if}
    {#if a.plugin}
      {@const pl = a.plugin}
      <dt class="text-muted">Maintained by</dt>
      <dd class="text-fg" data-app-plugin>{pl.maintainer?.name ?? pl.maintainer?.model ?? 'No agent'}</dd>
      {#if pl.owner}
        <dt class="text-muted">Built for</dt>
        <dd class="text-fg">{pl.owner.name ?? 'A member'}</dd>
      {/if}
      {#if pl.board}
        <dt class="text-muted">Tickets go to</dt>
        <dd class="text-fg">
          {pl.board.name ?? 'Its board'}{#if pl.ticketRef}<span class={`ml-2 ${identifier}`}>{pl.ticketRef}</span>{/if}
        </dd>
      {/if}
      <dt class="text-muted">Updates</dt>
      <dd class="text-fg">{POLICY_LABEL[pl.updatePolicy]}</dd>
    {/if}
    {#each triggersOf(a) as t (t.id)}
      {@const last = t.runs[0]}
      <dt class="font-mono text-xs leading-5 text-muted">{t.id}</dt>
      <dd class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 text-muted" data-app-trigger={t.id}>
        <span class={`shrink-0 ${identifier}`} title={triggerTitle(t)}>{t.cron ? `${t.cron} UTC` : `on ${t.event}`}</span>
        {#if !last}
          <span class="min-w-0 wrap-anywhere">{a.enabled ? 'Not run yet' : 'Off while the app is disabled'}</span>
        {:else}
          <span class={`shrink-0 ${last.state === 'failed' ? 'text-danger' : last.state === 'ok' ? 'text-fg' : ''}`}>
            {last.state === 'ok' ? 'ran' : last.state} {relativeTime(last.finishedAt ?? last.startedAt ?? last.queuedAt)}
          </span>
          {#if last.state === 'failed' && last.error}
            <span class="min-w-0 wrap-anywhere text-danger" data-app-trigger-detail>{last.error}</span>
          {:else if last.detail}
            <span class="min-w-0 wrap-anywhere" data-app-trigger-detail>{last.detail}</span>
          {:else if last.state === 'skipped' && last.error}
            <span class="min-w-0 wrap-anywhere" data-app-trigger-detail>{last.error}</span>
          {/if}
        {/if}
      </dd>
    {/each}
  </dl>
{/snippet}

{#if query.isLoading}
  <SettingsGroup>
    <div class="px-5 py-4"><SkeletonRows rows={4} /></div>
  </SettingsGroup>
{:else if !query.data}
  <QueryError error={query.error} title="Could not load installed apps" onRetry={() => void query.refetch()} />
{:else if apps.length === 0}
  <EmptyState icon="⬡" title="No apps installed" hint="Find one under Discover, or install your own from a Git repository there." />
{:else}
  <SettingsSection title="Installed apps">
    <SettingsGroup data-app-cards>
      {#each apps as a (a.slug)}
        {@const standing = statusOf(a)}
        {@const fix = fixOf(a)}
        {@const menu = menuOf(a)}
        <div>
          <SettingsRow label={a.name} openHref={canOpen(a) ? p('/x/:app', { params: { app: a.slug } }) : undefined} openLabel={`Open ${a.name}`}>
            {#snippet icon()}<AppsMark icon={a.icon} />{/snippet}
            {#snippet meta()}{@render exceptions(a)}{/snippet}
            {#snippet hint()}
              <div>{a.description}</div>
              {@render adds(a)}
              {@render attention(a)}
            {/snippet}
            {#snippet status()}
              <span class="flex items-center gap-2" title={standing.title}>
                <StatusDot status={standing.dot} pulse={standing.pulse} />{standing.text}
              </span>
            {/snippet}
            {#if fix === 'rebuild'}
              <SettingsRowFix label={`Fix ${a.name}: rebuild`} disabled={busy === a.slug} onclick={() => void rebuild(a)} />
            {:else if fix === 'enable'}
              <SettingsRowFix text="Enable" label={`Enable ${a.name}`} disabled={busy === a.slug} onclick={() => void toggle(a)} />
            {/if}
            <SettingsRowMenu label={`Actions for ${a.name}`} items={menu} />
          </SettingsRow>
          {#if detailsOpen[a.slug] && hasDetails(a)}
            <div transition:slide={{ duration: 150 }}>{@render details(a)}</div>
          {/if}
          {#if changing === a.slug}
            <div transition:slide={{ duration: 150 }}>
              <div class="space-y-3 border-t border-line-subtle px-5 py-4" data-request-change-form>
                <Textarea
                  bind:value={changeText}
                  rows={3}
                  maxlength={4000}
                  placeholder={`What should change in ${a.name}? The first line becomes the ticket title.`}
                />
                <div class="flex justify-end gap-2">
                  <Button size="sm" variant="ghost" onclick={() => (changing = null)}>Cancel</Button>
                  <Button size="sm" variant="outline" disabled={busy === a.slug || !changeText.trim()} onclick={() => void sendChange(a)}>
                    {busy === a.slug ? 'Filing' : 'File ticket'}
                  </Button>
                </div>
              </div>
            </div>
          {/if}
        </div>
      {/each}
    </SettingsGroup>
  </SettingsSection>
{/if}
