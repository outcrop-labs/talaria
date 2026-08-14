<script lang="ts">
  // Admin → Secrets → the credentials agents may USE.
  //
  // THE SHAPE FOLLOWS FROM WHAT CANNOT BE SHOWN. A value goes in once and never
  // comes back — no read path returns one — so a row's job is not content, it is
  // ANSWERING TWO QUESTIONS an operator actually has: what is this, and who can
  // spend it. Everything on a row serves one of those; the handle is there
  // because it is the string a human pastes into a soul or a ticket, and a
  // credential nobody can address is one nobody uses.
  //
  // A DOC HOLDS ONE OR MANY, and the form does not make you choose upfront: you
  // add entries, and a doc with one entry is a single secret. That is the same
  // decision the store makes, for the same reason — a deploy needs a PAT, a
  // registry password and a signing key TOGETHER, and making somebody create
  // three unrelated secrets is how the wrong one gets used.
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { listStagger, slide } from '@/lib/motion'
  import { handlesFor, useWorkspaceSecretAction, useWorkspaceSecrets, type WorkspaceSecret } from '@/lib/workspace-secrets'
  import { KeyRound, Plus, Trash2, X } from '@lucide/svelte'

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
  let open = $state(false)

  // The draft. `value` lives here and nowhere else — it is cleared the moment
  // the create returns, because a form that keeps a credential in component
  // state after it has been stored is a second copy nobody asked for.
  let name = $state('')
  let title = $state('')
  let note = $state('')
  let relay = $state(false)
  let hosts = $state('')
  let entries = $state<Array<{ key: string; label: string; value: string }>>([{ key: '', label: '', value: '' }])

  const reset = () => {
    name = ''
    title = ''
    note = ''
    relay = false
    hosts = ''
    entries = [{ key: '', label: '', value: '' }]
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
    else open = false
  }

  const remove = async (s: WorkspaceSecret) => {
    const ok = await confirm({
      title: `Delete ${s.title}?`,
      message:
        `This deletes the stored value and every grant on it. ${s.grants.length > 0 ? `${s.grants.length} agent${s.grants.length === 1 ? '' : 's'} will stop being able to use it.` : 'No agent has been granted it.'} ` +
        'There is no undo and no copy — if you need it again, enter the value again.',
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
</script>

<Panel>
  <SectionHeader
    title="Agent credentials"
    info="Credentials an agent can USE without ever reading one. The value is sealed here and substituted at the boundary that spends it — a tool call, a push — so it never enters a model's context and never reaches a provider. Values are write-only: nothing on this page, and no API, can show one again."
  />

  {#if query.isPending}
    <Skeleton class="h-24" />
  {:else if query.isError}
    <QueryError error={query.error} />
  {:else}
    {#if secrets.length === 0}
      <EmptyState
        variant="compact"
        title={all.length > 0 ? 'No live credentials' : 'No agent credentials yet'}
        hint={all.length > 0
          ? 'Every credential here has been spent or has expired.'
          : 'Add one to let an agent push, publish or authenticate without the value ever entering its context.'}
      />
    {:else}
      <ul class="space-y-2" use:listStagger>
        {#each secrets as s (s.id)}
          <li class="rounded-md border border-line p-3" transition:slide>
            <div class="flex flex-wrap items-baseline gap-2">
              <KeyRound size={14} class="text-muted" aria-hidden="true" />
              <span class="font-sans text-sm text-fg">{s.title}</span>
              {#if s.kind === 'relay'}
                <Chip tone="warn">one-shot{s.usesRemaining !== null ? ` · ${s.usesRemaining} left` : ''}</Chip>
              {/if}
              {#if s.usesRemaining !== null && s.usesRemaining <= 0}
                <!-- The value behind a spent row is destroyed, not merely
                     unreachable — see resolveHandles. The row is the receipt. -->
                <Chip tone="neutral">spent · value destroyed</Chip>
              {:else if s.expiresAt && new Date(s.expiresAt).getTime() <= Date.now()}
                <Chip tone="neutral">expired</Chip>
              {/if}
              <span class="ml-auto font-mono text-[10px] text-ink-dim">
                {s.lastUsedAt ? `last used ${new Date(s.lastUsedAt).toLocaleDateString()}` : 'never used'}
              </span>
            </div>

            {#if s.note}<p class="mt-1 font-sans text-xs text-muted">{s.note}</p>{/if}

            <!-- THE HANDLE IS THE POINT OF THE ROW. It is what a human pastes
                 into a soul, a ticket or a runbook, and a credential nobody can
                 address is one nobody uses. A bundle shows the qualified form
                 because the bare one refuses as ambiguous. -->
            <div class="mt-2 flex flex-wrap gap-1.5">
              {#each handlesFor(s) as h (h)}
                <code class="rounded bg-panel px-1.5 py-0.5 font-mono text-[11px] text-accent">{h}</code>
              {/each}
            </div>

            <!-- SPENDABLE WHERE. An unrestricted credential is the dangerous
                 default, and a default that shows as blank space is one nobody
                 ever notices — so it says so in words. -->
            <div class="mt-2 flex flex-wrap items-center gap-1.5">
              <span class="font-mono text-[10px] text-ink-dim">spendable at</span>
              {#if s.allowedHosts.length === 0}
                <span class="font-sans text-xs text-warning">any destination</span>
              {:else}
                {#each s.allowedHosts as h (h)}
                  <code class="rounded bg-panel px-1.5 py-0.5 font-mono text-[11px] text-fg">{h}</code>
                {/each}
              {/if}
            </div>

            <div class="mt-2 flex flex-wrap items-center gap-1.5">
              <span class="font-mono text-[10px] text-ink-dim">granted to</span>
              {#if s.grants.length === 0}
                <span class="font-sans text-xs text-warning">nobody — this credential is inert until an agent is granted it</span>
              {:else}
                {#each s.grants as g (g)}
                  <button
                    type="button"
                    onclick={() => revoke(s, g)}
                    disabled={busy}
                    title="Revoke {g}"
                    class="inline-flex items-center gap-1 rounded border border-line px-1.5 py-0.5 font-mono text-[10px] text-muted hover:text-fg"
                  >
                    {g}
                    <X size={10} aria-hidden="true" />
                  </button>
                {/each}
              {/if}
              <Button size="sm" variant="ghost" onclick={() => remove(s)} disabled={busy} class="ml-auto">
                <Trash2 size={13} aria-hidden="true" />
                Delete
              </Button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    <div class="mt-3">
      {#if !open}
        <div class="flex flex-wrap items-center gap-2">
          <Button size="sm" onclick={() => (open = true)}>
            <Plus size={14} aria-hidden="true" />
            Add credentials
          </Button>
          {#if spent.length > 0}
            <Button size="sm" variant="ghost" onclick={() => (showSpent = !showSpent)}>
              {showSpent ? 'Hide' : 'Show'}
              {spent.length} spent or expired
            </Button>
          {/if}
        </div>
      {:else}
        <div class="rounded-md border border-line p-3" transition:slide>
          <div class="grid gap-2 sm:grid-cols-2">
            <label class="font-sans text-xs text-muted">
              Handle name
              <input bind:value={name} placeholder="deploy" class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg" />
            </label>
            <label class="font-sans text-xs text-muted">
              Title
              <input bind:value={title} placeholder="Deploy credentials" class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg" />
            </label>
          </div>
          <label class="mt-2 block font-sans text-xs text-muted">
            What it is for
            <input bind:value={note} placeholder="Pushing to the release repo" class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg" />
          </label>

          <!-- ONE ENTRY OR MANY, and the form does not ask you to decide first.
               A doc with one entry IS a single secret. -->
          <p class="mt-3 font-mono text-[10px] text-ink-dim">entries</p>
          {#each entries as e, i (i)}
            <div class="mt-1 grid gap-1 sm:grid-cols-[1fr_1fr_2fr_auto]">
              <input bind:value={e.key} placeholder="github_pat" class="rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg" />
              <input bind:value={e.label} placeholder="GitHub token" class="rounded border border-line bg-panel px-2 py-1 font-sans text-xs text-fg" />
              <input
                bind:value={e.value}
                type="password"
                autocomplete="off"
                placeholder="value — stored sealed, never shown again"
                class="rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg"
              />
              <Button
                size="sm"
                variant="ghost"
                disabled={entries.length === 1}
                onclick={() => (entries = entries.filter((_, n) => n !== i))}
                title="Remove this entry"
              >
                <X size={13} aria-hidden="true" />
              </Button>
            </div>
          {/each}
          <Button size="sm" variant="ghost" class="mt-1" onclick={() => (entries = [...entries, { key: '', label: '', value: '' }])}>
            <Plus size={13} aria-hidden="true" />
            Another entry
          </Button>

          <label class="mt-3 block font-sans text-xs text-muted">
            Spendable at (optional) — hosts, space or comma separated
            <input
              bind:value={hosts}
              placeholder="github.com  registry.outcrop.dev"
              class="mt-1 w-full rounded border border-line bg-panel px-2 py-1 font-mono text-xs text-fg"
            />
            <span class="mt-1 block text-[11px] text-ink-dim">
              {#if hosts.trim()}
                Talaria refuses to substitute this credential into anything bound elsewhere — including a call whose destination it cannot read.
              {:else}
                Leave empty and this credential can be spent against any destination an agent is talked into.
              {/if}
            </span>
          </label>

          <label class="mt-3 flex items-center gap-2 font-sans text-xs text-muted">
            <input type="checkbox" bind:checked={relay} />
            One-shot — spent the first time an agent uses it
          </label>

          <div class="mt-3 flex items-center gap-2">
            <Button size="sm" onclick={create} disabled={busy || !name.trim() || !entries.some((e) => e.key.trim() && e.value)}>Store</Button>
            <Button
              size="sm"
              variant="ghost"
              onclick={() => {
                reset()
                open = false
              }}
              disabled={busy}>Cancel</Button
            >
            <span class="font-sans text-xs text-muted">Grant it to an agent from that agent's page once it exists.</span>
          </div>
        </div>
      {/if}
    </div>
  {/if}

  {#if msg}<p class="mt-2 font-sans text-xs text-danger">{msg}</p>{/if}
</Panel>
