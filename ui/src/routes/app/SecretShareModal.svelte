<script lang="ts">
  // WHO CAN SEE THIS, AND WHO CAN SPEND IT — the sharing dialog for a working
  // secret.
  //
  // IT LOOKS LIKE THE FILE ONE ON PURPOSE. People already know that dialog:
  // pick someone, they appear in a list, remove them to take it back. Inventing
  // a different gesture for the one surface where getting sharing wrong actually
  // costs something would be a strange place to be original.
  //
  // BUT IT SAYS DIFFERENT THINGS, because the powers are different:
  //
  //   A PERSON gets READ. They can reveal the value, copy it, paste it into
  //     their own .env — and every look they take is recorded under their name.
  //   AN AGENT gets USE. It receives the handle, can pass it to any tool call,
  //     and can never see the value. Not withheld as a policy: there is no code
  //     path that would hand it over, and `secret-vault.ts` would seal it back
  //     into a placeholder if one tried.
  //
  // A row that showed both as "has access" would be lying about what the click
  // just did, so the two are separate lists with their own words.
  //
  // AND THERE IS NO GENERAL-ACCESS TIER. The file dialog offers Restricted /
  // Organization / Anyone with the link; this offers nothing of the sort. A
  // credential everyone in the workspace can read is a credential in a Slack
  // channel with extra steps, and a PUBLIC one is an incident. Explicit names
  // only — which is why the footer says so rather than leaving it as an absence.
  //
  // WRITES ARE INCREMENTAL, one call per change, unlike the file dialog's
  // wholesale PUT. That dialog carries a long comment about a failed read
  // becoming an empty list and Save destroying real grants; per-change writes
  // cannot have that bug, and for a credential the safer shape is worth the
  // extra requests.
  import { createRawSnippet, mount, unmount, type Component, type Snippet } from 'svelte'
  import { Bot, Eye, KeyRound, X } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import type { ComboOption } from '@/components/ui/combobox'
  import { listQuery } from '@/components/ui/query-state'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useUsers } from '@/lib/users'
  import { useAgents } from '@/lib/agents'

  import { listStagger } from '@/lib/motion'

  let {
    title,
    readers,
    grants,
    ownerUserId,
    handle,
    onShare,
    onGrant,
    onClose,
    canManage,
  }: {
    title: string
    readers: string[]
    grants: string[]
    ownerUserId: string | null
    /** Shown beside a granted agent so somebody can copy the string an agent
     *  would write. Absent for a FOLDER, where there is no single handle. */
    handle?: string
    onShare: (userId: string, on: boolean) => Promise<{ error?: string }>
    onGrant: (agentModel: string, on: boolean) => Promise<{ error?: string }>
    onClose: () => void
    /** Only the owner may change who has access. A reader was let in to USE the
     *  credential; letting them widen the circle turns sharing into forwarding,
     *  and the person who put the key in stops knowing who has it. */
    canManage: boolean
  } = $props()

  const usersList = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const agentsQuery = useAgents()

  let busy = $state(false)
  const disabled = $derived(busy)
  let msg = $state<string | null>(null)

  const userName = (id: string) => {
    const u = usersList.rows.find((x) => x.id === id)
    return u?.name ?? u?.email ?? id
  }
  const agentLabel = (model: string) => (agentsQuery.data?.agents ?? []).find((a) => a.id === model)?.label ?? model

  // Same trick PermissionsModal uses: ComboOption.icon is a zero-arg Snippet,
  // but these options are built in script from directory data.
  const componentIcon = (component: Component<any>, props: Record<string, unknown>, wrapperClass = 'contents'): Snippet =>
    createRawSnippet(() => ({
      render: () => `<span class="${wrapperClass}"></span>`,
      setup(el) {
        const instance = mount(component, { target: el, props })
        return () => unmount(instance)
      },
    }))

  // ONE PICKER, BOTH AUDIENCES — because "who should have this" is one thought,
  // and making somebody pick the right of two menus first is a question about
  // our data model rather than about their intent. What each choice MEANS is
  // then spelled out on the row it creates.
  const addOptions = $derived.by((): ComboOption[] => {
    const people = new Set(readers)
    const agents = new Set(grants)
    return [
      ...usersList.rows
        .filter((u) => !people.has(u.id) && u.id !== ownerUserId)
        .map((u) => ({
          value: `user:${u.id}`,
          label: u.name ?? u.email ?? u.id,
          sub: 'Person · can reveal it',
          icon: componentIcon(Avatar, { name: u.name ?? u.email, class: 'h-5 w-5 text-[9px]' }),
        })),
      ...(agentsQuery.data?.agents ?? [])
        .filter((a) => !agents.has(a.id))
        .map((a) => ({
          value: `agent:${a.id}`,
          label: a.label,
          sub: 'Agent · can use it, never sees it',
          icon: componentIcon(Bot, { size: 12 }, 'grid h-5 w-5 place-items-center rounded-full bg-card2 text-muted'),
        })),
    ]
  })

  const run = async (fn: () => Promise<{ error?: string }>) => {
    busy = true
    msg = null
    const r = await fn()
    busy = false
    if (r.error) msg = r.error
  }

  const add = (val: string) => {
    const i = val.indexOf(':')
    const [type, id] = [val.slice(0, i), val.slice(i + 1)]
    if (type === 'user') void run(() => onShare(id, true))
    else if (type === 'agent') void run(() => onGrant(id, true))
  }
