<script lang="ts">
  import { untrack } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSaveBar from '@/components/settings/SettingsSaveBar.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Select from '@/components/ui/Select.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import AssistantWizard from './AssistantWizard.svelte'
  import AssistantPanels from './AssistantPanels.svelte'
  import AssistantGoogleRow from './AssistantGoogleRow.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { HANDLE_RE, updateAssistant, useAssistant, type AssistantDmAccess } from '@/lib/assistant'
  import { useModels } from '@/lib/muse.svelte'
  import { navigate } from '@/router'

  // Settings › Assistant — the WHOLE of a member's personal agent in one place:
  // identity (name, @handle), personality, which model powers it, on/off, and
  // the working parts (schedules, skills, memory) inline below. Owner-scoped
  // APIs throughout — no admin role anywhere.
  //
  // TWO KINDS OF SETTING, and the cards follow them. A name, a handle or a
  // personality is a new version of the agent and restarts it, so those three
  // share one card and one Save. The model and the DM reach apply as they are
  // picked, and each says Saved on its own row: the page has a Save button, so
  // its header makes no claim about saving.
  const qc = useQueryClient()
  const query = useAssistant()
  const assistant = $derived(query.data)
  const autosave = useAutosave()
  let wizard = $state(false)
  let personaEditor = $state(false)
  let handle = $state('')
  let power = $state(false)
  let name = $state('')
  let personality = $state('')
  let personaRev = $state(0)
  let busy = $state(false)
  const savedFlash = useSavedFlash()
  // A refusal is said under the group the person acted in.
  let error = $state<{ at: 'power' | 'identity' | 'access'; text: string } | null>(null)

  // The assistant names its models by id. The catalog has the name a person
  // knows for most of them, under the bare id or an "<endpoint>/<id>" pin.
  const catalog = useModels()
  const modelName = (id: string): string | null =>
    catalog.data?.models.find((m) => m.id === id || m.id.endsWith(`/${id}`))?.label ?? null

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
    if (!r.assistant) error = { at: 'access', text: r.error ?? 'Could not save' }
    else autosave.flash('dm')
    await qc.invalidateQueries({ queryKey: ['my-assistant'] })
  }

  const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)
  // "Fast" and "Deep" are the product's words, so each choice says which model it is.
  const activeTier = $derived(assistant?.tiers.find((t) => t.active)?.name ?? '')
  const setTier = async (tier: string) => {
    if (busy) return
    error = null
    const r = await updateAssistant({ model: tier })
    if (!r.assistant) error = { at: 'access', text: r.error ?? 'Could not save' }
    else autosave.flash('model')
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
        error = { at: 'identity', text: r.error ?? 'Could not save' }
        return
      }
      await qc.invalidateQueries({ queryKey: ['my-assistant'] })
      savedFlash.flash()
    } finally {
      busy = false
    }
  }

  const discard = () => {
    const a = query.data
    if (!a) return
    error = null
    name = a.displayName
    handle = a.slug
    personality = a.personality ?? ''
    personaRev += 1
  }

  const togglePower = async (a: { id: string; running: boolean }) => {
    power = true
    error = null
    try {
      await postJson(`/api/fleet/agents/${a.id}/control`, { action: a.running ? 'stop' : 'up' })
    } catch (e) {
      error = { at: 'power', text: errorMessage(e) }
    }
    await qc.invalidateQueries({ queryKey: ['my-assistant'] })
    power = false
  }
</script>

