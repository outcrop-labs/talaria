<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { ArrowUpRight, RefreshCw } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { alert, confirm } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { fetchAdminApps, installApp, shortSha, type KeptDataConflict } from './apps'
  import AppsCatalogs from './AppsCatalogs.svelte'
  import AppsMark from './AppsMark.svelte'

  let { isAdmin }: { isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['admin-apps-catalog'],
    queryFn: () => fetchAdminApps(true),
    staleTime: 5 * 60_000,
  }))
  let gitUrl = $state('')
  let gitRef = $state('')
  let gitSecret = $state('')
  let busy = $state<string | null>(null)
  let notice = $state<string | null>(null)

  const installed = $derived(new Set((query.data?.apps ?? []).map((a) => a.slug)))

  type InstallArgs = { url: string; slug?: string; ref?: string; secret?: string }
  /** An install refused over data another repository's install kept: the
   *  sentence, both repositories, and the request to repeat with a choice. */
  let keptConflict = $state<{ error: string; conflict: KeptDataConflict; args: InstallArgs } | null>(null)

  const install = async (args: InstallArgs, keptData?: 'adopt' | 'wipe') => {
    busy = args.slug ?? args.url
    notice = null
    keptConflict = null
    try {
      const r = await installApp({
        installUrl: args.url,
        ...(args.slug ? { slug: args.slug } : {}),
        ...(args.ref ? { ref: args.ref } : {}),
        ...(args.secret ? { secret: args.secret } : {}),
        ...(keptData ? { keptData } : {}),
      })
      if (r.conflict) {
        keptConflict = { error: r.error ?? 'the slug holds data another install kept', conflict: r.conflict, args }
      } else if (r.error) {
        void alert({ title: 'Install failed', message: r.error })
      } else {
        const built = r.build?.ok
          ? 'It built and loaded. Enable it under Installed.'
          : `It did not build: ${r.build?.error ?? r.buildError ?? 'unknown error'}. Fix the app and rebuild it, or uninstall it.`
        const kept = r.keptData
          ? ' The data a previous install kept is attached again.'
          : r.wipedData
            ? ' The data a previous install kept was deleted first.'
            : ''
        // Grants on a slug follow the repository: those the slug held for
        // another repository, or for none on record, are gone.
        const n = r.grantsRemoved ?? 0
        const grants = n
          ? ` ${n === 1 ? 'One person or team' : `${n} people and teams`} lost the access they held under this name, which belonged to another repository. Grant it again in Admin or Teams.`
          : ''
        notice = `Installed ${r.slug} at ${shortSha(r.sha)}. ${built}${kept}${grants}`
        gitUrl = ''
        gitRef = ''
        gitSecret = ''
        await qc.invalidateQueries({ queryKey: ['admin-apps'] })
        await qc.invalidateQueries({ queryKey: ['admin-apps-catalog'] })
        await qc.invalidateQueries({ queryKey: ['admin-apps-updates'] })
      }
    } finally {
      busy = null
    }
  }

  /** Repeat the refused install, giving it the kept data or deleting that
   *  data first. Deleting is for good, so it asks once more. */
  const settleKept = async (choice: 'adopt' | 'wipe') => {
    const k = keptConflict
    if (!k) return
    if (choice === 'wipe') {
      const ok = await confirm({
        title: `Delete the data kept under "${k.conflict.slug}"?`,
        message: `Every record ${k.conflict.keptFrom ?? 'the earlier install'} stored is deleted for good, and ${k.conflict.installing} installs with an empty store.`,
        confirmLabel: 'Delete data and install',
        danger: true,
      })
      if (!ok) return
    }
    await install(k.args, choice)
  }

  const catalog = $derived(query.data?.catalog)
  const catalogs = $derived(catalog?.catalogs ?? [])
  /** The enabled catalogs that could not be read, by name and reason. */
  const unreadable = $derived(catalogs.filter((c) => c.enabled && c.error).map((c) => `${c.name}: ${c.error}`))
  /** Why Discover lists nothing, in the operator's terms: no catalog at
   *  all, every catalog off, the catalogs could not be read, or they are
   *  simply empty. */
  const emptyWhy = $derived.by(() => {
    if (catalogs.length === 0) return { title: 'No catalog source yet', hint: 'Add a source below, or install an app from its Git repository.' }
    if (!catalogs.some((c) => c.enabled)) return { title: 'Every catalog source is off', hint: 'Turn one on below, or install an app from its Git repository.' }
    if (unreadable.length > 0) return { title: 'Catalog sources unreachable', hint: `${unreadable.join('; ')}. You can still install an app from its Git repository.` }
    return { title: 'No apps listed yet', hint: 'Install an app from its Git repository below.' }
  })

  const gitUrlValid = $derived(/^(https|http|file):\/\/.+/.test(gitUrl.trim()))
</script>

