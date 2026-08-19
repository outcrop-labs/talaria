<script lang="ts">
  import Button from './Button.svelte'
  import Input from './Input.svelte'
  import Modal from './Modal.svelte'
  import { dialog } from './confirm.svelte'

  // Mount once, at the app root, so the imperative confirm/alert/prompt
  // functions (see confirm.svelte.ts) have somewhere to render.
  let value = $state('')
  let inputRef = $state<HTMLInputElement | null>(null)

  const spec = $derived(dialog.active?.spec)
  const isAlert = $derived(spec?.kind === 'alert')
  // The typed challenge: both prompts and danger confirms with `requireText`
  // gate the accept button on this matching exactly.
  const needsText = $derived(spec?.kind === 'prompt' || !!spec?.requireText)
  const unlocked = $derived(spec?.kind === 'confirm' && spec.requireText !== undefined ? value.trim() === spec.requireText : true)

  $effect(() => {
    if (needsText) {
      value = spec?.kind === 'prompt' ? (spec.defaultValue ?? '') : ''
      const id = requestAnimationFrame(() => inputRef?.select())
      return () => cancelAnimationFrame(id)
    }
  })

  function settle(result: boolean | string | null) {
    dialog.active?.resolve(result)
    dialog.active = null
  }

  const cancel = () => settle(spec?.kind === 'prompt' ? null : false)
  const accept = () => settle(spec?.kind === 'confirm' ? true : spec?.kind === 'prompt' ? value : null)
</script>

<Modal open={!!spec} onClose={cancel} title={spec?.title}>
  {#if spec?.message}
    <div class="whitespace-pre-line text-sm text-muted">{spec.message}</div>
  {/if}
  {#if needsText}
    <Input
      bind:ref={inputRef}
      bind:value
      placeholder={spec?.kind === 'prompt' ? spec.placeholder : spec?.requireText}
      onkeydown={(e) => {
        // Enter accepts only when the challenge is met — a matched name is
        // the second opt-in, and the key must not skip it.
        if (e.key === 'Enter' && unlocked) {
          e.preventDefault()
          accept()
        }
      }}
      class={spec?.message ? 'mt-4' : undefined}
      autofocus
    />
  {/if}
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      {#if !isAlert}
        <Button variant="ghost" size="sm" onclick={cancel}>
          {spec?.cancelLabel ?? 'Cancel'}
        </Button>
      {/if}
      <Button
        variant={spec?.danger ? 'danger-outline' : 'primary'}
        size="sm"
        onclick={accept}
        disabled={spec?.kind === 'confirm' ? !unlocked : false}
      >
        {spec?.confirmLabel ?? (isAlert ? 'OK' : 'Confirm')}
      </Button>
    </div>
  {/snippet}
</Modal>