{#snippet refusal(at: 'power' | 'identity' | 'access')}
  {#if error?.at === at}<p class="mt-2 font-sans text-[13px] leading-5 text-danger" role="alert">{error.text}</p>{/if}
{/snippet}

{#if query.isLoading}
  <!-- The page's shape while we find out whether an assistant exists: who it
       is, then the card of fields. -->
  <div aria-hidden="true" class="space-y-8">
    <div class="flex items-center gap-3">
      <Skeleton class="h-10 w-10 shrink-0 rounded-full" />
      <div class="min-w-0 flex-1 space-y-2">
        <Skeleton class="h-3 w-32 rounded-full" />
        <Skeleton class="h-2.5 w-48 rounded-full" />
      </div>
      <Skeleton class="h-8 w-24 rounded-md" />
    </div>
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
  <div class="space-y-8">
    <!-- Who it is and whether it is running: a line of the page, not a card of one row. -->
    <div>
      <div class="flex flex-wrap items-center gap-x-6 gap-y-3">
        <div class="flex min-w-0 flex-1 basis-48 items-center gap-3">
          <Avatar name={assistant.displayName} class="h-10 w-10 text-sm" />
          <div class="min-w-0 flex-1">
            <h2 class="break-words font-sans text-lg font-semibold leading-6 text-fg">{assistant.displayName}</h2>
            <p class="font-sans text-[13px] leading-5 text-muted">
              {#if assistant.currentModel}
                {@const model = assistant.currentModel}
                <span class="font-mono text-xs">@{assistant.slug}</span>
                · {#if modelName(model)}{modelName(model)}{:else}<span class="font-mono text-xs">{model}</span>{/if}
              {:else}
                <span class="font-mono text-xs">@{assistant.slug}</span>
              {/if}
            </p>
          </div>
        </div>
        <div class="flex shrink-0 items-center gap-3">
          <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
            <StatusDot status={power ? 'warn' : assistant.running ? 'ok' : 'idle'} />
            {power ? (assistant.running ? 'Stopping' : 'Starting') : assistant.running ? 'Running' : 'Stopped'}
          </span>
          {#if !assistant.running}
            <Button size="sm" variant="outline" disabled={busy || power} onclick={() => void togglePower(assistant)}>Start</Button>
          {/if}
          <SettingsRowMenu
            label={`Actions for ${assistant.displayName}`}
            items={[
              { label: 'Open chat', onSelect: () => void navigate('/chat') },
              {
                label: assistant.running ? 'Stop assistant' : 'Start assistant',
                disabled: busy || power,
                onSelect: () => void togglePower(assistant),
              },
            ]}
          />
        </div>
      </div>
      {@render refusal('power')}
    </div>

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
            <Button variant="ghost" size="sm" class="-mr-3 ml-auto" onclick={() => (personaEditor = true)}>Open full editor</Button>
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
        <SettingsSaveBar
          label="Save and restart"
          reason="Saving restarts your assistant."
          {dirty}
          saving={busy}
          saved={savedFlash.saved}
          disabled={!name.trim()}
          onSave={() => void save()}
          onDiscard={discard}
        />
      </SettingsGroup>
      {@render refusal('identity')}
    </SettingsSection>

    <SettingsSection title="Model and access">
      <SettingsGroup>
        {#if assistant.tiers.length > 0}
          <!-- An assistant can run a model that is neither tier. The select names it and offers the two. -->
          <SettingsRow
            label="Model"
            hint={activeTier ? undefined : 'Not one of the two tiers. Pick one to switch.'}
            as="label"
            saved={autosave.saved('model')}
          >
            <Select size="sm" class="w-[280px]" value={activeTier} disabled={busy} onchange={(e) => void setTier(e.currentTarget.value)}>
              {#if !activeTier}
                {@const model = assistant.currentModel}
                <option value="" disabled>{model ? (modelName(model) ?? model) : 'No model set'}</option>
              {/if}
              {#each assistant.tiers as t (t.name)}
                <option value={t.name}>{sentence(t.name)} · {modelName(t.model) ?? t.model}</option>
              {/each}
            </Select>
          </SettingsRow>
        {/if}
        <SettingsRow
          label="Direct messages"
          hint="What it may do in your direct messages with other people."
          info="Channels follow your own membership either way."
          saved={autosave.saved('dm')}
        >
          <Segmented options={DM_ACCESS} value={assistant.dmAccess} onChange={(id) => void setDmAccess(id)} />
        </SettingsRow>
        <AssistantGoogleRow />
      </SettingsGroup>
      {@render refusal('access')}
    </SettingsSection>

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