{#if notice}
  <!-- A line, not a card of one row. -->
  <p role="status" class="flex items-baseline gap-2 font-sans text-[13px] leading-5 text-muted">
    <StatusDot status="ok" class="shrink-0 translate-y-[-1px]" />
    <span><span class="font-medium text-fg">App installed.</span> {notice}</span>
  </p>
{/if}
{#if keptConflict}
  <!-- Kept data from another repository: never adopted silently. -->
  <SettingsGroup data-kept-data-conflict>
    <SettingsRow label="Not installed yet" stacked>
      {#snippet hint()}
        The name <code class="font-mono text-xs">{keptConflict?.conflict.slug}</code> still holds data from
        {#if keptConflict?.conflict.keptFrom}<code class="break-all font-mono text-xs">{keptConflict.conflict.keptFrom}</code>{:else}an earlier install that did not record its repository{/if}.
        This install is <code class="break-all font-mono text-xs">{keptConflict?.conflict.installing}</code>.
      {/snippet}
      <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void settleKept('adopt')}>Install and keep that data</Button>
      <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void settleKept('wipe')}>Delete that data, then install</Button>
      <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => (keptConflict = null)}>Cancel</Button>
    </SettingsRow>
  </SettingsGroup>
{/if}

<SettingsSection
  title="Available apps"
  info="Installing an app adds its code to this instance, where it runs fully trusted, like the platform itself. Install only apps you trust. Official apps are maintained by Outcrop Labs."
>
  {#snippet action()}
    <Button size="sm" variant="ghost" onclick={() => void query.refetch()}>
      <RefreshCw size={14} class={query.isFetching ? 'gd-breathe' : ''} /> Refresh
    </Button>
  {/snippet}
  {#if query.isLoading}
    <SettingsGroup>
      <div class="px-5 py-4"><SkeletonRows rows={4} /></div>
    </SettingsGroup>
  {:else if !query.data}
    <!-- Distinct from "Catalog sources unreachable" below: THAT is the server
         telling us it could not read a catalog (a real, reported answer,
         with each catalog's `error` to quote). This is OUR read of the
         server failing, which says nothing about the catalogs at all. -->
    <QueryError error={query.error} title="Could not load apps" onRetry={() => void query.refetch()} />
  {:else if (catalog?.apps ?? []).length === 0}
    <EmptyState icon="◎" title={emptyWhy.title} hint={emptyWhy.hint} />
  {:else}
    <!-- data-app-cards: the hook Apps.svelte's data-stagger-items selector
         targets, so these cards cascade on the page entrance. -->
    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3" data-app-cards>
      {#each catalog!.apps as c (c.slug)}
        <Panel class="flex flex-col p-5">
          <div class="flex items-center gap-3">
            <AppsMark icon={c.icon} tile />
            <div class="min-w-0 flex-1">
              <div class="flex min-h-[22px] items-center gap-2">
                <span class="min-w-0 truncate font-sans text-sm font-medium leading-5 text-fg">{c.name}</span>
                {#if c.official}<Chip tone="accent">Official</Chip>{/if}
              </div>
              <div class="truncate font-sans text-xs leading-4 text-muted" title={`Listed by the ${c.catalog.name} catalog`}>by {c.author}</div>
            </div>
          </div>
          <p class="mt-3 min-h-10 flex-1 font-sans text-[13px] leading-5 text-muted">{c.description}</p>
          <div class="mt-4 flex h-8 items-center justify-between gap-3">
            {#if installed.has(c.slug)}
              <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><StatusDot status="ok" />Installed</span>
            {:else if isAdmin}
              <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void install({ url: c.repo, slug: c.slug })}>
                {busy === c.slug ? 'Installing' : 'Install'}
              </Button>
            {:else}
              <span></span>
            {/if}
            <a
              href={c.repo}
              target="_blank"
              rel="noreferrer"
              class="flex items-center gap-1 font-sans text-[13px] leading-5 text-muted transition-colors hover:text-fg"
            >
              Source <ArrowUpRight size={14} />
            </a>
          </div>
        </Panel>
      {/each}
    </div>
  {/if}
</SettingsSection>

{#if isAdmin}
  <SettingsSection
    title="Install from Git"
    description="Any repository with a talaria.json file at its root. Its code runs fully trusted."
    info="A private repository on one of this instance's Git hosts (Admin, Git hosts) is read with that host's login, read-only where the host can scope one. A private repository anywhere else needs a workspace secret that holds a token and lists that host."
  >
    <SettingsGroup>
      <SettingsRow label="Repository" hint="The https address of the repository." as="label">
        <Input size="sm" class="w-[360px] font-mono" bind:value={gitUrl} placeholder="https://github.com/you/your-app" />
      </SettingsRow>
      <SettingsRow label="Branch or tag" hint="Leave empty to use the default branch." as="label">
        <Input size="sm" class="w-[360px] font-mono" bind:value={gitRef} placeholder="main" />
      </SettingsRow>
      <SettingsRow label="Access secret" hint="Only for a private repository this instance has no login for." as="label">
        <Input size="sm" class="w-[360px]" bind:value={gitSecret} placeholder="Name of a workspace secret" />
      </SettingsRow>
      <div class="flex justify-end px-5 py-3">
        <Button
          size="sm"
          variant="outline"
          disabled={!gitUrlValid || busy !== null}
          onclick={() => void install({ url: gitUrl.trim(), ref: gitRef.trim(), secret: gitSecret.trim() })}
        >
          {busy !== null && busy === gitUrl.trim() ? 'Installing' : 'Install'}
        </Button>
      </div>
    </SettingsGroup>
  </SettingsSection>
{/if}

{#if query.data && catalog}
  <AppsCatalogs {catalogs} {isAdmin} />
{/if}
