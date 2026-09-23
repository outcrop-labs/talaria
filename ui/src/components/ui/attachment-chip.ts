// AttachmentChip — the editor's inline read-only chip for a non-image file
// embedded in a document. Images stay on their existing path (an image NODE
// serialized as `![alt](/api/uploads/<id>)`); this node is for everything
// else, and its markdown token is a link with a custom scheme:
//
//   [<filename>](attachment://<uuid>?s=<size>&m=<enc(mime)>)
//
// Grammar lives in @/lib/attachment-token — one module, editor and renderer.
//
// Why a link-shaped token at all: markdown-it passes the scheme through
// (validateLink blocks only javascript:/vbscript:/file:/data:), tiptap's own
// Link mark REFUSES the href (isAllowedUri's scheme list has no
// `attachment:`), and prosemirror-model's matchTag skips a rule whose
// getAttrs answers false — so this node's parseHTML rule claims the anchor
// the Link mark just declined. No priority games, no collision: ordinary
// links and `![…]()` images never match the regex either way.

import type { Node as PMNode } from '@tiptap/pm/model'
import type { MarkdownSerializerState } from '@tiptap/pm/markdown'
import { Node, mergeAttributes } from '@tiptap/core'

import { attachmentHref, parseAttachmentHref } from '@/lib/attachment-token'
import { humanSize } from '@/lib/attachments'

// The chip's visual, verbatim from MessageAttachments.svelte's file chip
// (the chat surface's spelling) — with the two inline adjustments the
// editor's chip needs: `max-w-full` truncates against the text column, and
// the size rides in a shrink-0 span so a long name can't push it out.
const CHIP_CLASS =
  'inline-flex max-w-full items-center gap-2 rounded-md border border-line bg-raised px-2.5 py-1.5 font-sans text-xs text-fg transition-colors hover:border-line-strong'

export interface AttachmentChipAttrs {
  id: string
  filename: string
  mime: string
  size: number
}

export const AttachmentChip = Node.create({
  name: 'attachmentChip',
  inline: true,
  // An atom: the chip is one unit in the doc — selected whole, deleted
  // whole, never edited in place (read-only inline, per the design).
  atom: true,
  group: 'inline',

  addAttributes() {
    return {
      id: { default: null },
      filename: {
        default: 'file',
        // markdown-it renders the href in <a> but the filename only as link
        // TEXT — data-filename is the authoritative slot both DOM shapes
        // (markdown-it's and our own renderHTML) parse identically from.
        parseHTML: (dom) => dom.getAttribute('data-filename') || dom.textContent || 'file',
      },
      mime: {
        default: '',
        parseHTML: (dom) => parseAttachmentHref(dom.getAttribute('href') ?? '')?.mime ?? '',
      },
      size: {
        default: 0,
        parseHTML: (dom) => parseAttachmentHref(dom.getAttribute('href') ?? '')?.size ?? 0,
      },
    }
  },

  parseHTML() {
    return [
      {
        tag: 'a[href^="attachment://"]',
        // False → matchTag falls through to the next rule; an anchor whose
        // href is not exactly our grammar is somebody's hand edit and must
        // render as the plain link it looks like.
        getAttrs: (dom) => {
          const t = parseAttachmentHref(dom.getAttribute('href') ?? '')
          if (!t) return false
          return {
            id: t.id,
            filename: dom.getAttribute('data-filename') || dom.textContent || 'file',
            mime: t.mime,
            size: t.size,
          }
        },
      },
    ]
  },

  renderHTML({ node, HTMLAttributes }) {
    const { filename, size, ...rest } = node.attrs as AttachmentChipAttrs
    return [
      'a',
      mergeAttributes(HTMLAttributes, {
        href: attachmentHref({ ...rest, size }),
        'data-filename': filename,
        class: CHIP_CLASS,
      }),
      ['span', { class: 'min-w-0 truncate' }, filename],
      ['span', { class: 'shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted' }, humanSize(size)],
    ]
  },

  addStorage() {
    return {
      markdown: {
        // The image node's serializer is defaultMarkdownSerializer.nodes.image;
        // ours is one write() — an inline atom, no content to close. The
        // filename is escaped so brackets/parens in a name survive the
        // round-trip; esc() is the serializer's own, same as the image path.
        serialize: (state: MarkdownSerializerState, node: PMNode) => {
          const { filename, ...rest } = node.attrs as AttachmentChipAttrs
          state.write(`[${state.esc(filename)}](${attachmentHref(rest)})`)
        },
      },
    }
  },
})
