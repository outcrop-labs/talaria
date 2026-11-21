<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import NoticeRow from '@/components/admin/NoticeRow.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { fromOf, withoutPermission, withPermission, workspaceSecretNotice, type AgentGithubPermissions, type HostPermissions } from '@/lib/agent-github-permissions'
  import { useClock } from '@/lib/clock.svelte'
  import { getList, postJson, putJsonOr } from '@/lib/fetch-json'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { gitHostAgentsKey } from '@/lib/git-host-agents'
  import type { PermissionMap } from '@/lib/git-permissions'
  import { githubPermissionsKey, matrixRows, type LevelChoice, type MatrixRow, type MatrixView } from '@/lib/github-access'
  import { pushToast, toastError } from '@/lib/toast.svelte'

  // What this agent's GitHub token carries on one host: its WHOLE set there,
  // for every repo it holds on the host, or the default (contents and pull
  // requests to write, metadata to read). Each permission is a None / Read /
  // Write control with only the levels GitHub takes it at, beside what the
  // installation holds and what the agent's last token carried. What the
  // installation holds is the most a token can carry, never what this
  // agent's does: nothing reaches its token until it is set here.
  //
  // Admins set them; everyone who may read the agent reads them. Nothing here
  // is a gate: any key GitHub knows may be set, at any level it takes. Each
  // change is the whole next set, a compare-and-set PUT from what was read,
  // applied on change; while one is in flight every control on this host
  // waits for the re-read, because the next write's `from` must be it. A 409
  // re-reads and says so.
  let {
    def,
    data,
    block,
    isAdmin,
    requested,
    checking,
    onCheck,
    onWrote,
  }: {
    def: AgentDef
    data: AgentGithubPermissions
    block: HostPermissions
    isAdmin: boolean
    /** Keys the agent has an open request for on this host. */
    requested: string[]
    checking: boolean
    /** Check token, from a row that says its key is not checked yet. Absent:
     *  this viewer may not check, or no repo is held here. */
    onCheck?: () => void
    /** A write landed: the last Check token answer measured another set. */
    onWrote: () => void
  } = $props()

  const qc = useQueryClient()
  const clock = useClock()
  const autosave = useAutosave()
  const canEdit = $derived(data.canEdit && isAdmin)

  let view = $state<MatrixView>('set')
  let filter = $state('')
  // Keys changed this visit stay listed, at None too, so a misclick is undone
  // where it was made.
  let touched = $state<string[]>([])
  let saving = $state(false)

  const rows = $derived(
    matrixRows(block, data.catalog, { view: canEdit ? view : 'set', filter, requested, touched, nowMs: clock.now }),
  )
  const empty = $derived(rows.repository.length === 0 && rows.organization.length === 0)

  interface Saved {
    ok?: true
    notes?: string[]
    /** A 409's: what the agent carries now. */
    current?: PermissionMap
  }

  const write = async (to: PermissionMap | null, key?: string) => {
    saving = true
    if (key && !touched.includes(key)) touched = [...touched, key]
    try {
      const res = await putJsonOr<Saved>(`/api/workbench/github-permissions/${def.id}`, { host: block.host, from: fromOf(block), to }, [409])
      onWrote()
      if (res.current) {
        pushToast({
          title: 'These permissions changed since you read them',
          body: 'Showing what is set now. Make your change again if it still applies.',
        })
      } else {
        autosave.flash(key ? `${block.host}:${key}` : `${block.host}:reset`)
        if (res.notes?.length) pushToast({ title: `Saved GitHub permissions on ${block.name}`, body: res.notes.join(' ') })
      }
    } catch (e) {
      toastError('Could not save GitHub permissions', e)
    } finally {
      // Busy until the re-read lands: an edit against the set from before
      // this write would send a stale `from` and come back a 409.
      await Promise.all([
        qc.invalidateQueries({ queryKey: githubPermissionsKey(def.id) }),
        qc.invalidateQueries({ queryKey: gitHostAgentsKey(block.host) }),
      ])
      saving = false
    }
  }

  const choose = (r: MatrixRow, choice: LevelChoice) => {
    if ((r.level ?? 'none') === choice) return
    const set = block.permissions ?? {}
    void write(choice === 'none' ? withoutPermission(set, r.key) : withPermission(set, r.key, choice), r.key)
  }

  // ── The workspace secret that answers git here first ─────────────────────
  // The agent's grant decides it: a secret granted to the agent directly, or
  // through a folder, and spendable at this host. An admin revokes a direct
  // grant in place, with the same write the agent's Secrets section makes; a
  // folder grant is revoked from the folder, so this names it and links there.
  const secretName = $derived(block.workspaceSecret?.trim() || null)
  const handles = createQuery(() => ({
    queryKey: ['agent-held-handles', def.model],
    queryFn: (): Promise<Array<{ name: string; via: 'direct' | 'folder'; folder: string | null }>> =>
      getList(`/api/admin/workspace-secrets?agent=${encodeURIComponent(def.model)}`, 'held'),
    enabled: isAdmin && secretName !== null,
  }))
  const grant = $derived(secretName ? (handles.data ?? []).find((h) => h.name === secretName) : undefined)
  let revoking = $state(false)
  const revoke = async () => {
    if (!secretName) return
    const ok = await confirm({
      title: `Revoke ${secretName}`,
      message: `Revoke ${secretName} from ${def.displayName}? Its git at ${block.name} then uses the App's token with the permissions below. Anything else ${def.displayName} used ${secretName} for stops too.`,
      confirmLabel: 'Revoke',
      danger: true,
    })
    if (!ok) return
    revoking = true
    try {
      await postJson('/api/admin/workspace-secrets', { action: 'revoke', name: secretName, agentModel: def.model })
    } catch (e) {
      toastError(`Could not revoke ${secretName}`, e)
    }
    await Promise.all([
      qc.invalidateQueries({ queryKey: ['agent-held-handles', def.model] }),
      qc.invalidateQueries({ queryKey: ['workspace-secrets'] }),
      qc.invalidateQueries({ queryKey: githubPermissionsKey(def.id) }),
    ])
    revoking = false
  }

  const developerOff = $derived(!def.developer)
  const description = $derived(
    `What the App's installation accepted is the most a token can carry; nothing here reaches the token until it is set.${canEdit ? '' : ' An admin sets these.'}`,
  )
  const GROUPS = [
    ['repository', 'Repository'],
    ['organization', 'Organization'],
  ] as const
