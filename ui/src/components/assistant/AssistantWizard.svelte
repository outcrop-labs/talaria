<script lang="ts" module>
  // Onboarding for the personal assistant: Name (+handle) → Personality → Launch.
  // Everything is optional-with-good-defaults so the flow never blocks on ideas —
  // but every choice that matters (name, handle, voice) is theirs to make.
  const STEPS = ['Name', 'Personality', 'Launch'] as const

  // §8 field label: 10px mono uppercase 0.08em ink-dim.
  const fieldLabel = 'mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'

  const PRESETS = [
    {
      label: 'Warm & proactive',
      text: 'Be warm and personable. Anticipate what I need, suggest next steps, and check in when something is ambiguous rather than guessing.',
    },
    {
      label: 'Concise & professional',
      text: 'Be brief and businesslike. Lead with the answer, skip pleasantries, and use short bullet points for any detail.',
    },
    {
      label: 'Playful & curious',
      text: 'Keep it light. A little wit is welcome. Be curious, offer ideas I did not ask for when they are good, but stay useful first.',
    },
  ] as const
</script>

<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Steps from '@/components/ui/Steps.svelte'
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { fade, slide, QUICK } from '@/lib/motion'
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
        error = r.error ?? 'could not create your assistant'
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
          {#if busy}<GeneratingBars bars={3} variant="weave" step={0.15} />{/if}
          {busy ? 'Creating' : 'Create assistant'}
        </Button>
      </div>
    {/if}
  {/snippet}
  <div class="space-y-5">
    {#if !created}<Steps steps={STEPS} current={step} />{/if}

    {#if created}
      <div in:fade={{ duration: 150, delay: 80 }} out:fade={QUICK} class="space-y-2 py-2 text-center">
        <span class="mx-auto grid h-11 w-11 place-items-center rounded-lg bg-accent-soft text-accent">
          <Sparkles size={20} />
        </span>
        <div class="text-sm font-medium text-fg">{created.displayName} is ready</div>
        <p class="text-xs text-muted">
          Its workspace is starting up. It has its own memory, skills, and private knowledge, and it only works
          for you. Tune it any time in Settings.
        </p>
      </div>
    {:else if step === 0}
      <div in:fade={{ duration: 150, delay: 80 }} out:fade={QUICK} class="space-y-4">
        <p class="text-sm text-muted">
          Your assistant is a real agent that's just yours: its own memory, skills, and tools. Start by naming it.
        </p>
        <div>
          <label class={fieldLabel}>Name</label>
          <Input bind:value={name} oninput={rename} placeholder="Maxie" autofocus maxlength={60} />
        </div>
        <div>
          <label class={fieldLabel}>Handle</label>
          <div class="flex items-center gap-2">
            <span class="text-sm text-muted">@</span>
            <Input
              bind:value={handle}
              oninput={() => {
                handleTouched = true
                handle = handle.toLowerCase()
              }}
              placeholder="maxie"
              maxlength={30}
            />
          </div>
          <p class={cn('mt-1 text-xs', handle && !handleOk ? 'text-danger' : 'text-muted')}>
            {handle && !handleOk
              ? 'Lowercase letters and numbers only, starting with a letter (2–30 characters).'
              : "How agents and integrations refer to it. Can't be changed later."}
          </p>
        </div>
      </div>
    {:else if step === 1}
      <div in:fade={{ duration: 150, delay: 80 }} out:fade={QUICK} class="space-y-3">
        <p class="text-sm text-muted">
          How should {name.trim() || 'it'} come across? Pick a starting point or write your own. You can refine it
          any time.
        </p>
        <div class="flex flex-wrap gap-2">
          {#each PRESETS as preset (preset.label)}
            <button
              type="button"
              onclick={() => {
                personality = preset.text
                personaRev += 1 // reseed the editor with the preset
              }}
              class={cn(
                'rounded-md border px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
                focusGold,
                personality === preset.text ? 'border-accent text-accent' : 'border-line text-muted hover:bg-hover hover:text-fg',
              )}
            >
              {preset.label}
            </button>
          {/each}
        </div>
        <div class="max-h-60 overflow-y-auto">
          {#key personaRev}
            <RichEditor
              value={personality}
              onSave={(md) => (personality = md)}
              autosave
              minHeight="5rem"
              placeholder="Optional, e.g. “Be direct and keep things short. Remind me about loose ends.”"
            />
          {/key}
        </div>
      </div>
    {:else}
      <div in:fade={{ duration: 150, delay: 80 }} out:fade={QUICK} class="space-y-3">
        <p class="text-sm text-muted">Ready to go. Creating it starts a private workspace. This can take a minute.</p>
        <dl class="space-y-2 rounded-lg border border-line bg-panel p-3 text-sm">
          <div class="flex items-baseline gap-2">
            <dt class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</dt>
            <dd class="text-fg">{name.trim()}</dd>
          </div>
          <div class="flex items-baseline gap-2">
            <dt class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Handle</dt>
            <dd class="font-mono text-[13px] text-fg">@{handle}</dd>
          </div>
          <div class="flex items-baseline gap-2">
            <dt class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Personality</dt>
            <dd class="min-w-0 text-fg">
              {#if personality.trim()}{personality.trim()}{:else}<span class="text-muted">Warm, direct, and useful (default)</span>{/if}
            </dd>
          </div>
        </dl>
        {#if error}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</p>{/if}
      </div>
    {/if}
  </div>
</Modal>
