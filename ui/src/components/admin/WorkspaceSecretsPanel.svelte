<script lang="ts">
  // Admin → Secrets → the credentials agents may USE.
  //
  // THE SHAPE FOLLOWS FROM WHAT CANNOT BE SHOWN. A value goes in once and never
  // comes back — no read path returns one — so a row's job is not content, it is
  // ANSWERING TWO QUESTIONS an operator actually has: what is this, and who can
  // spend it. Everything on a row serves one of those; the handle is there
  // because it is the string a human pastes into an agent's instructions or a ticket, and a
  // credential nobody can address is one nobody uses.
  //
  // A DOC HOLDS ONE OR MANY, and the form does not make you choose upfront: you
  // add entries, and a doc with one entry is a single secret. That is the same
  // decision the store makes, for the same reason — a deploy needs a PAT, a
  // registry password and a signing key TOGETHER, and making somebody create
  // three unrelated secrets is how the wrong one gets used.
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import SecretEntriesEditor, { emptySecretEntry, type SecretEntry } from '@/components/ui/SecretEntriesEditor.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { slide } from '@/lib/motion'
  import { handlesFor, useWorkspaceSecretAction, useWorkspaceSecrets, type WorkspaceSecret } from '@/lib/workspace-secrets'

  /** The add dialog. The page header's "Add credentials" button opens it. */
  let { adding = $bindable(false) }: { adding?: boolean } = $props()

  const query = useWorkspaceSecrets()
  const act = useWorkspaceSecretAction()
  const all = $derived(query.data?.secrets ?? [])

  // SPENT ONE-SHOTS ARE NOT THIS PANEL'S SUBJECT. Chat mints relays into the
  // same table — that is deliberate, one shape to grant, audit and revoke — but
  // a workspace doing it a few times a day would bury the six durable
  // credentials an operator actually manages under a month of dead errands.
  // They are still here, one click away, because "did my key ever reach that
  // agent" is a real question; the row keeps who minted it and when it was
  // spent even after the value itself is destroyed.
  let showSpent = $state(false)
  const isSpent = (s: WorkspaceSecret) =>
    (s.usesRemaining !== null && s.usesRemaining <= 0) || (s.expiresAt !== null && new Date(s.expiresAt).getTime() <= Date.now())
  const spent = $derived(all.filter(isSpent))
  const secrets = $derived(showSpent ? all : all.filter((s) => !isSpent(s)))

  let busy = $state(false)
  let msg = $state<string | null>(null)

  // The draft. `value` lives here and nowhere else — it is cleared the moment
  // the create returns, because a form that keeps a credential in component
  // state after it has been stored is a second copy nobody asked for.
  let name = $state('')
  let title = $state('')
  let note = $state('')
  let relay = $state(false)
  let hosts = $state('')
  let entries = $state<SecretEntry[]>([emptySecretEntry()])

  const reset = () => {
    name = ''
    title = ''
    note = ''
    relay = false
    hosts = ''
    entries = [emptySecretEntry()]
  }

  const create = async () => {
    busy = true
    msg = null
    const clean = entries.filter((e) => e.key.trim() && e.value)
    const res = await act({
      action: 'create',
      name: name.trim(),
      title: title.trim() || name.trim(),
      kind: relay ? 'relay' : 'vault',
      note: note.trim() || null,
      allowedHosts: hosts
        .split(/[\s,]+/)
        .map((h) => h.trim().toLowerCase())
        .filter(Boolean),
      entries: clean.map((e) => ({ key: e.key.trim(), label: e.label.trim() || 'Credential', value: e.value })),
    })
    busy = false
    // CLEARED WHETHER OR NOT IT WORKED. On success the value is stored; on
    // failure it is a credential sitting in a browser tab, and asking somebody
    // to retype it is a far better outcome than leaving it there.
    reset()
    if (res.error) msg = res.error
    else adding = false
  }

  const remove = async (s: WorkspaceSecret) => {
    const ok = await confirm({
      title: `Delete ${s.title}?`,
      message:
        `This deletes the stored value and every grant on it. ${s.grants.length > 0 ? `${s.grants.length} agent${s.grants.length === 1 ? '' : 's'} will stop being able to use it.` : 'No agent has been granted it.'} ` +
        'There is no undo and no copy. If you need it again, enter the value again.',
      confirmLabel: 'Delete',
      danger: true,
    })
    if (!ok) return
    busy = true
    const res = await act({ action: 'delete', name: s.name })
    busy = false
    msg = res.error ?? null
  }

  const revoke = async (s: WorkspaceSecret, agentModel: string) => {
    busy = true
    const res = await act({ action: 'revoke', name: s.name, agentModel })
    busy = false
    msg = res.error ?? null
  }

  const rowMenu = (s: WorkspaceSecret): ContextMenuEntry[] => [
    ...s.grants.map((g): ContextMenuEntry => ({ label: `Remove access for ${g}`, disabled: busy, onSelect: () => void revoke(s, g) })),
    ...(s.grants.length ? (['sep'] as const) : []),
    { label: 'Delete credential', danger: true, disabled: busy, onSelect: () => void remove(s) },
  ]
  const closeDialog = () => {
    reset()
    adding = false
  }

  const COLS = 'grid grid-cols-[minmax(0,1fr)_15rem_8rem_5.5rem_1.75rem] gap-4 px-5'
