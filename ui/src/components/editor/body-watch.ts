/** Tells a document editor that its rich body was edited, while the caret is
 *  still in it. The rich editor reports an edit only when it loses focus, so
 *  until then Save would stay off and the first click on it would only blur.
 *
 *  The watch remembers the body as it stood when the document was last clean
 *  and compares the live body to that. It reads the body through `read`, and
 *  only when it has to: serialising a document is not free. */
export interface BodyWatch {
  /** Before an edit can land (focus, key down, pointer down): remember the body, unless one is remembered. */
  arm: () => void
  /** After an edit may have landed (input, key up, click): does the body differ from the remembered one? */
  changed: () => boolean
  /** The document is clean again (saved, restored, re-seeded): forget the remembered body. */
  reset: () => void
}

export function bodyWatch(read: () => string): BodyWatch {
  let clean: string | null = null
  return {
    arm: () => {
      clean ??= read()
    },
    changed: () => {
      // Nothing remembered: the document was marked clean with the caret still in it (⌘S). What is there now is the clean body.
      if (clean === null) {
        clean = read()
        return false
      }
      return read() !== clean
    },
    reset: () => {
      clean = null
    },
  }
}
