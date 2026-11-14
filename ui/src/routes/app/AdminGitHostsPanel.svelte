<script lang="ts">
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import AdminGitHostRow from './AdminGitHostRow.svelte'
  import { useGitHosts, useGitHostsRefresh, type GitHostKind } from './git-hosts'

  /** Admin → Git hosts: the git providers this instance works repositories
   *  on. Operators choose their providers — GitHub (github.com or an
   *  Enterprise Server) or Forgejo / Gitea — and agents reach a repo only
   *  through the host it lives on and a per-agent grant. */
  /** The add dialog. The page header's "Add host" button opens it. */
  let { adding = $bindable(false) }: { adding?: boolean } = $props()

  const query = useGitHosts()
  const refresh = useGitHostsRefresh()

  let kind = $state<GitHostKind>('forgejo')
  let id = $state('')
  let name = $state('')
  let baseUrl = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)

  // Closing drops the draft, so the dialog always opens empty.
  const close = () => {
    adding = false
    kind = 'forgejo'
    id = ''
    name = ''
    baseUrl = ''
    idTouched = false
    error = null
  }
  // The slug follows the name until it is typed itself.
  let idTouched = $state(false)
  const slug = (s: string) =>
    s
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 40)
  $effect(() => {
    if (!idTouched) id = slug(name)
  })

  const create = async () => {
    busy = true
    error = null
    try {
      await postJson('/api/workbench/git-hosts', { id: id.trim(), kind, name: name.trim(), baseUrl: baseUrl.trim() })
      close()
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }
</script>

<SettingsSection
  title="Git hosts"
  description="Where your agents' repositories live."
  info="Add each git host your agents work on. Which agent may touch which repository stays a grant you give per agent. A GitHub host connects with a GitHub App or an access token. A Forgejo or Gitea host signs in with a workspace secret."
>
  <SettingsGroup>
    <QueryState {query} errorTitle="Could not load the git hosts" errorVariant="compact" isEmpty={(d) => d.hosts.length === 0}>
      {#snippet skeleton()}<div class="px-5 py-4"><SkeletonRows rows={2} avatar /></div>{/snippet}
      {#snippet empty()}
        <EmptyState variant="compact" title="No git host yet" hint="Agents cannot start a coding job until you add a host at the top of the page." />
      {/snippet}
      {#snippet children(d)}
        {#each d.hosts as h (h.id)}<AdminGitHostRow host={h} />{/each}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>

<Modal open={adding} onClose={close} title="Add git host" width="max-w-lg">
  <div class="space-y-5">
    <Segmented
      options={[
        { id: 'forgejo', label: 'Forgejo / Gitea' },
        { id: 'github', label: 'GitHub' },
      ] as const}
      value={kind}
      onChange={(k) => {
        kind = k
        if (k === 'github' && !baseUrl) baseUrl = 'https://github.com'
      }}
    />
    <SettingsField label="Name" for="git-host-name">
      <Input id="git-host-name" size="sm" bind:value={name} maxlength={80} placeholder={kind === 'github' ? 'GitHub Enterprise' : 'Company Forgejo'} />
    </SettingsField>
    <SettingsField label="Base URL" for="git-host-url" hint="The host's address, starting with https://.">
      <Input
        id="git-host-url"
        size="sm"
        bind:value={baseUrl}
        class="font-mono"
        placeholder={kind === 'github' ? 'https://github.com' : 'https://git.example.com'}
      />
    </SettingsField>
    <SettingsField label="Id" for="git-host-id" hint="A short name agents use when a repository is on two hosts.">
      <Input id="git-host-id" size="sm" bind:value={id} oninput={() => (idTouched = true)} maxlength={40} class="font-mono" placeholder="company-forgejo" />
    </SettingsField>
    <p class="font-sans text-[13px] leading-5 text-muted">
      {kind === 'github'
        ? 'Next, connect it with a GitHub App or an access token.'
        : 'Next, choose the workspace secret that holds its token.'}
    </p>
    {#if error}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</div>{/if}
  </div>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button size="sm" variant="ghost" onclick={close}>Cancel</Button>
      <Button size="sm" disabled={busy || !id.trim() || !name.trim() || !baseUrl.trim()} onclick={() => void create()}>Add host</Button>
    </div>
  {/snippet}
</Modal>
