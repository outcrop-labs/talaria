<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import type { AskOption } from '@/lib/chips'

  // THE AGENT ASKED SOMETHING. Full width, like an approval, because it is a
  // thing to answer rather than a thing to read past — and because the one
  // failure mode worth designing against is a person scrolling by a question
  // the agent is waiting on.
  //
  // CLICKING SENDS THE OPTION'S OWN WORDS through the composer's send path,
  // the same `onInvoke` the tool chips use. So the answer is an ordinary
  // message: it appears in the transcript as the person saying it, it
  // triggers the next turn exactly as typing would, and there is no second
  // record of the answer anywhere to disagree with it.
  //
  // THE COMPOSER IS STILL THERE, which is why there is no "something else"
  // button: a person who wants to answer in their own words just types, and a
  // button that only focuses the composer would be furniture.
  let {
    question,
    detail,
    options,
    onAnswer,
  }: {
    question: string
    detail?: string
    options: AskOption[]
    onAnswer?: (text: string) => void
  } = $props()

  // Answered once. The chip stays on the turn it landed on (the transcript is
  // a record), but the buttons stop being live the moment one is used — a
  // second click would send a second answer to a question that already has
  // one, and the agent would be reading two.
  let sent = $state<string | null>(null)
</script>

{#if options.length > 0}
  <div class="w-full rounded-lg border border-line bg-surface px-3 py-2" role="group" aria-label={question}>
    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
      {sent ? 'Answered' : 'Waiting on you'}
    </div>
    <p class="mt-1 font-sans text-[13px] text-fg">{question}</p>
    {#if detail}
      <p class="mt-0.5 font-sans text-xs leading-relaxed text-muted">{detail}</p>
    {/if}
    {#if sent}
      <p class="mt-1.5 font-sans text-xs text-muted">You answered: {sent}</p>
    {:else}
      <div class="mt-2 flex flex-wrap gap-1.5">
        {#each options as option (option.label)}
          <!-- The detail rides as the title rather than as a second line: six
               options with a paragraph each is not a row of buttons any more. -->
          <span class="inline-flex" title={option.detail || undefined}>
            <Button
              size="xs"
              variant="outline"
              disabled={!onAnswer}
              onclick={() => {
                sent = option.label
                onAnswer?.(option.label)
              }}
            >
              {option.label}
            </Button>
          </span>
        {/each}
      </div>
    {/if}
  </div>
{/if}
