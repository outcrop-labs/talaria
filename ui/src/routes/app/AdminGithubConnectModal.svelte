<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { secretHint } from '@/components/admin/secret-hint'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage, getList, putJson } from '@/lib/fetch-json'
  import { fly, slide } from '@/lib/motion'
  import AdminGithubGuideModal from './AdminGithubGuideModal.svelte'
  import { useGitHostsRefresh, type GitHost } from './git-hosts'

  /** A GitHub host's connection — App (recommended) or access token — as its
   *  own dialog in Admin → Git hosts. The field-by-field walkthrough is the
   *  setup guide, which takes this dialog's place and hands it back, so what
   *  was typed here survives a look at the guide. Secrets go up once and never
   *  come back: the host only ever says whether each is set. */
  let { host, open, onClose }: { host: GitHost; open: boolean; onClose: () => void } = $props()

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
  const saveApp = async () => {
    await save({ mode: 'app', app: { ...(appId.trim() ? { appId: appId.trim() } : {}), ...(privateKey.trim() ? { privateKey: privateKey.trim() } : {}) } })
    appId = ''
    privateKey = ''
  }
  const savePat = async () => {
    await save({ mode: 'pat', pat: { token: pat.trim() } })
    pat = ''
    if (!error) onClose()
  }

  // An empty install list makes the dialog say "install the app on GitHub
  // first" — advice that is wrong, and acted on, if the list simply failed to
  // load.
  const installsQuery = createQuery(() => ({
    queryKey: ['git-host-installations', host.id],
    enabled: gh.mode === 'app' && gh.app.keySet,
    queryFn: (): Promise<Array<{ id: number; account: string }>> =>
      getList<{ id: number; account: string }>(`${url}?installations=1`, 'installations'),
  }))
  const installations = $derived(installsQuery.data ?? [])
  const installsFailed = $derived(installsQuery.isError && installsQuery.data === undefined)
  const installHint = $derived.by(() => {
    if (!gh.app.keySet) return 'Save the App ID and key first.'
    if (!installsFailed && installations.length === 0 && !installsQuery.isPending) return 'Install the app on GitHub first. It is step 3 of the setup guide.'
    if (gh.app.installationIds.length > 1) return 'Agents can reach the repositories of every installation you turn on.'
    return null
  })
</script>

<Modal open={open && !guideOpen} {onClose} title="Connect {host.name}" width="max-w-lg">
  <div class="space-y-5">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <Segmented
        options={[
          { id: 'app', label: 'GitHub App' },
          { id: 'pat', label: 'Access token' },
        ] as const}
        value={effMode === 'pat' ? 'pat' : 'app'}
        onChange={(m) => (mode = m)}
      />
      <Button size="sm" variant="link" class="text-[13px]" onclick={() => (guideOpen = true)}>Setup guide</Button>
    </div>

    <!-- Method switch = a tab pane: fields rise in and the rows below glide as
         the region's height changes. Keying is safe — the drafts live above. -->
    <AutoHeight>
      {#key effMode === 'pat'}
        <div in:fly={{ y: 6, duration: 200 }} class="space-y-5">
          {#if effMode !== 'pat'}
            <SettingsField label="App ID" for="gh-app-id-{host.id}">
              <Input id="gh-app-id-{host.id}" size="sm" bind:value={appId} placeholder={gh.app.appId || '1234567'} class="font-mono" />
            </SettingsField>
            <!-- A TEXTAREA, not a password input: the HTML value sanitizer
                 strips line breaks from single-line inputs on paste, and a
                 .pem pasted as one line is a key nothing can parse. -->
            <SettingsField
              label="Private key"
              for="gh-app-key-{host.id}"
              hint={gh.app.keySet ? 'A key is stored, and pasting a .pem file replaces it.' : 'Paste the whole .pem file, first and last lines included.'}
            >
              <Textarea id="gh-app-key-{host.id}" aria-label="App private key" rows={3} bind:value={privateKey} class="font-mono text-xs" />
            </SettingsField>
            <SettingsField label="Installed on">
              {#if installations.length > 0}
                <ul class="divide-y divide-line-subtle rounded-md border border-line">
                  {#each installations as i (i.id)}
                    {@const selected = gh.app.installationIds.includes(String(i.id))}
                    <li class="flex items-center gap-3 px-3 py-2">
                      <span class="min-w-0 flex-1 truncate font-sans text-[13px] leading-5 text-fg">{i.account}</span>
                      <span class="font-mono text-xs text-muted">{i.id}</span>
                      <Toggle
                        checked={selected}
                        disabled={!gh.app.keySet}
                        title="Use the installation on {i.account}"
                        onChange={() =>
                          void save({
                            mode: 'app',
                            app: {
                              installationIds: selected ? gh.app.installationIds.filter((x) => x !== String(i.id)) : [...gh.app.installationIds, String(i.id)],
                            },
                          })}
                      />
                    </li>
                  {/each}
                </ul>
              {/if}
              {#if gh.app.keySet && installsFailed}
                <QueryError variant="inline" error={installsQuery.error} title="Could not list installations" onRetry={() => void installsQuery.refetch()} />
              {/if}
              {#if installHint}<p class="font-sans text-[13px] leading-5 text-muted">{installHint}</p>{/if}
            </SettingsField>
          {:else}
            <SettingsField
              label="Access token"
              for="gh-token-{host.id}"
              hint={secretHint(gh.patSet, 'token')}
            >
              <Input id="gh-token-{host.id}" size="sm" type="password" autocomplete="off" bind:value={pat} placeholder="github_pat_" class="font-mono" />
            </SettingsField>
          {/if}
        </div>
      {/key}
    </AutoHeight>
    {#if error}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
  </div>
  {#snippet footer()}
    <div class="flex items-center gap-2">
      {#if gh.mode}<DangerLink onClick={() => void disconnect()}>Disconnect {host.name}</DangerLink>{/if}
      <span class="flex-1"></span>
      <Button size="sm" variant="ghost" onclick={onClose}>Cancel</Button>
      {#if effMode !== 'pat'}
        <Button size="sm" disabled={busy || (!appId.trim() && !privateKey.trim())} onclick={() => void saveApp()}>Save</Button>
      {:else}
        <Button size="sm" disabled={!pat.trim() || busy} onclick={() => void savePat()}>Connect</Button>
      {/if}
    </div>
  {/snippet}
</Modal>

<AdminGithubGuideModal open={open && guideOpen} onClose={() => (guideOpen = false)} mode={effMode === 'pat' ? 'pat' : 'app'} />