</script>

<SettingsSection title={`Permissions on ${block.name}`} {description} info="Each permission says what the installation holds and what this agent's last token carried. A token carries the agent's set as far as the installation holds it, for every repository the agent holds on this host.">
  {#snippet action()}
    <span class="font-sans text-[13px] leading-5 text-muted">{block.isDefault ? 'Default' : 'Custom'}</span>
    {#if canEdit && !block.isDefault}
      <Button variant="link" size="sm" disabled={saving} onclick={() => void write(null)}>Reset to default</Button>
    {/if}
  {/snippet}
  <div class="space-y-4">
    {#if secretName || developerOff}
      <SettingsGroup>
        {#if secretName}
          <NoticeRow status="warn" data-testid="github-access-secret">
            {workspaceSecretNotice(block)}{#if grant?.via === 'folder'}{` It reaches the agent through the folder ${grant.folder ?? ''}.`} <a href="/admin/secrets" class="text-accent hover:underline">Secrets ›</a>{/if}
            {#snippet action()}
              {#if isAdmin && grant?.via === 'direct'}
                <DangerLink onClick={() => void revoke()} disabled={revoking}>Revoke from {def.displayName}</DangerLink>
              {/if}
            {/snippet}
          </NoticeRow>
        {/if}
        {#if developerOff}
          <NoticeRow status="idle">The Developer switch is off, so its git gets no credential, whatever is set here.</NoticeRow>
        {/if}
      </SettingsGroup>
    {/if}

    <div class="flex flex-wrap items-center gap-2">
      <Input size="sm" class="min-w-0 flex-1 basis-56" placeholder="Find a permission" aria-label={`Find a permission on ${block.name}`} bind:value={filter} />
      {#if canEdit}
        <Segmented
          size="md"
          label="Which permissions to list"
          value={view}
          onChange={(v) => (view = v)}
          options={[
            { id: 'set', label: 'Its set' },
            { id: 'all', label: `All ${data.catalog.length}` },
          ]}
        />
      {/if}
    </div>

    {#if empty}
      <EmptyState variant="inline" title={filter.trim() ? `No permission matches "${filter.trim()}"` : 'Nothing in its set'} />
    {/if}
    {#each GROUPS as [scope, title] (scope)}
      {#if rows[scope].length > 0}
        <div class="space-y-2">
          <div class="font-sans text-[13px] font-medium leading-5 text-muted">{title}</div>
          <SettingsGroup>
            {#each rows[scope] as r (r.key)}
              <SettingsRow label={r.label} saved={autosave.saved(`${block.host}:${r.key}`)}>
                {#snippet meta()}
                  <span class="shrink-0 font-mono text-xs text-muted">{r.key}</span>
                  {#if r.impact}<InfoTip text={r.impact} />{/if}
                  {#if r.key === 'metadata'}<InfoTip text="GitHub puts metadata: read on every token." />{/if}
                {/snippet}
                {#snippet hint()}
                  {#if r.held.word}
                    <span class={r.held.tone === 'warn' ? 'text-warning' : undefined}>{r.held.word}</span>
                    {#if r.held.link}<a href={r.held.link.href} target="_blank" rel="noreferrer" class="text-accent hover:underline">{r.held.link.label}</a>{/if}
                    {' · '}
                  {/if}
                  <span title={r.token.title} class="inline-flex items-baseline gap-1.5"
                    ><StatusDot status={r.token.tone} class="translate-y-[-1px]" /><span class={r.token.tone === 'warn' ? 'text-warning' : undefined}
                      >{r.token.word}{#if r.token.check && onCheck}:{/if}</span
                    ></span
                  >
                  {#if r.token.check && onCheck}
                    <Button variant="link" size="sm" class="h-auto p-0 text-[13px]" disabled={checking} onclick={onCheck}>Check token</Button>
                  {/if}
                  {#if r.requested}{' · '}<span class="text-warning">requested by the agent</span>{/if}
                {/snippet}
                {#if canEdit}
                  <Segmented
                    label={`${r.label} permission level`}
                    value={r.level ?? 'none'}
                    disabled={saving}
                    options={r.options}
                    onChange={(choice) => choose(r, choice)}
                  />
                {:else}
                  <span class="font-sans text-[13px] leading-5 text-fg">{r.level ?? '—'}</span>
                {/if}
              </SettingsRow>
            {/each}
          </SettingsGroup>
        </div>
      {/if}
    {/each}
  </div>
</SettingsSection>
