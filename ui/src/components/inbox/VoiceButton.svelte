<script lang="ts">
  import { Mic, MicOff } from '@lucide/svelte'
  import { tileBase } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'

  interface SpeechRecognitionLike {
    continuous: boolean
    interimResults: boolean
    lang: string
    onresult: ((event: { results: ArrayLike<{ 0: { transcript: string }; isFinal: boolean }> }) => void) | null
    onend: (() => void) | null
    onerror: (() => void) | null
    start: () => void
    stop: () => void
  }

  type SpeechRecognitionConstructor = new () => SpeechRecognitionLike

  let { onTranscript, disabled }: { onTranscript: (text: string) => void; disabled: boolean } = $props()

  let recording = $state(false)
  let supported = $state(true)
  let recognition: SpeechRecognitionLike | null = null

  $effect(() => () => recognition?.stop())

  const toggle = () => {
    if (recording) {
      recognition?.stop()
      recording = false
      return
    }
    const speechWindow = window as unknown as {
      SpeechRecognition?: SpeechRecognitionConstructor
      webkitSpeechRecognition?: SpeechRecognitionConstructor
    }
    const Constructor = speechWindow.SpeechRecognition ?? speechWindow.webkitSpeechRecognition
    if (!Constructor) {
      supported = false
      return
    }
    const instance = new Constructor()
    instance.continuous = true
    instance.interimResults = false
    instance.lang = navigator.language || 'en-US'
    instance.onresult = (event) => {
      const transcript = Array.from(event.results)
        .filter((result) => result.isFinal)
        .map((result) => result[0]?.transcript ?? '')
        .join(' ')
        .trim()
      if (transcript) onTranscript(`${transcript} `)
    }
    instance.onend = () => (recording = false)
    instance.onerror = () => (recording = false)
    recognition = instance
    instance.start()
    recording = true
    supported = true
  }
</script>

<button
  type="button"
  {disabled}
  onclick={toggle}
  class={cn(tileBase, recording && 'border-accent bg-accent-subtle text-accent')}
  title={supported ? (recording ? 'Stop dictation' : 'Start voice dictation') : 'Voice dictation is not available in this browser'}
  aria-pressed={recording}
>
  {#if supported}<Mic size={14} />{:else}<MicOff size={14} />{/if}
</button>
