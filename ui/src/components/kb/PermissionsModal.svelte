<script lang="ts">
  import { createRawSnippet, mount, unmount, type Component, type Snippet } from 'svelte'
  import { useHasPerm } from '@/lib/session'
  import { Globe, Lock, Building2, Bot, X, Check, Copy } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import type { ComboOption } from '@/components/ui/combobox'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useUsers } from '@/lib/users'
  import { useAgents } from '@/lib/agents'
  import { useEditors, type EditPolicy, type GrantRole, type KbEditor, type PermKind, type Visibility } from '@/lib/kb'
  import { cn } from '@/lib/cn'
  import { listStagger } from '@/lib/motion'

  // Google-Drive-style sharing: a list of people + agents each with a Viewer/
  // Editor role, then a "general access" tier (restricted / org / public).
  // Sharing is owner-only; non-owners see it read-only.

  const ROLE_OPTIONS: ComboOption[] = [
    { value: 'viewer', label: 'Viewer', sub: 'Can read it' },
    { value: 'editor', label: 'Editor', sub: 'Can read & edit' },
  ]
  const ACCESS_OPTIONS: ComboOption[] = [
    { value: 'private', label: 'Restricted', sub: 'Only people you add below' },
    { value: 'org', label: 'Organization', sub: 'Everyone in the workspace' },
    { value: 'public', label: 'Anyone with the link', sub: 'Public on the internet' },
  ]

  let {
    open,
    onClose,
    kind,
    id,
    label,
    visibility,
    editPolicy,
    publicSlug,
    canManage,
    inheritable = false,
    inherited = false,
    folderName,
    onSave,
  }: {
    open: boolean
    onClose: () => void
    kind: PermKind
    id: string
    label: string
    visibility: Visibility
    editPolicy: EditPolicy
    publicSlug: string | null
    canManage: boolean
    /** Docs can inherit their folder's access; folders can't. */
    inheritable?: boolean
    inherited?: boolean
    folderName?: string
    onSave: (patch: { visibility: Visibility; editPolicy: EditPolicy; editors: KbEditor[]; permsInherited?: boolean }) => Promise<void>
  } = $props()

  // The grants query below was fixed in an earlier round; the DIRECTORY that
  // resolves those grants to names was not. `{ data: users = [] }` meant a 500
  // on /api/users rendered a live grant as the bare principal id — "u2" sitting
  // in an access list, with nothing anywhere saying the directory read failed
  // and no way to retry it. The picker beside it silently lost every person too.
  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const agentsQuery = useAgents()
  // Until both principal lists resolve, grants would render as raw ids and the
  // add picker would be empty — hold the list's shape instead.
  const principalsLoading = $derived(usersList.pending || agentsQuery.isLoading)
  let vis = $state<Visibility>(visibility)
  // `save()` PUTs the grant list back WHOLESALE, so this list is never allowed
  // to hold a value nobody read off the server. It is derived, not copied: the
  // server's answer is the base and `edits` is an overlay that only exists once
  // the user actually changes something. `known` is the whole invariant — while
  // it is false there is no current list, so there is nothing safe to write.
  //
  // The old shape seeded `useState([])` from a fetch that returned `[]` on
  // failure, which made "the read broke" and "nobody is shared with" the same
  // value; Save then wrote the failure over real grants. Destroyed data, in a
  // browser, not in theory.
  const editors = useEditors(kind, () => id, () => open)
  let edits = $state<KbEditor[] | null>(null)
  const grants = $derived(edits ?? editors.data ?? [])
  // `isError` matters as much as `data`: a modal reopened onto a FAILED refetch
  // still holds the last read's list, and letting Save write that while the
  // surface says "could not load" is the same lie one degree quieter.
  const known = $derived(editors.data !== undefined && !editors.isError)
  let inh = $state(inherited)
  // General audience role for org/public: editors → edit_policy 'org'.
  let orgRole = $state<GrantRole>(editPolicy === 'org' ? 'editor' : 'viewer')
  let saving = $state(false)
  let copied = $state(false)

  $effect(() => {
    // Track the modal's target identity so a reopen — or the same modal aimed
    // at a different item — re-seeds (the React deps: open, kind, id, plus the
    // server truths read below).
    void kind
    void id
    if (!open) return
    vis = visibility
    orgRole = editPolicy === 'org' ? 'editor' : 'viewer'
    inh = inherited
    copied = false
    // Drop any overlay: whatever the reopened modal shows must come from a
    // fresh read, never from what the last one was holding.
    edits = null
  })

  const showInheritBanner = $derived(inheritable && inh)

  const nameOf = (g: KbEditor) => {
    if (g.principalType === 'user') {
      const u = usersList.rows.find((x) => x.id === g.principalId)
      return u?.name ?? u?.email ?? g.principalId
    }
    return (agentsQuery.data?.agents ?? []).find((a) => a.id === g.principalId)?.label ?? g.principalId
  }

  // ComboOption.icon is a zero-arg Snippet, but these options are computed in
  // script from directory data — so the icons are built programmatically: a raw
  // snippet whose setup mounts the real component (the same trick
  // Markdown.svelte uses to hydrate AgentMediaImage into rendered HTML).
  const componentIcon = (component: Component<any>, props: Record<string, unknown>, wrapperClass = 'contents'): Snippet =>
    createRawSnippet(() => ({
      render: () => `<span class="${wrapperClass}"></span>`,
      setup(el) {
        const instance = mount(component, { target: el, props })
        return () => unmount(instance)
      },
    }))

  // People/agents not already granted — the "add" picker.
  const addOptions = $derived.by((): ComboOption[] => {
    const has = new Set(grants.map((g) => `${g.principalType}:${g.principalId}`))
    return [
      ...usersList.rows
        .filter((u) => !has.has(`user:${u.id}`))
        .map((u) => ({
          value: `user:${u.id}`,
          label: u.name ?? u.email ?? u.id,
          sub: u.email ?? 'Person',
          icon: componentIcon(Avatar, { name: u.name ?? u.email, class: 'h-5 w-5 text-[9px]' }),
        })),
      ...(agentsQuery.data?.agents ?? [])
        .filter((a) => !has.has(`agent:${a.id}`))
        .map((a) => ({
          value: `agent:${a.id}`,
          label: a.label,
          sub: `Agent · ${a.role}`,
          icon: componentIcon(Bot, { size: 12 }, 'grid h-5 w-5 place-items-center rounded-full bg-card2 text-muted'),
        })),
    ]
  })

  // Every mutation goes through here so none of them can start from a list the
  // server never sent: editing is refused outright until `known`.
  const editGrants = (fn: (g: KbEditor[]) => KbEditor[]) => {
    if (!known) return
    edits = fn(grants)
  }
  const addGrant = (val: string) => {
    const [type, pid] = [val.slice(0, val.indexOf(':')), val.slice(val.indexOf(':') + 1)]
    if (type !== 'user' && type !== 'agent') return
    editGrants((g) => [...g, { principalType: type, principalId: pid, role: 'viewer' }])
  }
  const setRole = (i: number, role: GrantRole) => editGrants((g) => g.map((x, j) => (j === i ? { ...x, role } : x)))
  const remove = (i: number) => editGrants((g) => g.filter((_, j) => j !== i))

  // Resetting to the folder's access writes `editors: []` on purpose, so it
  // does not need to know the current list. Every other save does.
  const inheritReset = $derived(inheritable && inh)
  const canSave = $derived(canManage && (inheritReset || known))

  const save = async () => {
    // The disabled button is the affordance; this is the invariant. Saving
    // while the current list is unknown is IMPOSSIBLE, not discouraged.
    if (!canSave) return
    saving = true
    try {
      if (inheritReset) {
        // Still inheriting — just make sure it's marked inherited (reset case).
        await onSave({ visibility: vis, editPolicy: 'owner', editors: [], permsInherited: true })
      } else {
        const editPolicyNext: EditPolicy = vis !== 'private' && orgRole === 'editor' ? 'org' : 'owner'
        await onSave({ visibility: vis, editPolicy: editPolicyNext, editors: grants, ...(inheritable ? { permsInherited: false } : {}) })
      }
      onClose()
    } finally {
      saving = false
    }
  }

  // Publishing to the open web is its own permission; without it the tier
  // simply isn't offered (the server enforces regardless).
  const mayPublish = useHasPerm('artifacts.publish')
  const accessOptions = $derived(mayPublish.current ? ACCESS_OPTIONS : ACCESS_OPTIONS.filter((o) => o.value !== 'public'))
  const publicBase = $derived(kind === 'artifacts' ? 'a' : kind === 'spaces' ? 'kb/space' : 'kb')
  const publicUrl = $derived(publicSlug ? `${typeof window !== 'undefined' ? window.location.origin : ''}/${publicBase}/${publicSlug}` : null)
  const AccessIcon = $derived(vis === 'public' ? Globe : vis === 'org' ? Building2 : Lock)
