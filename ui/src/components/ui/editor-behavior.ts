import { Extension } from '@tiptap/core'
import { liftEmptyBlock } from '@tiptap/pm/commands'
import { Plugin, PluginKey } from '@tiptap/pm/state'

// Standard "don't get stuck in a block" behaviours, matching what Notion/Outline
// do. Two pieces:
//   1. Enter on an empty line inside a wrapping container (blockquote, and any
//      future callout / notice / toggle block) lifts you out of it.
//   2. The document always ends in a paragraph, so a terminal blockquote /
//      table / code block / image is never a dead end — you can click or
//      arrow-down into the trailing line.

// Blocks that handle their own Enter/exit and must NOT be lifted out of:
//  - lists exit on Enter-at-empty (list/task item),
//  - table cells keep Enter inside the cell,
//  - code blocks exit on triple-Enter / arrow-down,
//  - the top document (a top-level empty Enter just adds a paragraph).
const NO_LIFT = new Set(['doc', 'listItem', 'taskItem', 'tableCell', 'tableHeader'])

const ExitOnEnter = Extension.create({
  name: 'exitOnEnter',
  // Run before the default Enter (which would continue the block).
  priority: 1000,
  addKeyboardShortcuts() {
    return {
      Enter: () => {
        const { editor } = this
        const { $from, empty } = editor.state.selection
        if (!empty) return false
        const block = $from.parent
        const atEnd = $from.parentOffset === block.content.size

        // 1) Enter at the end of a heading starts body copy — the next line is a
        //    paragraph (and the heading button un-highlights, since the caret is
        //    genuinely in a paragraph now). Splitting mid-heading keeps a heading.
        if (block.type.name === 'heading' && atEnd) {
          return block.content.size === 0
            ? editor.chain().setParagraph().run() // empty heading → just become copy
            : editor.chain().splitBlock().setParagraph().run()
        }

        // 2) Enter on an empty line inside a wrapping container (blockquote, and
        //    any future callout / notice) lifts you out of it.
        if (block.isTextblock && block.content.size === 0 && block.type.name !== 'codeBlock') {
          const wrapper = $from.node($from.depth - 1)
          if (wrapper && !NO_LIFT.has(wrapper.type.name)) {
            // liftEmptyBlock lifts the empty line out; no-ops otherwise.
            return editor.commands.command(({ state, dispatch }) => liftEmptyBlock(state, dispatch))
          }
        }
        return false
      },
    }
  },
})

const trailingNodeKey = new PluginKey('trailingNode')

const TrailingNode = Extension.create({
  name: 'trailingNode',
  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: trailingNodeKey,
        appendTransaction: (_transactions, _oldState, state) => {
          const { doc, tr, schema } = state
          const last = doc.lastChild
          if (last && last.type.name === 'paragraph') return null
          const paragraph = schema.nodes.paragraph
          if (!paragraph) return null
          return tr.insert(doc.content.size, paragraph.create())
        },
      }),
    ]
  },
})

/** Bundle the block-escape behaviours as one extension for the editor. */
export const BlockEscape = Extension.create({
  name: 'blockEscape',
  addExtensions() {
    return [ExitOnEnter, TrailingNode]
  },
})
