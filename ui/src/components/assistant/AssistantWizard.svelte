<script lang="ts" module>
  // Onboarding for the personal assistant: Name (+handle) → Personality → Launch.
  // Everything is optional-with-good-defaults so the flow never blocks on ideas —
  // but every choice that matters (name, handle, voice) is theirs to make.
  const STEPS = ['Name', 'Personality', 'Launch'] as const

  const PRESETS = [
    {
      label: 'Warm and proactive',
      text: 'Be warm and personable. Anticipate what I need, suggest next steps, and check in when something is ambiguous rather than guessing.',
    },
    {
      label: 'Concise and professional',
      text: 'Be brief and businesslike. Lead with the answer, skip pleasantries, and use short bullet points for any detail.',
    },
    {
      label: 'Playful and curious',
      text: 'Keep it light. A little wit is welcome. Be curious, offer ideas I did not ask for when they are good, but stay useful first.',
    },
  ] as const
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Steps from '@/components/ui/Steps.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { slide, staggerIn } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { HANDLE_RE, createAssistant, suggestHandle, type Assistant } from '@/lib/assistant'
  import { navigate } from '@/router'

  let { onClose }: { onClose: () => void } = $props()

  const qc = useQueryClient()
  const session = useSession()
  const first = session.data?.name?.split(' ')[0] ?? session.data?.email?.split('@')[0] ?? 'My'

  let step = $state(0)
  let name = $state(`${first}'s assistant`)
  let handle = $state(suggestHandle(`${first}'s assistant`))
  let handleTouched = $state(false)
  let personality = $state('')
  let personaRev = $state(0)
  let busy = $state(false)
  let error = $state<string | null>(null)
  let created = $state<Assistant | null>(null)

  const handleOk = $derived(HANDLE_RE.test(handle))
  const nameOk = $derived(name.trim().length > 0)

  // Runs after bind:value has updated `name` on each keystroke.
  const rename = () => {
    if (!handleTouched) handle = suggestHandle(name)
  }

  const launch = async () => {
    busy = true
    error = null
    try {
      const r = await createAssistant({ name: name.trim(), handle, personality: personality.trim() || undefined })
      if (!r.assistant) {
        error = r.error ?? 'Could not create your assistant'
        return
      }
      created = r.assistant
      await qc.invalidateQueries({ queryKey: ['my-assistant'] })
      await qc.invalidateQueries({ queryKey: ['agents'] })
    } finally {
      busy = false
    }
  }
</script>

<Modal open {onClose} title="Set up your assistant" width="max-w-lg">
  {#snippet footer()}
    {#if created}
      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Done
        </Button>
        <Button
          size="sm"
          onclick={() => {
            onClose()
            void navigate('/chat')
          }}
        >
          Say hello
        </Button>
      </div>
    {:else if step === 0}
      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onclick={() => (step = 1)} disabled={!nameOk || !handleOk}>
          Continue
        </Button>
      </div>
    {:else if step === 1}
      <div class="flex justify-between gap-2">
        <Button variant="ghost" size="sm" onclick={() => (step = 0)}>
          Back
        </Button>
        <Button size="sm" onclick={() => (step = 2)}>
          Continue
        </Button>
      </div>
    {:else}
      <div class="flex justify-between gap-2">
        <Button variant="ghost" size="sm" onclick={() => (step = 1)} disabled={busy}>
          Back
        </Button>
        <Button size="sm" onclick={() => void launch()} disabled={busy}>
          {#if busy}<WaitingMark site="assistant/wizard" size={12} />{/if}
          {busy ? 'Creating' : 'Create assistant'}
        </Button>
      </div>
    {/if}
  {/snippet}
  <!-- Step-to-step motion (ANIMATIONS.md wizard row): the panel glides between
       step heights via AutoHeight, and each step's sections stagger in inside
       {#key}. No exit on the outgoing step — the incoming stagger + gliding
       height IS the transition. -->
  <AutoHeight>
    <div class="space-y-5">
      {#if !created}<Steps steps={STEPS} current={step} />{/if}

      {#key created ? 'done' : step}
        {#if created}
          <div use:staggerIn class="space-y-2 py-2 text-center">
            <span class="mx-auto grid h-11 w-11 place-items-center rounded-lg bg-accent-soft text-accent">
              <Sparkles size={20} />
            </span>
            <div class="font-sans text-[15px] font-semibold leading-5 text-fg">{created.displayName} is ready</div>
            <p class="font-sans text-[13px] leading-5 text-muted">
              Its workspace is starting up. It has its own memory and skills, and it only works for you.
            </p>
          </div>
        {:else if step === 0}
          <div use:staggerIn class="space-y-5">
            <p class="font-sans text-sm leading-5 text-muted">An agent that is just yours, with its own memory, skills and tools.</p>
            <SettingsField label="Name" for="wizard-name">
              <Input id="wizard-name" size="sm" bind:value={name} oninput={rename} placeholder="Maxie" autofocus maxlength={60} />
            </SettingsField>
            <SettingsField label="Handle" for="wizard-handle">
              <div class="flex items-center gap-2">
                <span class="font-sans text-sm text-muted">@</span>
                <Input
                  id="wizard-handle"
                  size="sm"
                  class="font-mono"
                  bind:value={handle}
                  oninput={() => {
                    handleTouched = true
                    handle = handle.toLowerCase()
                  }}
                  placeholder="maxie"
                  maxlength={30}
                />
              </div>
              {#if handle && !handleOk}
                <p class="font-sans text-[13px] leading-5 text-danger" role="alert">2 to 30 lowercase letters or numbers, starting with a letter.</p>
              {:else}
                <p class="font-sans text-[13px] leading-5 text-muted">How people and agents mention it.</p>
              {/if}
            </SettingsField>
          </div>
        {:else if step === 1}
          <div use:staggerIn class="space-y-4">
            <p class="font-sans text-sm leading-5 text-muted">
              How should {name.trim() || 'it'} come across? Pick a starting point or write your own.
            </p>
            <div class="flex flex-wrap gap-2">
              {#each PRESETS as preset (preset.label)}
                <Chip
                  selected={personality === preset.text}
                  onSelect={() => {
                    personality = preset.text
                    personaRev += 1 // reseed the editor with the preset
                  }}
                >
                  {preset.label}
                </Chip>
              {/each}
            </div>
            <div class="max-h-60 overflow-y-auto">
              {#key personaRev}
                <RichEditor
                  value={personality}
                  onSave={(md) => (personality = md)}
                  autosave
                  minHeight="5rem"
                  placeholder="Optional. For example: be direct, keep things short, remind me about loose ends."
                />
              {/key}
            </div>
          </div>
        {:else}
          <div use:staggerIn class="space-y-4">
            <p class="font-sans text-sm leading-5 text-muted">Creating it starts a private workspace. This can take a minute.</p>
            <dl class="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-4 gap-y-2 font-sans text-[13px] leading-5">
              <dt class="text-muted">Name</dt>
              <dd class="text-fg">{name.trim()}</dd>
              <dt class="text-muted">Handle</dt>
              <dd class="font-mono text-xs leading-5 text-fg">@{handle}</dd>
              <dt class="text-muted">Personality</dt>
              <dd class="min-w-0 text-fg">
                {#if personality.trim()}{personality.trim()}{:else}<span class="text-muted">Warm, direct and useful (the default)</span>{/if}
              </dd>
            </dl>
            {#if error}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</p>{/if}
          </div>
        {/if}
      {/key}
    </div>
  </AutoHeight>
</Modal>