</script>

<Modal {open} {onClose} title={`Share “${label}”`}>
  <div class="space-y-5">
    {#if showInheritBanner}
      <div class="rounded-lg border border-line bg-panel p-4 text-sm">
        <div class="mb-1 font-medium text-fg">Access follows the folder{folderName ? ` “${folderName}”` : ''}</div>
        <p class="mb-3 text-xs text-muted">
          This document shares whoever the folder is shared with. Customize it to give this doc its own,
          more or less restrictive, access.
        </p>
        {#if canManage}
          <Button variant="outline" size="sm" onclick={() => (inh = false)}>
            Customize for this doc
          </Button>
        {/if}
      </div>
    {:else}
      <!-- People & agents -->
      <div>
        <!-- §8 section header: 10px mono uppercase 0.08em ink-dim. -->
        <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">People &amp; agents</div>
        {#if canManage}
          <Combobox
            options={addOptions}
            selected={[]}
            onChange={(v) => v[0] && addGrant(v[0])}
            placeholder="Add a person or agent"
            size="sm"
            disabled={!known}
            class="mb-2"
          />
        {/if}
        <!-- Three answers, three renderings. The failed read used to render as
             the third one — an empty roster over live grants. -->
        {#if editors.isError}
          <div class="px-1 py-2">
            <QueryError
              variant="inline"
              title="Could not load who this is shared with"
              error={editors.error}
              onRetry={() => void editors.refetch()}
            />
            <p class="mt-2 text-[11px] text-muted">
              Sharing can’t be saved until this loads; saving now would write an empty list over the real one.
            </p>
          </div>
        {:else if principalsLoading || !known}
          <SkeletonRows rows={3} avatar class="px-1 py-2" />
        {:else}
          <div class="space-y-1" use:listStagger>
            <!-- The grants below are real; it is their NAMES that are missing.
                 Without this line an id in an access list reads as the grant
                 itself being junk. -->
            {#if usersList.notice}<QueryError {...usersList.notice} />{/if}
            <!-- Owner row (implicit editor) -->
            <div class="flex items-center gap-2.5 rounded-lg px-1 py-1.5">
              <Avatar name={label} class="h-7 w-7 shrink-0 text-[10px]" />
              <span class="min-w-0 flex-1 truncate text-sm text-fg">Owner</span>
              <span class="shrink-0 text-xs text-muted">Owner</span>
            </div>
            {#each grants as g, i (`${g.principalType}:${g.principalId}`)}
              <div class="flex items-center gap-2.5 rounded-lg px-1 py-1">
                {#if g.principalType === 'user'}
                  <Avatar name={nameOf(g)} class="h-7 w-7 shrink-0 text-[10px]" />
                {:else}
                  <span class="grid h-7 w-7 shrink-0 place-items-center rounded-full bg-card2 text-muted"><Bot size={14} /></span>
                {/if}
                <span class="min-w-0 flex-1 truncate text-sm text-fg">{nameOf(g)}</span>
                {#if canManage}
                  <Combobox
                    options={ROLE_OPTIONS}
                    selected={[g.role]}
                    onChange={(v) => v[0] && setRole(i, v[0] as GrantRole)}
                    searchable={false}
                    size="sm"
                    class="w-28 shrink-0"
                  />
                  <button type="button" onclick={() => remove(i)} class="shrink-0 rounded p-1 text-muted transition-colors hover:text-danger" title="Remove">
                    <X size={14} />
                  </button>
                {:else}
                  <span class="shrink-0 text-xs capitalize text-muted">{g.role}</span>
                {/if}
              </div>
            {/each}
            {#if grants.length === 0}<div class="px-1 py-1 text-xs text-muted">No one else has been added.</div>{/if}
          </div>
        {/if}
      </div>

      <!-- General access -->
      <div class="border-t border-line-subtle pt-4">
        <div class="mb-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">General access</div>
        <div class="flex items-center gap-2.5">
          <span class={cn('grid h-9 w-9 shrink-0 place-items-center rounded-full', vis === 'private' ? 'bg-card2 text-muted' : 'bg-accent/15 text-accent')}>
            <AccessIcon size={16} />
          </span>
          <Combobox
            options={accessOptions}
            selected={[vis]}
            onChange={(v) => v[0] && (vis = v[0] as Visibility)}
            searchable={false}
            disabled={!canManage}
            size="sm"
            class="min-w-0 flex-1"
          />
          {#if vis !== 'private'}
            <Combobox
              options={ROLE_OPTIONS}
              selected={[orgRole]}
              onChange={(v) => v[0] && (orgRole = v[0] as GrantRole)}
              searchable={false}
              disabled={!canManage}
              size="sm"
              class="w-28 shrink-0"
            />
          {/if}
        </div>

        {#if vis === 'public' && publicUrl}
          <button
            type="button"
            onclick={() => {
              void navigator.clipboard?.writeText(publicUrl)
              copied = true
            }}
            class="mt-3 flex w-full items-center gap-2 rounded-md border border-line px-3 py-2 text-left text-xs text-muted transition-colors hover:border-[var(--theme-accent-border)]"
          >
            <Globe size={13} class="shrink-0" />
            <code class="min-w-0 flex-1 truncate font-mono text-fg">{publicUrl}</code>
            {#if copied}<Check size={13} class="shrink-0 text-accent" />{:else}<Copy size={13} class="shrink-0" />{/if}
          </button>
        {/if}
        <p class="mt-2 text-[11px] text-muted">Agents only edit when given the Editor role here, never by default.</p>
        {#if inheritable && canManage}
          <button type="button" onclick={() => (inh = true)} class="mt-2 text-[11px] text-accent hover:underline">
            Reset to folder defaults
          </button>
        {/if}
      </div>
    {/if}
    {#if !canManage}<p class="text-[11px] text-muted">Only the owner can change sharing.</p>{/if}
  </div>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="outline" size="sm" onclick={onClose}>
        {canManage ? 'Cancel' : 'Close'}
      </Button>
      {#if canManage}
        <Button
          size="sm"
          onclick={() => void save()}
          disabled={saving || !canSave}
          title={canSave ? undefined : 'Waiting for the current access list; saving now could overwrite it.'}
        >
          {saving ? 'Saving' : 'Save'}
        </Button>
      {/if}
    </div>
  {/snippet}
</Modal>
