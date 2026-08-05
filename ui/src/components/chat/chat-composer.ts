// Shared types for <ChatComposer> (see ChatComposer.svelte).

/** Imperative surface of <ChatComposer>. In React this was a forwardRef
 *  handle; in Svelte the component instance itself satisfies it — grab it
 *  with `bind:this={handle}` where `handle: ChatComposerHandle | null`. */
export interface ChatComposerHandle {
  focus: () => void
  insertText: (text: string) => void
  isEmpty: () => boolean
  clear: () => void
}
