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
  <div class="flex flex-col gap-4">
    <p class="font-sans text-[13px] leading-5 text-muted">{flowHint(service)}</p>

    {#if failed}
      <div role="alert">
        <p class="font-sans text-[13px] leading-5 text-danger">{failed}</p>
        <Button class="mt-2" size="sm" variant="outline" onclick={() => void start()}>Try again</Button>
      </div>
    {:else if starting}
      <p class="font-sans text-[13px] leading-5 text-muted">Starting</p>
    {/if}

    {#if login?.url}
      <div class="rounded-md border border-line bg-raised px-3 py-2.5">
        <div class="flex items-center gap-2">
          <a
            class="min-w-0 flex-1 truncate font-mono text-xs text-accent underline"
            href={login.url}
            target="_blank"
            rel="noreferrer noopener"
          >
            {login.url}
          </a>
          <CopyButton value={login.url} title="Copy the sign-in link" />
        </div>
        {#if login.instructions}
          <p class="mt-2 font-sans text-[13px] leading-5 text-fg">{login.instructions}</p>
        {/if}
      </div>
    {/if}

    {#if login?.progress && login.phase !== 'input'}
      <p class="font-sans text-[13px] leading-5 text-muted">{login.progress}</p>
    {/if}

    {#if login?.prompt}
      <label class="flex flex-col gap-1.5">
        <span class="font-sans text-[13px] font-medium leading-5 text-fg">{login.prompt.message}</span>
        <div class="flex items-center gap-2">
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
            size="md"
            variant="outline"
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
    <div class="flex justify-end"><Button variant="ghost" size="sm" onclick={() => void cancel()}>Cancel</Button></div>
  {/snippet}
</Modal>
