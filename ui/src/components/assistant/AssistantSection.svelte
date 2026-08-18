<script lang="ts">
  import { untrack } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import AssistantWizard from './AssistantWizard.svelte'
  import AssistantPanels from './AssistantPanels.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { HANDLE_RE, updateAssistant, useAssistant } from '@/lib/assistant'
  import { slide } from '@/lib/motion'
  import { p } from '@/router'

  // §8 field label: 10px mono uppercase 0.08em ink-dim.
  const fieldLabel = 'mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'

  // Settings › Assistant — the WHOLE of a member's personal agent in one place:
  // identity (name, @handle), personality, which model powers it, on/off, and
  // the working parts (schedules, skills, memory) inline below. Owner-scoped
  // APIs throughout — no admin role anywhere.
  const qc = useQueryClient()
  const query = useAssistant()
  const assistant = $derived(query.data)
  let wizard = $state(false)
  let personaEditor = $state(false)
  let handle = $state('')
  let power = $state(false)
  let name = $state('')
  let personality = $state('')
  let personaRev = $state(0)
  let busy = $state(false)
  let saved = $state(false)
  let error = $state<string | null>(null)

  $effect(() => {
    const a = query.data
    if (a) {
      name = a.displayName
      handle = a.slug
      personality = a.personality ?? ''
      personaRev = untrack(() => personaRev) + 1 // reseed the editor with the fetched value
    }
  })

  const handleDirty = $derived(!!assistant && handle.trim() !== assistant.slug && HANDLE_RE.test(handle.trim()))
  const dirty = $derived(
    !!assistant &&
      (name.trim() !== assistant.displayName || handleDirty || personality.trim() !== (assistant.personality ?? '')),
  )

  const save = async () => {
    const a = query.data
    if (!a || !dirty || !name.trim()) return
    busy = true
    error = null
    try {
      const r = await updateAssistant({
        ...(name.trim() !== a.displayName ? { name: name.trim() } : {}),
        ...(handleDirty ? { handle: handle.trim() } : {}),
        ...(personality.trim() !== (a.personality ?? '') ? { personality: personality.trim() } : {}),
      })
      if (!r.assistant) {
        error = r.error ?? 'could not save'
        return
      }
      await qc.invalidateQueries({ queryKey: ['my-assistant'] })
      saved = true
      setTimeout(() => (saved = false), 1500)
    } finally {
      busy = false
    }
  }

  const togglePower = async (a: { id: string; running: boolean }) => {
    power = true
    try {
      await fetch(`/api/fleet/agents/${a.id}/control`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ action: a.running ? 'stop' : 'up' }),
      })
      await qc.invalidateQueries({ queryKey: ['my-assistant'] })
    } finally {
      power = false
    }
  }
</script>

