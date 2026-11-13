import { getContext, hasContext, setContext } from 'svelte'

/** `mono` is the house chrome voice. `sans` is the settings area's sentence-case voice. */
export type UiVoice = 'mono' | 'sans'

const KEY = Symbol('ui-voice')

/** Set the voice for this component's subtree. Context follows the component
 *  tree, so a modal or menu the subtree opens inherits it through its portal. */
export function setUiVoice(voice: UiVoice): void {
  setContext(KEY, voice)
}

/** The voice of the nearest ancestor that set one. Call during component init. */
export function useUiVoice(): UiVoice {
  return hasContext(KEY) ? getContext<UiVoice>(KEY) : 'mono'
}

let sansScreens = 0

/** Declare the screen a sans area until the returned function runs. A host
 *  mounted at the app root (the confirm dialog) sits above every page, where
 *  no context reaches it, so it asks `screenVoice()` when it opens. */
export function holdSansScreen(): () => void {
  sansScreens += 1
  return () => {
    sansScreens -= 1
  }
}

/** The voice of the area on screen right now. */
export function screenVoice(): UiVoice {
  return sansScreens > 0 ? 'sans' : 'mono'
}