</script>

<SettingsSection
  title="Agent credentials"
  description="Logins and keys an agent can use without ever seeing the value."
  info="A value is stored once and filled in only at the moment an agent's tool call or push needs it, so it never reaches a model or a provider. Nothing on this page, and no API, can show a value again."
>
  {#snippet action()}
    {#if spent.length > 0}
      <Button size="sm" variant="ghost" onclick={() => (showSpent = !showSpent)}>
        {showSpent ? 'Hide' : 'Show'}
        {spent.length} used up or expired
      </Button>
    {/if}
  {/snippet}
  <SettingsGroup>
    <QueryState query={query} errorTitle="Could not load your agent credentials" errorVariant="compact">
      {#snippet skeleton()}<div class="px-5 py-4"><SkeletonRows rows={3} /></div>{/snippet}
      {#snippet children(_data)}
        {#if secrets.length === 0}
          <SettingsRow
            label={all.length > 0 ? 'No live credentials' : 'No agent credentials yet'}
            hint={all.length > 0
              ? 'Every credential here has been used up or has expired.'
              : 'Add one to let an agent sign in, push or publish without seeing the value.'}
          />
        {:else}
          <div class="{COLS} h-10 items-center font-sans text-xs font-medium leading-4 text-muted">
            <span>Credential</span>
            <span>Can be used at</span>
            <span>Agents</span>
            <span>Last used</span>
            <span></span>
          </div>
          {#each secrets as s (s.id)}
            <div class="{COLS} items-start py-4" transition:slide>
              <div class="min-w-0">
                <div class="flex min-w-0 flex-wrap items-center gap-2">
                  <span class="min-w-0 truncate font-sans text-sm font-medium leading-5 text-fg">{s.title}</span>
                  {#if s.kind === 'relay'}
                    <Chip tone="warn">One use{s.usesRemaining !== null ? ` · ${s.usesRemaining} left` : ''}</Chip>
                  {/if}
                  {#if s.usesRemaining !== null && s.usesRemaining <= 0}
                    <!-- The value behind a spent row is destroyed, not merely
                         unreachable — see resolveHandles. The row is the receipt. -->
                    <Chip tone="neutral">Used up · value destroyed</Chip>
                  {:else if s.expiresAt && new Date(s.expiresAt).getTime() <= Date.now()}
                    <Chip tone="neutral">Expired</Chip>
                  {/if}
                </div>
                {#if s.note}<p class="font-sans text-[13px] leading-5 text-muted">{s.note}</p>{/if}
                <!-- THE HANDLE IS THE POINT OF THE ROW. It is what a human pastes
                     into an agent's instructions, a ticket or a runbook, and a
                     credential nobody can address is one nobody uses. A bundle
                     shows the qualified form because the bare one refuses as
                     ambiguous. -->
                <div class="mt-1 flex flex-wrap gap-x-3 gap-y-0.5">
                  {#each handlesFor(s) as h (h)}
                    <code class="font-mono text-xs leading-5 text-muted">{h}</code>
                  {/each}
                </div>
              </div>
              <!-- USABLE WHERE. An unrestricted credential is the dangerous
                   default, and a default that shows as blank space is one nobody
                   ever notices — so it says so in words. -->
              <div class="min-w-0 font-sans text-[13px] leading-5">
                {#if s.allowedHosts.length === 0}
                  <span class="text-warning">Any destination</span>
                {:else}
                  {#each s.allowedHosts as h (h)}
                    <code class="block truncate font-mono text-xs leading-5 text-fg">{h}</code>
                  {/each}
                {/if}
              </div>
              <div class="min-w-0 font-sans text-[13px] leading-5">
                {#if s.grants.length === 0}
                  <span class="text-warning" title="This credential does nothing until an agent is granted it.">No agents yet</span>
                {:else}
                  {#each s.grants as g (g)}
                    <code class="block truncate font-mono text-xs leading-5 text-fg">{g}</code>
                  {/each}
                {/if}
              </div>
              <span
                class="font-sans text-[13px] leading-5 tabular-nums text-muted"
                title={s.lastUsedAt ? new Date(s.lastUsedAt).toLocaleString() : undefined}
              >
                {s.lastUsedAt ? relativeTime(s.lastUsedAt) : 'Never'}
              </span>
              <span class="-mt-1 flex justify-end">
                <SettingsRowMenu label={`Actions for ${s.title}`} items={() => rowMenu(s)} />
              </span>
            </div>
          {/each}
        {/if}
      {/snippet}
    </QueryState>
    {#if msg && !adding}<p class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{msg}</p>{/if}
  </SettingsGroup>
</SettingsSection>

<Modal open={adding} onClose={closeDialog} title="Add credentials" width="max-w-2xl">
  <div class="space-y-5">
    <div class="grid gap-5 sm:grid-cols-2">
      <SettingsField label="Name" for="ws-secret-title">
        <Input id="ws-secret-title" size="sm" bind:value={title} placeholder="Deploy credentials" />
      </SettingsField>
      <SettingsField label="Handle" for="ws-secret-name" hint="The short name agents refer to it by.">
        <Input id="ws-secret-name" size="sm" bind:value={name} placeholder="deploy" class="font-mono" />
      </SettingsField>
    </div>
    <SettingsField label="What it is for" for="ws-secret-note">
      <Input id="ws-secret-note" size="sm" bind:value={note} placeholder="Pushing to the release repo" />
    </SettingsField>
    <SettingsField label="Values" hint="A key, a label and the value. Add a row for each part, such as a username and a password.">
      <!-- The shared editor still draws its own small mono caption; the field
           label above replaces it here. -->
      <div class="[&>p:first-child]:hidden">
        <SecretEntriesEditor bind:entries keyPlaceholder="github_pat" labelPlaceholder="GitHub token" valuePlaceholder="Value, stored and never shown again" />
      </div>
    </SettingsField>
    <SettingsField
      label="Where it can be used"
      for="ws-secret-hosts"
      hint={hosts.trim()
        ? 'Talaria will not fill this in for any other destination, or for one it cannot read.'
        : 'Optional host names, separated by spaces. Left empty, an agent can use it anywhere.'}
    >
      <Input id="ws-secret-hosts" size="sm" bind:value={hosts} placeholder="github.com registry.outcrop.dev" class="font-mono" />
    </SettingsField>
    <div class="flex items-center gap-6">
      <div class="min-w-0 flex-1">
        <div class="font-sans text-[13px] font-medium leading-5 text-fg">Use once</div>
        <div class="font-sans text-[13px] leading-5 text-muted">The value is destroyed the first time an agent uses it.</div>
      </div>
      <Toggle title="Use once" checked={relay} onChange={(checked) => (relay = checked)} />
    </div>
    {#if msg}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{msg}</div>{/if}
  </div>
  {#snippet footer()}
    <div class="flex items-center justify-end gap-2">
      <span class="mr-auto font-sans text-[13px] leading-5 text-muted">Grant it to an agent from that agent's page.</span>
      <Button variant="ghost" size="sm" onclick={closeDialog} disabled={busy}>Cancel</Button>
      <Button size="sm" onclick={() => void create()} disabled={busy || !name.trim() || !entries.some((e) => e.key.trim() && e.value)}>
        {busy ? 'Saving' : 'Save credentials'}
      </Button>
    </div>
  {/snippet}
</Modal>
