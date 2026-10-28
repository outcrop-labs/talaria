<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { errorMessage, getList, putJson } from '@/lib/fetch-json'
  import { fly, slide } from '@/lib/motion'
  import AdminGithubGuideModal from './AdminGithubGuideModal.svelte'
  import { useGitHostsRefresh, type GitHost } from './git-hosts'

  /** A GitHub host's connection — App (recommended) or access token — inside
   *  its card in Admin → Git hosts. The full field-by-field walkthrough lives
   *  in the setup-guide modal so the card stays readable. Secrets go up once
   *  and never come back: the host only ever says whether each is set. */
  let { host }: { host: GitHost } = $props()

  const refresh = useGitHostsRefresh()
  const gh = $derived(host.github ?? { mode: null, app: { appId: '', installationIds: [], keySet: false }, patSet: false })
  let mode = $state<'app' | 'pat' | ''>('')
  let pat = $state('')
  let appId = $state('')
  let privateKey = $state('')
  let error = $state<string | null>(null)
  let busy = $state(false)
  let guideOpen = $state(false)
  const effMode = $derived(mode || gh.mode || '')
  const url = $derived(`/api/workbench/git-hosts/${encodeURIComponent(host.id)}`)

  const save = async (github: unknown) => {
    busy = true
    error = null
    try {
      await putJson(url, { github })
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh(host.id)
    await installsQuery.refetch()
  }
  const disconnect = async () => {
    if (!(await confirm({ title: `Disconnect ${host.name}`, message: 'Agents lose workbench access to its repos until it is reconnected.', confirmLabel: 'Disconnect', danger: true }))) return
    await save({ mode: null, pat: { token: null }, app: { appId: '', installationIds: [], privateKey: null } })
  }

  // An empty install list makes the card say "install the app on GitHub first"
  // — advice that is wrong, and acted on, if the list simply failed to load.
  const installsQuery = createQuery(() => ({
    queryKey: ['git-host-installations', host.id],
    enabled: gh.mode === 'app' && gh.app.keySet,
    queryFn: (): Promise<Array<{ id: number; account: string }>> =>
      getList<{ id: number; account: string }>(`${url}?installations=1`, 'installations'),
  }))
  const installations = $derived(installsQuery.data ?? [])
  const installsFailed = $derived(installsQuery.isError && installsQuery.data === undefined)
</script>

<div class="space-y-2 rounded-md border border-line bg-raised/40 px-3 py-3 sm:px-4">
  <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
    <span class="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Method</span>
    <Segmented
      options={[
        { id: 'app', label: 'GitHub App' },
        { id: 'pat', label: 'Access token' },
      ] as const}
      value={effMode === 'pat' ? 'pat' : 'app'}
      onChange={(m) => (mode = m)}
    />
    <Button size="xs" variant="ghost" onclick={() => (guideOpen = true)}>Setup guide</Button>
    {#if gh.mode}
      <Button size="xs" variant="ghost" class="ml-auto hover:text-danger" onclick={() => void disconnect()}>Disconnect</Button>
    {/if}
  </div>

  <!-- Method switch = a tab pane: fields rise in and the rows below glide as
       the region's height changes. Keying is safe — the drafts live above. -->
  <AutoHeight>
    {#key effMode === 'pat'}
      <div in:fly={{ y: 6, duration: 200 }} class="space-y-2">
        {#if effMode !== 'pat'}
          <div class="flex flex-wrap items-start gap-x-3 gap-y-2">
            <label for="gh-app-id-{host.id}" class="w-20 shrink-0 pt-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">App ID</label>
            <Input id="gh-app-id-{host.id}" size="sm" bind:value={appId} placeholder={gh.app.appId || 'e.g. 1234567'} class="w-40" />
            <!-- A TEXTAREA, not a password input: the HTML value sanitizer
                 strips line breaks from single-line inputs on paste, and a
                 .pem pasted as one line is a key nothing can parse. -->
            <Textarea
              aria-label="App private key"
              rows={2}
              bind:value={privateKey}
              placeholder={gh.app.keySet ? 'key set (paste .pem to replace)' : 'paste the whole .pem'}
              class="min-w-0 flex-1 basis-56 font-mono text-xs"
            />
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
          <div class="flex flex-wrap items-start gap-x-3 gap-y-1.5">
            <span class="w-20 shrink-0 pt-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Installed on</span>
            <div class="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
              {#each installations as i (i.id)}
                {@const selected = gh.app.installationIds.includes(String(i.id))}
                <button
                  type="button"
                  disabled={!gh.app.keySet}
                  onclick={() =>
                    void save({
                      mode: 'app',
                      app: {
                        installationIds: selected ? gh.app.installationIds.filter((x) => x !== String(i.id)) : [...gh.app.installationIds, String(i.id)],
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
              {#if gh.app.keySet && installsFailed}
                <QueryError variant="inline" error={installsQuery.error} title="Could not list installations" onRetry={() => void installsQuery.refetch()} />
              {/if}
              {#if gh.app.keySet && !installsFailed && installations.length === 0 && !installsQuery.isPending}
                <span class="text-xs text-muted">install the app on GitHub first (the guide's step 3)</span>
              {/if}
              {#if !gh.app.keySet}<span class="text-xs text-muted">save the App ID + key first</span>{/if}
              {#if gh.app.installationIds.length > 1}<span class="text-xs text-muted">repos pool across all selected orgs</span>{/if}
            </div>
          </div>
        {:else}
          <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
            <label for="gh-token-{host.id}" class="w-20 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Token</label>
            <Input id="gh-token-{host.id}" size="sm" type="password" bind:value={pat} placeholder={gh.patSet ? '••••••••  set (paste to replace)' : 'github_pat_…'} class="min-w-0 flex-1 basis-56" />
            <Button size="sm" disabled={!pat.trim() || busy} onclick={() => void save({ mode: 'pat', pat: { token: pat.trim() } }).then(() => (pat = ''))}>
              Connect
            </Button>
          </div>
        {/if}
      </div>
    {/key}
  </AutoHeight>
  {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
</div>

<AdminGithubGuideModal open={guideOpen} onClose={() => (guideOpen = false)} mode={effMode === 'pat' ? 'pat' : 'app'} />