</script>

<Modal open={true} {onClose} title="Share “{title}”" width="max-w-lg">
  {#if canManage}
    <Combobox options={addOptions} selected={[]} placeholder="Add a person or an agent" onChange={(v) => add(v[0] ?? '')} {disabled} />
  {/if}

  <!-- THE DIRECTORY READ, REPORTED. Without this a failed /api/users renders
       every grant as a bare uuid and the picker as empty — which reads as "this
       is shared with nobody" on the one surface where believing that is
       expensive. Same fix, same reason, as PermissionsModal. -->
  {#if usersList.notice}<QueryError {...usersList.notice} />{/if}

  <!-- PEOPLE — the ones who can actually read the value. -->
  <p class="mt-4 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Can reveal it</p>
  <ul class="mt-1 space-y-1" use:listStagger>
    <li class="flex items-center gap-2 rounded-md px-2 py-1.5">
      <Avatar name={userName(ownerUserId ?? '')} class="h-6 w-6 text-[10px]" />
      <span class="font-sans text-[13px] text-fg">{userName(ownerUserId ?? '')}</span>
      <span class="ml-auto font-mono text-[10px] text-ink-dim">owner</span>
    </li>
    {#each readers as r (r)}
      <li class="flex items-center gap-2 rounded-md px-2 py-1.5">
        <Avatar name={userName(r)} class="h-6 w-6 text-[10px]" />
        <span class="font-sans text-[13px] text-fg">{userName(r)}</span>
        <Eye size={12} class="text-muted" aria-hidden="true" />
        <span class="font-sans text-xs text-muted">every look is recorded</span>
        {#if canManage}
          <button
            type="button"
            title="Remove access"
            disabled={busy}
            onclick={() => void run(() => onShare(r, false))}
            class="ml-auto text-muted hover:text-fg"
          >
            <X size={13} aria-hidden="true" />
          </button>
        {/if}
      </li>
    {/each}
    {#if readers.length === 0}
      <li class="px-2 font-sans text-xs text-muted">Nobody else — only you.</li>
    {/if}
  </ul>

  <!-- AGENTS — a different power, so a different list and different words. -->
  <p class="mt-4 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Can use it, without ever seeing it</p>
  <ul class="mt-1 space-y-1" use:listStagger>
    {#each grants as g (g)}
      <li class="flex items-center gap-2 rounded-md px-2 py-1.5">
        <span class="grid h-6 w-6 place-items-center rounded-full bg-card2 text-muted"><Bot size={12} aria-hidden="true" /></span>
        <span class="font-sans text-[13px] text-fg">{agentLabel(g)}</span>
        {#if handle}
          <KeyRound size={12} class="text-muted" aria-hidden="true" />
          <code class="font-mono text-[10px] text-accent">{handle}</code>
        {:else}
          <span class="font-sans text-xs text-muted">everything in this folder</span>
        {/if}
        {#if canManage}
          <button
            type="button"
            title="Revoke"
            disabled={busy}
            onclick={() => void run(() => onGrant(g, false))}
            class="ml-auto text-muted hover:text-fg"
          >
            <X size={13} aria-hidden="true" />
          </button>
        {/if}
      </li>
    {/each}
    {#if grants.length === 0}
      <li class="px-2 font-sans text-xs text-muted">No agents. Add one and it can spend this credential without the value entering its context.</li>
    {/if}
  </ul>

  {#if msg}<p class="mt-3 font-sans text-xs text-danger">{msg}</p>{/if}

  {#snippet footer()}
    <!-- SAID OUT LOUD, because it is the one way this dialog differs from the
         file one and an absence is not a statement. -->
    <p class="font-sans text-xs text-muted">
      Named people and agents only — a secret is never workspace-wide and never public.
      {#if !canManage}<span class="text-warning"> You can use this one, but only its owner can change who else has it.</span>{/if}
    </p>
  {/snippet}
</Modal>
