<script lang="ts">
  import { untrack } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Input from '@/components/ui/Input.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import AssistantWizard from './AssistantWizard.svelte'
  import AssistantPanels from './AssistantPanels.svelte'
  import AssistantGoogleCard from './AssistantGoogleCard.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { HANDLE_RE, updateAssistant, useAssistant, type AssistantDmAccess } from '@/lib/assistant'
  import { p } from '@/router'

  // Settings › Assistant — the WHOLE of a member's personal agent in one place:
  // identity (name, @handle), personality, which model powers it, on/off, and
  // the working parts (schedules, skills, memory) inline below. Owner-scoped
  // APIs throughout — no admin role anywhere.
  //
  // TWO KINDS OF SETTING, and the cards follow them. A name, a handle or a
  // personality is a new version of the agent and restarts it, so those three
  // share one card and one Save. The model and the DM reach apply as they are
  // picked.
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

  // The owner's DM reach: whether the assistant sees the owner's direct
  // messages with people at all, reads them, or reads and posts in them.
  // Autosave on change (it is a setting, not a deploy): the API applies it
  // without a version or a restart.
  const DM_ACCESS: ReadonlyArray<SegmentedOption<AssistantDmAccess>> = [
    { id: 'none', label: 'None', title: 'Your DMs with people are not listed, read, or posted in' },
    { id: 'read', label: 'Read', title: 'It can read your DMs with people but never post in them' },
    { id: 'read-write', label: 'Read and post', title: 'Your own view: it can read and post in your DMs with people' },
  ]
  const setDmAccess = async (dmAccess: AssistantDmAccess) => {
    error = null
    const r = await updateAssistant({ dmAccess })
    if (!r.assistant) error = r.error ?? 'Could not save'
    await qc.invalidateQueries({ queryKey: ['my-assistant'] })
  }

  const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)
  const tierOptions = $derived(
    (assistant?.tiers ?? []).map((t): SegmentedOption => ({ id: t.name, label: sentence(t.name), title: t.model })),
  )
  const setTier = async (tier: string) => {
    if (busy) return
    await updateAssistant({ model: tier })
    await qc.invalidateQueries({ queryKey: ['my-assistant'] })
  }

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
        error = r.error ?? 'Could not save'
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
    error = null
    try {
      await postJson(`/api/fleet/agents/${a.id}/control`, { action: a.running ? 'stop' : 'up' })
    } catch (e) {
      error = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['my-assistant'] })
    power = false
  }
</script>