{#if query.isLoading}
  <!-- The card's shape while we find out whether an assistant exists: header
       (avatar + name/status), the two identity inputs, model chips, the
       personality editor block, and the tab strip below the divider. -->
  <section aria-hidden="true" class="rounded-lg border border-line bg-panel p-6">
    <div class="mb-4 flex items-center gap-3">
      <Skeleton class="h-10 w-10 shrink-0 rounded-full" />
      <div class="min-w-0 flex-1 space-y-2">
        <Skeleton class="h-3 w-32 rounded-full" />
        <Skeleton class="h-2.5 w-48 rounded-full" />
      </div>
      <Skeleton class="h-9 w-16 rounded-lg" />
    </div>
    <div class="mb-4 grid gap-4 sm:grid-cols-2">
      <div class="space-y-2">
        <Skeleton class="h-2.5 w-12 rounded-full" />
        <Skeleton class="h-11 w-full" />
      </div>
      <div class="space-y-2">
        <Skeleton class="h-2.5 w-14 rounded-full" />
        <Skeleton class="h-11 w-full" />
      </div>
    </div>
    <div class="mb-4 flex flex-wrap gap-1.5">
      <Skeleton class="h-6 w-16 rounded-full" />
      <Skeleton class="h-6 w-16 rounded-full" />
      <Skeleton class="h-6 w-16 rounded-full" />
    </div>
    <Skeleton class="mb-6 h-20 w-full rounded-xl" />
    <div class="flex gap-2 border-t border-line pt-5">
      <Skeleton class="h-8 w-24 rounded-lg" />
      <Skeleton class="h-8 w-16 rounded-lg" />
      <Skeleton class="h-8 w-16 rounded-lg" />
    </div>
  </section>
{:else if !assistant}
  <section class="rounded-lg border border-line bg-panel p-6">
    <EmptyState
      title="No assistant yet"
      hint="A personal agent that's just yours: memory, skills, and tools of its own."
    >
      {#snippet icon()}<Sparkles size={24} />{/snippet}
      {#snippet action()}
        <Button size="sm" onclick={() => (wizard = true)}>
          Set up your assistant
        </Button>
      {/snippet}
    </EmptyState>
    {#if wizard}<AssistantWizard onClose={() => (wizard = false)} />{/if}
  </section>
{:else}
  <section class="rounded-lg border border-line bg-panel p-6">
    <div class="mb-4 flex items-center gap-3">
      <Avatar name={assistant.displayName} class="h-10 w-10" />
      <div class="min-w-0 flex-1">
        <div class="truncate text-sm font-medium text-fg">{assistant.displayName}</div>
        <!-- Identity meta speaks mono (spec §2 metadata voice); 6px status dot
             green=online, hairline-toned=off (spec §8 status dots). -->
        <div class="flex items-center gap-1.5 font-mono text-[11px] text-muted">
          <span class={cn('h-1.5 w-1.5 rounded-full', assistant.running ? 'bg-success' : 'bg-line')}></span>
          @{assistant.slug} · {assistant.running ? 'online' : 'offline'}
          {#if assistant.currentModel}<span class="truncate"> · {assistant.currentModel}</span>{/if}
        </div>
      </div>
      <Button variant="outline" size="sm" disabled={busy} onclick={() => void togglePower(assistant)}>
        {power ? '…' : assistant.running ? 'Stop' : 'Start'}
      </Button>
      <a href={p('/chat')} class="text-xs text-accent hover:underline">
        Open chat →
      </a>
    </div>

    <div class="mb-4 grid gap-4 sm:grid-cols-2">
      <div>
        <label class={fieldLabel}>Name</label>
        <Input bind:value={name} maxlength={60} />
      </div>
      <div>
        <label class={fieldLabel}>Handle</label>
        <div class="flex items-center gap-2">
          <span class="text-sm text-muted">@</span>
          <Input
            bind:value={handle}
            oninput={() => (handle = handle.toLowerCase())}
            maxlength={30}
            title="Chats, memory, and access move with it; mentions pick up the new handle"
          />
        </div>
        {#if handle !== '' && !HANDLE_RE.test(handle)}
          <p transition:slide={{ duration: 150 }} class="mt-1 text-xs text-danger">2–30 lowercase letters/numbers, starting with a letter.</p>
        {/if}
      </div>
    </div>

    {#if assistant.tiers.length > 0}
      <div class="mb-4">
        <label class={fieldLabel}>Model</label>
        <!-- Tier chips per §7/§8 chip anatomy: radius 6, mono 10px uppercase;
             the active tier reads gold, inactive hairline+muted → hover. -->
        <div class="flex flex-wrap gap-1.5">
          {#each assistant.tiers as t (t.name)}
            <button
              type="button"
              title={t.model}
              disabled={busy}
              onclick={() =>
                void updateAssistant({ model: t.name }).then(() => qc.invalidateQueries({ queryKey: ['my-assistant'] }))}
              class={cn(
                'rounded-md border px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
                focusGold,
                t.active ? 'border-accent text-accent' : 'border-line text-muted hover:dither-fill hover:text-fg',
              )}
            >
              {t.name}
            </button>
          {/each}
        </div>
      </div>
    {/if}

    <div class="mb-1 flex items-center">
      <label class={cn(fieldLabel, 'mb-0')}>Personality</label>
      <button type="button" class="ml-auto text-xs text-accent hover:underline" onclick={() => (personaEditor = true)}>
        Open editor
      </button>
    </div>
    <!-- Inline rich edit (autosave keeps `personality`/dirty fresh); the
         "Open editor" modal adds muse drafting + version history. Reseeds
         when a modal save lands. -->
    <div class="max-h-64 overflow-y-auto">
      {#key personaRev}
        <RichEditor
          value={personality}
          onSave={(md) => (personality = md)}
          autosave
          minHeight="5rem"
          placeholder="How it should come across: tone, priorities, pet peeves."
        />
      {/key}
    </div>
    {#if personaEditor}
      <InternalEditorModal
        open
        onClose={() => (personaEditor = false)}
        title={`${assistant.displayName} · Personality`}
        subtitle="How your assistant comes across. Every save is versioned and applies right away. Your assistant restarts with it."
        value={assistant.personality ?? ''}
        editable
        saving={busy}
        muse={{ kind: 'personality', context: `${assistant.displayName}, the user's personal AI assistant.` }}
        onSave={async (md) => {
          busy = true
          try {
            const r = await updateAssistant({ personality: md })
            if (!r.assistant) throw new Error(r.error ?? 'could not save')
            personality = md // the invalidation below reseeds the inline editor
            await qc.invalidateQueries({ queryKey: ['my-assistant'] })
          } finally {
            busy = false
          }
        }}
        history={{ kind: 'personality', id: assistant.id }}
      />
    {/if}
    <div class="mt-3 flex items-center gap-3">
      <Button size="sm" onclick={() => void save()} disabled={busy || !dirty || !name.trim()}>
        {busy ? 'Saving' : 'Save'}
      </Button>
      <span class="text-xs text-muted">Changes apply right away. Your assistant restarts with them.</span>
    </div>
    {#if saved}<div transition:slide={{ duration: 150 }} class="mt-2 text-xs text-success">Saved</div>{/if}
    {#if error}<div transition:slide={{ duration: 150 }} class="mt-2 text-xs text-danger">{error}</div>{/if}
    <div class="mt-6 border-t border-line pt-5">
      <AssistantPanels {assistant} />
    </div>
  </section>
{/if}
