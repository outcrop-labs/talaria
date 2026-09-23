// The attachment token — ONE grammar, two consumers. The editor serializes a
// non-image upload into it (tiptap-markdown → storage.markdown.serialize);
// the read renderer (markdown.ts) parses it back into a chip. Everything on
// both sides goes through this module, so the grammar can't fork.
//
// Shape: [<filename>](attachment://<uuid>?s=<size>&m=<enc(mime)>)
// The filename lives in the link TEXT — callers own it (state.esc() on the
// editor side, the mdast children on the renderer side); this module owns
// only the href. `s` rides as plain digits, `m` URI-encoded, so a mime's
// `+` or `/` (and anything user-declared) survives the round-trip and the
// `&` inside it cannot smuggle a second query param.
//
// PURE AND ZERO-IMPORT ON PURPOSE: markdown.ts's test graph is node-env
// vitest with no DOM — this module must load there exactly as cleanly as
// `@/lib/format` does (see that file's header).

// One uuid, exactly one `s` and one `m`, in that order — deliberately strict:
// the shape is ours end to end, and a token that fails this is NOT ours
// (an ordinary link, a hand-edited href), so the renderer must fall through
// to the plain-link path rather than guess.
export const ATTACHMENT_HREF_RE = /^attachment:\/\/([0-9a-f-]{36})\?s=(\d+)&m=([0-9a-z._~!*'()%-]+)$/i

export interface AttachmentTokenAttrs {
  id: string
  mime: string
  size: number
}

/** The href the editor's renderHTML and the renderer's download link share. */
export const attachmentHref = (attrs: AttachmentTokenAttrs): string =>
  `attachment://${attrs.id}?s=${attrs.size}&m=${encodeURIComponent(attrs.mime)}`

/** The full markdown token — `[filename](href)`. Escaping the filename is
 *  the CALLER's job (state.esc() in the serializer); handing it over raw
 *  keeps this function the inverse of parseAttachmentHref's href half. */
export const buildAttachmentToken = (filename: string, attrs: AttachmentTokenAttrs): string =>
  `[${filename}](${attachmentHref(attrs)})`

/** Parse the href half of a token — null when it is not ours. The uuid is
 *  shape-checked only; the server re-validates ($1::uuid on the bind). */
export function parseAttachmentHref(href: string): AttachmentTokenAttrs | null {
  const m = ATTACHMENT_HREF_RE.exec(href)
  if (!m) return null
  let mime: string
  try {
    mime = decodeURIComponent(m[3]!)
  } catch {
    return null
  }
  return { id: m[1]!, mime, size: Number(m[2]!) }
}