{#if query.isLoading}
  <!-- The page's shape while we find out whether an assistant exists: the
       identity row, then the card of fields. -->
  <div aria-hidden="true" class="space-y-10">
    <SettingsGroup>
      <div class="flex min-h-[60px] items-center gap-3 px-5 py-4">
        <Skeleton class="h-10 w-10 shrink-0 rounded-full" />
        <div class="min-w-0 flex-1 space-y-2">
          <Skeleton class="h-3 w-32 rounded-full" />
          <Skeleton class="h-2.5 w-48 rounded-full" />
        </div>
        <Skeleton class="h-8 w-24 rounded-md" />
      </div>
    </SettingsGroup>
    <SettingsGroup>
      <SettingsRow label="Name"><Skeleton class="h-9 w-[280px]" /></SettingsRow>
      <SettingsRow label="Handle"><Skeleton class="h-9 w-[280px]" /></SettingsRow>
      <SettingsRow label="Personality" stacked><Skeleton class="h-24 w-full rounded-md" /></SettingsRow>
    </SettingsGroup>
  </div>
{:else if !assistant}
  <EmptyState
    title="No assistant yet"
    hint="A personal agent that is just yours, with memory, skills and tools of its own."
  >
    {#snippet icon()}<Sparkles size={24} />{/snippet}
    {#snippet action()}
      <Button size="sm" onclick={() => (wizard = true)}>
        Set up your assistant
      </Button>
    {/snippet}
  </EmptyState>
  {#if wizard}<AssistantWizard onClose={() => (wizard = false)} />{/if}
{:else}
  <div class="space-y-10">
    <SettingsGroup>
      <SettingsRow label={assistant.displayName}>
        {#snippet lead()}<Avatar name={assistant.displayName} class="h-10 w-10" />{/snippet}
        {#snippet hint()}
          <span class="font-mono text-xs">@{assistant.slug}</span>
          {#if assistant.currentModel}· <span class="font-mono text-xs">{assistant.currentModel}</span>{/if}
        {/snippet}
        {#snippet status()}
          <StatusDot status={assistant.running ? 'ok' : 'idle'} />
          {power ? (assistant.running ? 'Stopping' : 'Starting') : assistant.running ? 'Running' : 'Stopped'}
        {/snippet}
        <a href={p('/chat')} class={buttonClasses({ size: 'sm', variant: 'outline', voice: 'sans' })}>Open chat</a>
        <SettingsRowMenu
          label={`Actions for ${assistant.displayName}`}
          items={[
            {
              label: assistant.running ? 'Stop assistant' : 'Start assistant',
              disabled: busy || power,
              onSelect: () => void togglePower(assistant),
            },
          ]}
        />
      </SettingsRow>
    </SettingsGroup>

    <SettingsSection title="Identity">
      <SettingsGroup>
        <SettingsRow label="Name" as="label">
          <Input id="assistant-name" size="sm" class="w-[280px]" bind:value={name} maxlength={60} />
        </SettingsRow>
        <SettingsRow label="Handle" as="label">
          {#snippet hint()}
            {#if handle !== '' && !HANDLE_RE.test(handle)}
              <span class="text-danger" role="alert">2 to 30 lowercase letters or numbers, starting with a letter.</span>
            {:else}
              Chats, memory and mentions move with it.
            {/if}
          {/snippet}
          <span class="font-sans text-sm text-muted">@</span>
          <Input
            id="assistant-handle"
            size="sm"
            class="w-[280px] font-mono"
            bind:value={handle}
            oninput={() => (handle = handle.toLowerCase())}
            maxlength={30}
          />
        </SettingsRow>
        <SettingsRow label="Personality" hint="How it should come across: tone, priorities, pet peeves." stacked>
          {#snippet meta()}
            <!-- The full editor adds Muse drafting and version history. -->
            <Button variant="link" size="sm" class="ml-auto text-[13px]" onclick={() => (personaEditor = true)}>Open full editor</Button>
          {/snippet}
          <!-- Inline rich edit (autosave keeps `personality`/dirty fresh).
               Reseeds when a save from the full editor lands. -->
          <div class="max-h-64 w-full overflow-y-auto">
            {#key personaRev}
              <RichEditor
                value={personality}
                onSave={(md) => (personality = md)}
                autosave
                minHeight="5rem"
                placeholder="Brief and direct. Lead with what needs a decision."
              />
            {/key}
          </div>
        </SettingsRow>
        <!-- An explicit Save, not save-on-blur: each save is a new version of
             the agent and restarts it. -->
        <div class="flex min-h-[60px] items-center gap-3 px-5 py-3">
          <span class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">
            {#if error}
              <span class="text-danger" role="alert">{error}</span>
            {:else}
              Saving restarts your assistant.
            {/if}
          </span>
          {#if saved}<span class="font-sans text-[13px] leading-5 text-success">Saved</span>{/if}
          <Button size="sm" onclick={() => void save()} disabled={busy || !dirty || !name.trim()}>
            {busy ? 'Saving' : 'Save'}
          </Button>
        </div>
      </SettingsGroup>
    </SettingsSection>

    <SettingsSection title="Behavior">
      <SettingsGroup>
        {#if assistant.tiers.length > 0}
          <SettingsRow label="Model" hint="Pick the model it starts every chat on.">
            <Segmented
              options={tierOptions}
              value={assistant.tiers.find((t) => t.active)?.name ?? ''}
              onChange={(tier) => void setTier(tier)}
            />
          </SettingsRow>
        {/if}
        <SettingsRow
          label="Direct messages"
          hint="What it may do in your direct messages with other people."
          info="Channels follow your own membership either way."
        >
          <Segmented options={DM_ACCESS} value={assistant.dmAccess} onChange={(id) => void setDmAccess(id)} />
        </SettingsRow>
      </SettingsGroup>
    </SettingsSection>

    <!-- The assistant's outside reach. Above the working parts because it is
         the grant most owners are here to make. -->
    <AssistantGoogleCard />
    <AssistantPanels {assistant} />
  </div>

  {#if personaEditor}
    <InternalEditorModal
      open
      onClose={() => (personaEditor = false)}
      title={`${assistant.displayName} · Personality`}
      subtitle="How your assistant comes across. Every save is versioned and restarts your assistant."
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
{/if}
