<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { AlertTriangle, Folder, KeyRound } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { delJsonGone, errorMessage, getList, postJson, putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { listStagger, slide } from '@/lib/motion'

  interface SecretMeta {
    name: string
    updatedBy: string | null
    updatedAt: string
    /** Why this name is refused now — present only on a row saved before the rule. Such a row is not loaded. */
    refused?: string
    /** CONFIG_* only: where the agent's current config names it. Empty when nothing does. */
    usedAt?: string[]
  }

  /** A credential sealed out of the agent's own config (federation). */
  const sealed = (s: SecretMeta) => s.name.startsWith('CONFIG_')

  // Per-agent secrets: UI-configured env vars, encrypted at rest, write-only
  // (values never come back). Materialized into the agent's env at render — no
  // hand edits to fleet/.env unless you're an advanced deployer who wants to.
  let { agentId, agentModel, agentLabel }: { agentId: string; agentModel: string; agentLabel: string } = $props()

  // ── WHAT IT CAN SPEND WITHOUT SEEING ───────────────────────────────────────
  //
  // The section below this one is env vars, and env vars are PLAINTEXT IN THE
  // CONTAINER: every fleet agent runs a harness with a shell, so `echo $NAME`
  // returns the value and it is one step from the model's context and a
  // provider. That is the leak the handle store exists to close, and until now
  // the two lived on different pages with nothing saying they were alternatives.
  //
  // So both are here, in this order, with the safe one first.
  const held = createQuery(() => ({
    queryKey: ['agent-held-handles', agentModel],
    // The `?agent=` form, not the full listing: a credential reaching this agent
    // through a shared FOLDER is invisible in `SecretDoc.grants`, and a page
    // that said "no credentials" about an agent holding four would be worse
    // than no page.
    queryFn: (): Promise<Array<{ name: string; title: string; key: string; label: string; via: 'direct' | 'folder'; folder: string | null }>> =>
      getList(`/api/admin/workspace-secrets?agent=${encodeURIComponent(agentModel)}`, 'held'),
  }))
  const vault = createQuery(() => ({
    queryKey: ['workspace-secrets'],
    queryFn: (): Promise<Array<{ name: string; title: string; grants: string[]; entries: Array<{ key: string; label: string }> }>> =>
      getList('/api/admin/workspace-secrets', 'secrets'),
  }))
  const grantable = $derived((vault.data ?? []).filter((v) => !v.grants.includes(agentModel)))

  let granting = $state(false)
  const setGrant = async (name: string, on: boolean) => {
    granting = true
    // Swallowed on purpose: these rows and the Combobox have no error slot,
    // and the invalidates below re-read both lists — a failed grant lands as
    // "still not granted", which is the truth.
    await postJson('/api/admin/workspace-secrets', { action: on ? 'grant' : 'revoke', name, agentModel }).catch(() => {})
    granting = false
    await qc.invalidateQueries({ queryKey: ['agent-held-handles', agentModel] })
    await qc.invalidateQueries({ queryKey: ['workspace-secrets'] })
  }

  const qc = useQueryClient()
  const key = () => ['agent-secrets', agentId]
  // GET is 200 `{ secrets }`, 403, or 404 for an agent that no longer exists:
  // EVERY non-2xx is a failure. It used to answer `[]`, which rendered "No
  // secrets": an operator reading that would go add the token that is
  // already there.
  const query = createQuery(() => ({
    queryKey: key(),
    queryFn: (): Promise<SecretMeta[]> => getList<SecretMeta>(`/api/fleet/agents/${agentId}/secrets`, 'secrets'),
  }))
  let name = $state('')
  let value = $state('')
  let busy = $state(false)
  let err = $state<string | null>(null)
  const nameOk = $derived(/^[A-Z][A-Z0-9_]{1,63}$/.test(name))

  const save = async () => {
    busy = true
    err = null
    try {
      await putJson(`/api/fleet/agents/${agentId}/secrets`, { name, value })
      name = ''
      value = ''
      await qc.invalidateQueries({ queryKey: key() })
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }

  // A sealed credential its config still names cannot go alone: the agent
  // would read the handle's own text as the value. Removing it takes the
  // setting out too, in a new version — so the confirm says which one. A
  // stale list (the config changed since) gets the server's 409, which names
  // the paths, as the toast.
  const remove = async (s: SecretMeta) => {
    const used = s.usedAt ?? []
    const message = used.length
      ? `Remove ${s.name}? This agent's config uses it at ${used.join(', ')}. Removing it also takes ${used.length === 1 ? 'that setting' : 'those settings'} out of the config, in a new version, so the agent never reads the placeholder text as the value. Both apply on its next start.`
      : `Remove ${s.name}? The agent loses it on its next start.`
    const confirmLabel = used.length ? 'Remove secret and setting' : 'Remove'
    if (!(await confirm({ title: 'Remove secret', message, confirmLabel, danger: true }))) return
    try {
      // A 404 is an agent that no longer exists, and its secrets went with it.
      await delJsonGone(`/api/fleet/agents/${agentId}/secrets`, { name: s.name, removeSetting: used.length > 0 })
      // Off the list now, not only after the re-read: for an agent that is
      // gone, that re-read fails, and a failed refetch keeps the last list.
      qc.setQueryData<SecretMeta[]>(key(), (rows) => rows?.filter((r) => r.name !== s.name))
    } catch (e) {
      // Fire-and-forget from a row button; the err line belongs to the form.
      toastError('Remove failed', e)
    }
    await qc.invalidateQueries({ queryKey: key() })
    // A setting went with it: a new version, so the def's current one moved.
    if (used.length) {
      await qc.invalidateQueries({ queryKey: ['agent-versions', agentId] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    }
  }
</script>

{#snippet keyIcon()}<KeyRound size={22} />{/snippet}

<div class="space-y-8">
  <!-- ── SAFE: spends without seeing ────────────────────────────────────────── -->
  <SettingsSection
    title="Credentials it can use"
    description="Talaria fills in the real value when a tool spends it, so the agent never sees it."
    info={`${agentLabel} is given a handle. The value never enters its context and never reaches a model provider. Prefer this whenever a credential only has to be used.`}
  >
    {#snippet action()}
      {#if grantable.length > 0}
        <Combobox
          options={grantable.map((v) => ({ value: v.name, label: v.title, sub: v.entries.map((e) => e.label).join(', ') }))}
          selected={[]}
          size="sm"
          placeholder="Grant a credential"
          disabled={granting}
          onChange={(v) => v[0] && void setGrant(v[0], true)}
          class="w-[280px]"
        />
      {/if}
    {/snippet}
    {#if held.data && held.data.length > 0}
      <div use:listStagger>
        <SettingsGroup>
          {#each held.data as h (h.name + h.key)}
            <SettingsRow label={h.title} hint={h.label}>
              {#snippet icon()}<KeyRound size={16} />{/snippet}
              {#snippet meta()}<code class="truncate font-mono text-xs text-muted">«secret:{h.name}»</code>{/snippet}
              {#if h.via === 'folder'}
                <!-- REVOKED FROM THE FOLDER, NOT FROM HERE. Offering a control that
                     cannot do what it says is worse than offering none. -->
                <span class="flex items-center gap-1.5 font-sans text-[13px] leading-5 text-muted">
                  <Folder size={14} aria-hidden="true" />
                  From the folder {h.folder}
                </span>
              {:else}
                <DangerLink onClick={() => void setGrant(h.name, false)} disabled={granting}>Revoke</DangerLink>
              {/if}
            </SettingsRow>
          {/each}
        </SettingsGroup>
      </div>
    {:else}
      <SettingsGroup>
        <p class="px-5 py-4 font-sans text-[13px] leading-5 text-muted">None yet.</p>
      </SettingsGroup>
    {/if}
  </SettingsSection>

  <!-- ── UNSAFE-BY-CONSTRUCTION: plaintext in the container ─────────────────── -->
  <SettingsSection
    title="Environment variables"
    description={`Written into its container as plain text. ${agentLabel} can read them.`}
    info="Anything running in the container, including the shell its harness uses, can print these. Use one only when a process in the container reads the variable itself. A CONFIG_ entry is the exception: a credential sealed out of its own config, which Hermes reads and keeps out of every command it runs."
  >
    <QueryState {query} errorTitle="Could not load this agent's secrets" errorVariant="compact">
      {#snippet skeleton()}
        <SettingsGroup aria-hidden="true">
          {#each [0, 1] as i (i)}
            <div class="flex min-h-[60px] items-center gap-3 px-5 py-4">
              <Skeleton class="h-3.5 w-40 rounded-full" />
              <Skeleton class="ml-auto h-3 w-24 rounded-full" />
            </div>
          {/each}
        </SettingsGroup>
      {/snippet}
      {#snippet empty()}
        <EmptyState icon={keyIcon} title="No variables" hint="Everything it needs comes from the shared platform environment." />
      {/snippet}
      {#snippet children(secrets)}
        <div use:listStagger>
          <SettingsGroup>
            {#each secrets as s (s.name)}
              <SettingsRow>
                {#snippet label()}<code class="font-mono text-[13px] font-normal">{s.name}</code>{/snippet}
                {#snippet hint()}
                  {#if s.refused}
                    <!-- Saved before the rule that refuses this name. The render
                         leaves it out of the container, so say why, beside the remove. -->
                    <span class="flex items-start gap-1.5 text-warning">
                      <AlertTriangle size={14} class="mt-0.5 shrink-0" aria-hidden="true" />
                      Not loaded: {s.refused}.
                    </span>
                  {:else if sealed(s)}
                    {#if s.usedAt?.length}
                      A config credential, used at <span class="font-mono text-xs text-fg">{s.usedAt.join(', ')}</span>. The agent's commands do not see it.
                    {:else}
                      A config credential its current config does not use.
                    {/if}
                  {:else}
                    Set by {s.updatedBy ?? 'someone'}, {relativeTime(s.updatedAt)}
                  {/if}
                {/snippet}
                <DangerLink onClick={() => void remove(s)}>Remove</DangerLink>
              </SettingsRow>
            {/each}
          </SettingsGroup>
        </div>
      {/snippet}
    </QueryState>
    <div class="mt-4 flex flex-wrap items-center gap-2">
      <Input
        size="sm"
        value={name}
        oninput={(e) => (name = e.currentTarget.value.toUpperCase().replace(/[^A-Z0-9_]/g, '_'))}
        onkeydown={submitOnEnter(() => !busy && nameOk && value && void save())}
        placeholder="FIGMA_TOKEN"
        aria-label="Variable name"
        class="w-[280px] font-mono"
      />
      <Input
        size="sm"
        bind:value
        onkeydown={submitOnEnter(() => !busy && nameOk && value && void save())}
        placeholder="Value, never shown again"
        aria-label="Variable value"
        type="password"
        class="min-w-0 flex-1"
      />
      <Button variant="outline" size="md" class="shrink-0" onclick={() => void save()} disabled={busy || !nameOk || !value}>
        {busy ? 'Saving' : 'Set variable'}
      </Button>
    </div>
    {#if name && !nameOk}<p class="mt-2 font-sans text-[13px] leading-5 text-muted">Capital letters, digits and underscores, 2 to 64 characters, starting with a letter.</p>{/if}
    {#if err}<p transition:slide={{ duration: 150 }} class="mt-2 font-sans text-[13px] leading-5 text-danger" role="alert">{err}</p>{/if}
  </SettingsSection>
</div>
