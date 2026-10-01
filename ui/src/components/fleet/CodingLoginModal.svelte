<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { delJson, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import type { CodingLogin, CodingService } from './coding-accounts'
  import { flowHint } from './coding-accounts'

  // One sign-in, start to finish.
  //
  // Every provider's flow reduces to the same three things in some order: a
  // URL to open, a question to answer, and a credential at the end. So this is
  // one state machine rather than one screen per provider — the bridge tells
  // us which of the three it is waiting on, and we render that.
  //
  // WHY A PASTE BOX APPEARS FOR SOME. Most authorization-code providers
  // redirect to `http://localhost:<port>` on whatever machine runs the flow,
  // which is the server. When that is the same machine as the browser (a dev
  // stack) the redirect completes by itself. When it is not, the redirect
  // lands nowhere and the code has to come back by hand — which is the
  // provider's own documented fallback, not a workaround.
  let {
    agentId,
    service,
    onClose,
    onDone,
  }: {
    agentId: string
    service: CodingService
    onClose: () => void
    onDone: () => void | Promise<void>
  } = $props()

  let login = $state<CodingLogin | null>(null)
  let answer = $state('')
  let starting = $state(true)
  let submitting = $state(false)
  let failed = $state<string | null>(null)
  let timer: ReturnType<typeof setTimeout> | null = null

  const stopPolling = () => {
    if (timer) clearTimeout(timer)
    timer = null
  }

  /** Poll until the flow needs something or finishes. */
  const poll = async (id: string) => {
    try {
      const next = await getJson<CodingLogin>(`/api/workbench/coding/login/${id}`)
      login = next
      if (next.phase === 'done') {
        stopPolling()
        pushToast({ title: `Signed in${next.email ? ` as ${next.email}` : ''}`, tone: 'success' })
        await onDone()
        return
      }
      if (next.phase === 'error') {
        stopPolling()
        failed = next.error ?? 'the sign-in failed'
        return
      }
      // A flow waiting on a person is still polled: a loopback callback can
      // land at any moment and finish it without them touching the box.
      timer = setTimeout(() => void poll(id), 1200)
    } catch (e) {
      stopPolling()
      failed = e instanceof Error ? e.message : String(e)
    }
  }

  const start = async () => {
    starting = true
    failed = null
    try {
      const started = await postJson<CodingLogin>(`/api/workbench/coding/login`, {
        agentId,
        provider: service.id,
      })
      login = started
      void poll(started.id)
    } catch (e) {
      failed = e instanceof Error ? e.message : String(e)
    } finally {
      starting = false
    }
  }

  const submit = async () => {
    if (!login) return
    submitting = true
    try {
      login = await putJson<CodingLogin>(`/api/workbench/coding/login/${login.id}`, { value: answer })
      answer = ''
    } catch (e) {
      toastError('That answer was not accepted', e)
    } finally {
      submitting = false
    }
  }

  const cancel = async () => {
    stopPolling()
    if (login) await delJson(`/api/workbench/coding/login/${login.id}`).catch(() => {})
    onClose()
  }

  $effect(() => {
    void start()
    return stopPolling
  })
</script>

<Modal open title={`Sign in to ${service.name}`} onClose={() => void cancel()}>
  <div class="flex flex-col gap-3">
    <p class="text-xs text-muted">{flowHint(service)}</p>

    {#if failed}
      <div class="rounded border border-danger/40 bg-danger/5 px-2.5 py-2">
        <p class="text-xs text-danger">{failed}</p>
        <Button class="mt-2" size="xs" variant="ghost" onclick={() => void start()}>Try again</Button>
      </div>
    {:else if starting}
      <p class="text-xs text-muted">Starting…</p>
    {/if}

    {#if login?.url}
      <div class="rounded border border-line bg-surface-2 px-2.5 py-2">
        <div class="flex items-center gap-2">
          <a
            class="truncate text-xs text-accent underline"
            href={login.url}
            target="_blank"
            rel="noreferrer noopener"
          >
            {login.url}
          </a>
          <CopyButton value={login.url} title="Copy the sign-in link" />
        </div>
        {#if login.instructions}
          <p class="mt-1.5 text-xs text-fg">{login.instructions}</p>
        {/if}
      </div>
    {/if}

    {#if login?.progress && login.phase !== 'input'}
      <p class="text-xs text-muted">{login.progress}</p>
    {/if}

    {#if login?.prompt}
      <label class="flex flex-col gap-1">
        <span class="text-xs text-fg">{login.prompt.message}</span>
        <div class="flex items-center gap-1.5">
          <Input
            bind:value={answer}
            size="sm"
            class="flex-1"
            type={login.prompt.secret ? 'password' : 'text'}
            placeholder={login.prompt.placeholder ?? ''}
            onkeydown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
          />
          <Button
            size="sm"
            disabled={submitting || (answer === '' && !login.prompt.allowEmpty)}
            onclick={() => void submit()}
          >
            {login.prompt.kind === 'code' ? 'Finish' : 'Continue'}
          </Button>
        </div>
      </label>
    {/if}
  </div>

  {#snippet footer()}
    <Button variant="ghost" onclick={() => void cancel()}>Cancel</Button>
  {/snippet}
</Modal>
