<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import AdminGitHostCard from './AdminGitHostCard.svelte'
  import { useGitHosts, useGitHostsRefresh, type GitHostKind } from './git-hosts'

  /** Admin → Git hosts: the git providers this instance works repositories
   *  on. Operators choose their providers — GitHub (github.com or an
   *  Enterprise Server) or Forgejo / Gitea — and agents reach a repo only
   *  through the host it lives on and a per-agent grant. */
  const query = useGitHosts()
  const refresh = useGitHostsRefresh()

  let adding = $state(false)
  let kind = $state<GitHostKind>('forgejo')
  let id = $state('')
  let name = $state('')
  let baseUrl = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)

  const open = () => {
    kind = 'forgejo'
    id = ''
    name = ''
    baseUrl = ''
    error = null
    adding = true
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
      adding = false
      idTouched = false
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await refresh()
  }
</script>

<Panel class="mt-4 p-3 sm:p-4">
  <SectionHeader
    title="Git hosts"
    info="Where your agents' repositories live. Add each git host they work on; which agent may touch which repo stays an explicit per-agent grant. A GitHub host connects with a GitHub App or an access token; a Forgejo / Gitea host authenticates with a workspace secret."
  >
    {#snippet action()}
      <Button size="sm" variant="outline" onclick={open}>Add host</Button>
    {/snippet}
  </SectionHeader>
  <QueryState {query} errorTitle="Could not load the git hosts" errorVariant="compact" isEmpty={(d) => d.hosts.length === 0}>
    {#snippet skeleton()}<SkeletonRows rows={2} />{/snippet}
    {#snippet empty()}
      <EmptyState variant="compact" title="No git host yet" hint="Add the host your repositories live on — agents cannot start a workbench job until one is connected." />
    {/snippet}
    {#snippet children(d)}
      <div class="space-y-3">
        {#each d.hosts as h (h.id)}<AdminGitHostCard host={h} />{/each}
      </div>
    {/snippet}
  </QueryState>
</Panel>

<Modal open={adding} onClose={() => (adding = false)} title="Add a git host" width="max-w-lg">
  <div class="space-y-3">
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
    <label class="block space-y-1 text-xs text-muted">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</span>
      <Input size="sm" bind:value={name} maxlength={80} placeholder={kind === 'github' ? 'GitHub Enterprise' : 'Company Forgejo'} />
    </label>
    <label class="block space-y-1 text-xs text-muted">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Base URL</span>
      <Input size="sm" bind:value={baseUrl} class="font-mono" placeholder={kind === 'github' ? 'https://github.com or https://ghe.example.com' : 'https://git.example.com'} />
      <span class="block text-[11px]">The host's origin, https:// (plain http:// only to this machine's loopback).</span>
    </label>
    <label class="block space-y-1 text-xs text-muted">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Id</span>
      <Input size="sm" bind:value={id} oninput={() => (idTouched = true)} maxlength={40} class="font-mono" placeholder="company-forgejo" />
      <span class="block text-[11px]">A short slug agents name the host by when a repo is on two hosts.</span>
    </label>
    <p class="text-xs text-muted">
      {kind === 'github'
        ? 'Once added, open it to connect a GitHub App (recommended) or an access token.'
        : 'Once added, open it to name the workspace secret that holds its token.'}
    </p>
    {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
  </div>
  {#snippet footer()}
    <Button size="sm" variant="ghost" onclick={() => (adding = false)}>Cancel</Button>
    <Button size="sm" disabled={busy || !id.trim() || !name.trim() || !baseUrl.trim()} onclick={() => void create()}>Add host</Button>
  {/snippet}
</Modal>
