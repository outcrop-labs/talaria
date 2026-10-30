<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte'
  import type { Snippet } from 'svelte'
  import type { Readable } from 'svelte/store'
  import { createEditor, EditorContent, type Editor } from 'svelte-tiptap'
  import StarterKit from '@tiptap/starter-kit'
  import Link from '@tiptap/extension-link'
  import Placeholder from '@tiptap/extension-placeholder'
  import { Markdown } from 'tiptap-markdown'
  import { Extension } from '@tiptap/core'
  import Suggestion from '@tiptap/suggestion'
  import { PluginKey } from '@tiptap/pm/state'
  // The suggestion plugin speaks @tiptap/core's Editor; `$editor` below is
  // svelte-tiptap's subclass of it, so the two names stay apart.
  import type { Editor as CoreEditor, Range } from '@tiptap/core'
  import { flushSync, mount, unmount } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { MentionSuggest } from '@/components/ui/mention-suggest'
  import { EmojiSuggest } from '@/components/chat/emoji-suggest'
  import { menuHandlesKey, shouldSendNowOnModEnter, shouldSendOnEnter } from '@/components/chat/composer-keys'
  import CommandMenu from '@/components/chat/CommandMenu.svelte'
  import { commandItems, expandSlashCommand, platformSkills, type CommandItem } from '@/components/chat/command-suggest'
  import { BLUEPRINTS_KEY, createBlueprint, deleteBlueprint, useBlueprints } from '@/lib/blueprints'
  import { useSkills } from '@/lib/skills'
  import { useSession } from '@/lib/session'
  import { confirm, prompt } from '@/components/ui/confirm.svelte'
  import { toastError } from '@/lib/toast.svelte'
  import { placeMenu } from '@/lib/menu-position'
  import SendButton from '@/components/chat/SendButton.svelte'
  import StopButton from '@/components/chat/StopButton.svelte'
  import ComposerToolbar from './ComposerToolbar.svelte'
  import { emojify } from '@/lib/emoji'
  import type { Mentionable } from '@/components/chat/mentions.svelte'
  import { claimDraftKey, clearDraft, parseNewAgentDraftKey, readDraft, releaseDraftKey, saveDraft, useCommsDraftScope } from '@/lib/comms-drafts'

  // The Slack-shaped message editor: rich formatting with markdown under the
  // hood. Type syntax (**bold**, `code`, ``` blocks, > quotes, - lists) or use
  // the toolbar; @ mentions and : emoji autocomplete inline. Enter sends —
  // except inside a code block, where it makes a newline (Slack semantics);
  // Shift+Enter is always a soft newline. An armed-but-empty autocomplete
  // menu claims nothing: Enter falls through and sends (TALA-5). While a reply
  // streams the host hands over `onSendNow` and ⌘/Ctrl+Enter becomes the
  // mid-turn send — Enter still queues. Messages travel as markdown, which is
  // exactly what the message list renders and agents read.
  //
  // Mercury anatomy (spec §7, updated): the editor sits in an INSET prompt well
  // — ground background, hairline border, radius 6, 14px padding all round —
  // and NOTHING else; the gold 36×36 send tile is the LAST item on the 36px
  // control rail below, outside the well, every other control before it. The
  // host supplies the outer #141312 panel (strong border, radius 8, padding 8)
  // so attachment chips can ride between well and rail.

  let {
    placeholder,
    mentionables,
    onSubmit,
    onFiles,
    onEscape,
    commands = false,
    onEmptyChange,
    onInput,
    disabled,
    canSend,
    onStop,
    onSendNow,
    leftControls,
    rightControls,
    controlRail,
    compactOnNarrow,
    draftKey,
  }: {
    placeholder: string
    mentionables?: Mentionable[]
    /** Fired with the message as markdown (already emojified). */
    onSubmit: (markdown: string) => void
    /** Pasted/dropped files — the host uploads and tracks pending chips. */
    onFiles?: (files: File[]) => void
    onEscape?: () => void
    /** Offer "/" commands — the shipped skills and the viewer's blueprints — and
     *  expand a leading one on send. AGENT-FACING HOSTS ONLY: a human channel
     *  message that begins "/github" is a message, not a request to use a
     *  skill, and must travel as typed. Off, the composer runs no command
     *  reads at all. */
    commands?: boolean
    /** Notifies emptiness changes so the host can drive its send hint. */
    onEmptyChange?: (empty: boolean) => void
    /** The person changed the text (focused edits only — restoring a draft
     *  or clearing after send doesn't fire it). Drives typing presence. */
    onInput?: (empty: boolean) => void
    disabled?: boolean
    /** Host override for the send tile's enabled state (e.g. pending
     *  attachments make an empty editor sendable). Falls back to "editor has
     *  content" when omitted. */
    canSend?: boolean
    /** THE SEND TILE BECOMES THE STOP TILE while this is set: the host passes
     *  its stop handler for exactly as long as a reply is streaming, and the
     *  rail's last item swaps the gold ArrowUp for the pulsing stop square —
     *  one tile, two jobs, never both at once. Enter still submits (the host
     *  queues the message into the streaming turn) and Escape still reaches
     *  `onEscape`. */
    onStop?: () => void
    /** THE MID-TURN SEND, set for exactly as long as a reply is streaming.
     *  Enter keeps its promise — a message sent while the agent replies queues
     *  and never interrupts — and this is the other offer: hand the draft to
     *  the turn that is already going. It buys ⌘/Ctrl+⏎ (the host renders the
     *  visible control itself, and reaches this through `sendNow()`). Unset,
     *  ⌘⏎ is not ours and falls through to the editor's own hard break. */
    onSendNow?: (markdown: string) => void
    /** Host controls rendered at the START of the bottom row (attach, emoji). */
    leftControls?: Snippet
    /** Host controls rendered at the END of the bottom row (send hint, tiers, stop). */
    rightControls?: Snippet
    /** Complete replacement for the bottom rail. Used by surfaces whose
     *  control order is part of the product contract (the Inbox composer, for one). */
    controlRail?: Snippet
    /** Dense docks may hide the formatting strip below the sm breakpoint. */
    compactOnNarrow?: boolean
    /** Where unsent text is kept (lib/comms-drafts.ts). Omitted: the key Comms
     *  provides for its selection, if this composer is inside Comms. Null:
     *  no draft at all. */
    draftKey?: string | null
  } = $props()

  // ── "/" slash commands (./command-suggest.ts) ──────────────────────────────
  // The third suggestion menu in this well, beside "@" and ":". Two sources:
  // the SHIPPED skills an agent already knows how to run, and the viewer's
  // BLUEPRINTS — text they kept to send again. The reads live HERE rather than
  // in each host because every composer offers the same commands: a prop would
  // have been the same two queries wired six times, and a host that forgot one
  // would silently have no menu. One query key each, so it is one fetch.
  const qc = useQueryClient()
  // Created only when the host asked for commands: a composer over a human
  // channel fetches neither list. `commands` is fixed for a composer's life,
  // so the conditional creation is stable.
  // svelte-ignore state_referenced_locally -- reason: `commands` is fixed for a composer's life; the reads exist or not, they never toggle
  const skillsQuery = commands ? useSkills() : null
  // svelte-ignore state_referenced_locally -- reason: same — one decision at mount
  const blueprintsQuery = commands ? useBlueprints() : null
  const session = useSession()
  const commandSkills = $derived(platformSkills(skillsQuery?.data ?? []))
  const blueprints = $derived(blueprintsQuery?.data ?? [])
  const viewerId = $derived(session.data?.id ?? null)
  /** One list of names for BOTH halves: what the menu can insert is exactly
   *  what the send path expands. */
  const commandNames = $derived(commandSkills.map((s) => s.name))

  let empty = $state(true)
  let editor = $state() as Readable<Editor>

  // What travels: markdown, emojified. One serialization for both sends, so
  // the mid-turn one can never drift from the ordinary one.
  //
  // A SLASH COMMAND IS EXPANDED HERE, on the one path both sends share: the
  // person typed "/github fix the CI", and what they sent — what the agent
  // reads and what the transcript shows — is the sentence that asks for it.
  // Doing it in the host would have meant doing it in every host, and a
  // transcript that disagreed with the message in the one that forgot.
  const outgoing = (e: Editor): string => {
    const text = emojify((e.storage.markdown as { getMarkdown: () => string }).getMarkdown().trim())
    return commands ? expandSlashCommand(text, commandNames) : text
  }

  const submit = () => {
    const e = $editor
    if (!e || e.isEmpty) {
      onSubmit('')
      return
    }
    onSubmit(outgoing(e))
  }

  // The keymap is bound once at editor creation; props in runes mode are live
  // bindings, so the closures below always see the fresh handlers (React
  // needed submitRef/escapeRef for this).
  const SendKeymap = Extension.create({
    name: 'chatSendKeymap',
    addKeyboardShortcuts() {
      return {
        Enter: () => {
          // Inside a code block Enter writes code; everywhere else it sends.
          // An Enter that belongs to an IME composition is never a send — the
          // composition's own confirm keydown must not fire the message
          // (shouldSendOnEnter's belt to ProseMirror's suspend-on-compose).
          if (!shouldSendOnEnter({ inCodeBlock: this.editor.isActive('codeBlock'), compositionActive: this.editor.view.composing })) {
            return false
          }
          submit()
          return true
        },
        'Mod-Enter': () => {
          // The mid-turn send, and only while the host offers one. Tiptap
          // reverses the extension list before building keymaps, so a keymap
          // declared after StarterKit is consulted first — returning false
          // here hands the press back to HardBreak, which is what ⌘⏎ has
          // always done in this well.
          if (
            !shouldSendNowOnModEnter({
              offered: !!onSendNow,
              hasText: !this.editor.isEmpty,
              compositionActive: this.editor.view.composing,
            })
          ) {
            return false
          }
          sendNow()
          return true
        },
        'Shift-Enter': () =>
          this.editor.commands.first(({ commands }) => [
            () => commands.createParagraphNear(),
            () => commands.liftEmptyBlock(),
            () => commands.splitBlock(),
          ]),
        Escape: () => {
          if (onEscape) {
            onEscape()
            return true
          }
          return false
        },
      }
    },
  })

  // ── The "/" menu's plumbing ────────────────────────────────────────────────
  // Clone of the "@"/":" pattern (mention-suggest.ts, emoji-suggest.ts): tiptap
  // drives the menu from outside any component tree, so CommandMenu is mounted
  // imperatively and fed through the handle it exports. What differs is that
  // the rows here can WRITE — a blueprint replaces the draft, the last row
  // saves it — and those mutations need this component's query client, which
  // is the other reason the extension is built here and not in a module.

  /** The imperative surface CommandMenu.svelte exports (mount() returns it). */
  interface CommandMenuHandle {
    update: (items: CommandItem[], command: (item: CommandItem) => void, remove: (item: CommandItem) => void) => void
    onKeyDown: (e: KeyboardEvent) => boolean
  }

  /** Every row the menu shows for what has been typed, save row included.
   *  `markdownOf` is the drafts helper below — the composer's whole text is
   *  what the save row would keep. */
  const commandRowsFor = (e: CoreEditor, query: string): CommandItem[] =>
    commandItems({ query, skills: commandSkills, blueprints, text: e.isEmpty ? '' : markdownOf(e), viewerId })

  /** The save row: name it, keep it, and leave the well holding the body
   *  alone — the "/" that opened the menu was a gesture, not content. */
  const saveAsBlueprint = async (body: string) => {
    if (!body.trim()) return
    const name = await prompt({ title: 'Save as blueprint', placeholder: 'Blueprint name', confirmLabel: 'Save' })
    const trimmed = name?.trim()
    if (!trimmed) return
    try {
      await createBlueprint({ name: trimmed, body })
      void qc.invalidateQueries({ queryKey: BLUEPRINTS_KEY })
      $editor?.chain().setContent(body, true).focus('end').run()
    } catch (e) {
      toastError('Could not save the blueprint', e)
    }
  }

  const runCommand = ({ editor: e, range, props }: { editor: CoreEditor; range: Range; props: CommandItem }) => {
    if (props.kind === 'skill') {
      // The token, not the sentence: the well keeps reading as a command and
      // `outgoing` expands it on the way out.
      e.chain().focus().deleteRange(range).insertContent(`/${props.label} `).run()
      return
    }
    if (props.kind === 'blueprint') {
      // Mid-command by construction — the only thing lost is the "/…" — so the
      // body REPLACES the draft without asking.
      e.chain().setContent(props.body ?? '', true).focus('end').run()
      return
    }
    void saveAsBlueprint(props.body ?? '')
  }

  type CommandRenderProps = {
    items: CommandItem[]
    command: (item: CommandItem) => void
    query: string
    editor: CoreEditor
    clientRect?: (() => DOMRect | null) | null
  }

  const CommandSuggest = Extension.create({
    name: 'commandSuggest',
    addProseMirrorPlugins() {
      return [
        Suggestion<CommandItem>({
          editor: this.editor,
          pluginKey: new PluginKey('commandSuggest'),
          char: '/',
          // START OF LINE, ALWAYS: a slash mid-sentence belongs to a URL, a
          // path or a date, and must never open a menu over what is being
          // written. `allowSpaces` false ends the command at the first space,
          // so the ask after it is prose again.
          startOfLine: true,
          allowSpaces: false,
          items: ({ query, editor: e }) => commandRowsFor(e, query),
          command: runCommand,
          render: () => {
            let menu: CommandMenuHandle | null = null
            let popup: HTMLDivElement | null = null
            // The list the menu was last rendered with — what onKeyDown
            // consults, so an armed-but-empty menu falls through instead of
            // eating Enter (TALA-5, composer-keys.ts).
            let current: CommandItem[] = []
            // The last render's props, so a deleted blueprint can leave the
            // list it was deleted from: the plugin only re-runs `items` on the
            // next keystroke, and a row that stays after its × looks broken.
            let last: CommandRenderProps | null = null
            // Escape DISMISSES the command, not just the popup: the suggestion
            // plugin stays active until the token is gone, so without this a
            // dismissed menu re-armed itself invisibly on the next keystroke and
            // ate the Enter that meant send. Cleared when the suggestion exits.
            let dismissed = false

            const removeRow = async (item: CommandItem) => {
              if (!item.id) return
              // Quiet trigger, deliberate step: the × asks before it deletes,
              // because the thing it deletes is text somebody wrote.
              const go = await confirm({
                title: 'Delete blueprint',
                message: `“${item.label}” will stop being offered here. This cannot be undone.`,
                confirmLabel: 'Delete blueprint',
                danger: true,
              })
              if (!go) return
              await deleteBlueprint(item.id)
              await qc.invalidateQueries({ queryKey: BLUEPRINTS_KEY })
              if (last) ensure({ ...last, items: commandRowsFor(last.editor, last.query) })
            }

            // Creatable from onUpdate too, not only onStart: the first result
            // set can be empty (a query nothing matches), and onStart alone
            // would then never show the menu for the rest of the command.
            const ensure = (props: CommandRenderProps) => {
              if (dismissed) {
                current = []
                return
              }
              current = props.items
              last = props
              if (menu) {
                menu.update(props.items, (item: CommandItem) => props.command(item), (item: CommandItem) => void removeRow(item))
              } else if (props.items.length) {
                popup = document.createElement('div')
                popup.style.position = 'fixed'
                popup.style.zIndex = '60'
                document.body.appendChild(popup)
                menu = mount(CommandMenu, { target: popup }) as unknown as CommandMenuHandle
                menu.update(props.items, (item: CommandItem) => props.command(item), (item: CommandItem) => void removeRow(item))
              }
              // Render synchronously so placeMenu() can measure the popup.
              flushSync()
              const rect = props.clientRect?.()
              if (popup && rect) placeMenu(popup, rect)
            }

            return {
              onStart: ensure,
              onUpdate: ensure,
              onKeyDown: (props) => {
                if (props.event.key === 'Escape') {
                  popup?.remove()
                  popup = null
                  if (menu) {
                    void unmount(menu as unknown as Record<string, unknown>)
                    menu = null
                  }
                  // And it claims nothing more, now or on the next keystroke: a
                  // dismissed menu that kept its candidate list would go on
                  // eating Enter invisibly, and the key that sends a message
                  // has to mean send.
                  current = []
                  dismissed = true
                  return true
                }
                if (!menuHandlesKey(props.event.key, current)) return false
                return menu?.onKeyDown(props.event) ?? false
              },
              onExit: () => {
                popup?.remove()
                popup = null
                last = null
                dismissed = false
                if (menu) {
                  void unmount(menu as unknown as Record<string, unknown>)
                  menu = null
                }
              },
            }
          },
        }),
      ]
    },
  })

  onMount(() => {
    // Canonical svelte-tiptap setup (see RichEditor.svelte): the editor is
    // created onMount (SSR-safe) and exposed as a readable store that emits on
    // every transaction. (React's immediatelyRender/shouldRerenderOnTransaction
    // knobs are @tiptap/react concepts with no svelte-tiptap equivalent.)
    editor = createEditor({
      editable: !disabled,
      extensions: [
        // Chat-sized vocabulary: no headings/hr — Slack messages don't have them.
        StarterKit.configure({ heading: false, horizontalRule: false }),
        Link.configure({ openOnClick: false, autolink: true }),
        Placeholder.configure({ placeholder }),
        Markdown.configure({ html: false, breaks: true, transformPastedText: true }),
        SendKeymap,
        // Installed unconditionally with a LIVE read: the candidate list is
        // `$derived` over async queries everywhere it is built, so a snapshot
        // here froze whoever had resolved by mount (and an empty-at-mount
        // list installed nothing — the @ key did nothing at all).
        MentionSuggest.configure({ items: () => mentionables ?? [] }),
        EmojiSuggest,
        ...(commands ? [CommandSuggest] : []),
      ],
      content: '',
      editorProps: {
        // Prompt well interior: transparent 14/20 sans on the ground inset,
        // 14px padding all round, min-height ~76px (spec §7). The well holds
        // ONLY the text — the send tile lives on the rail below.
        attributes: {
          class: 'tiptap max-h-40 overflow-y-auto px-3.5 py-3.5 font-sans text-sm leading-5',
          style: 'min-height:4.625rem',
        },
        handlePaste: (_view, event) => {
          const files = Array.from(event.clipboardData?.files ?? [])
          if (files.length === 0) return false
          event.preventDefault()
          onFiles?.(files)
          return true
        },
        handleDrop: (_view, event) => {
          const files = Array.from(event.dataTransfer?.files ?? [])
          if (files.length === 0) return false
          event.preventDefault()
          onFiles?.(files)
          return true
        },
      },
      onUpdate: ({ editor }) => {
        empty = editor.isEmpty
        onEmptyChange?.(editor.isEmpty)
        if (editor.isFocused) onInput?.(editor.isEmpty)
        // No owned key (outside Comms, or another composer holds it): nothing
        // to save, so skip even scheduling the serialization.
        if (!owned) return
        queueDraftSave(editor)
      },
    })
  })

  // ── Drafts (KTD10) ─────────────────────────────────────────────────────────
  // Unsent text is saved per conversation while you type (debounced), put back
  // when you return, and dropped on send (`clear()`). Inside Comms the key
  // comes from the view's selection; the first composer to claim a key owns it,
  // so a thread-reply or modal composer in the same channel stays out of it.
  const draftScope = useCommsDraftScope()
  const draftToken = Symbol('composer')
  const wantedKey = $derived(draftKey !== undefined ? draftKey : (draftScope?.key() ?? null))
  const draftUser = $derived(draftScope?.userId() ?? null)
  /** The key this composer owns right now, and for whom. */
  let owned: { user: string; key: string } | null = null
  /** A draft key is owned: what is typed here survives a reload, so the
   *  desktop shell may suspend the page with text in it (lib/desktop-suspend:
   *  `data-desktop-draft`). */
  let keepsDraft = $state(false)
  /** The owned key has unsaved edits; the editor is serialized at flush. */
  let pending: { user: string; key: string; editor: DraftSource } | null = null
  /** What the store already holds for the owned key — restoring a draft must
   *  not re-save it (that would re-date it in the Drafts view). */
  let stored = ''
  let saveTimer: ReturnType<typeof setTimeout> | undefined

  type DraftSource = { storage: Record<string, unknown>; isEmpty: boolean }

  const markdownOf = (e: { storage: Record<string, unknown> }): string =>
    (e.storage.markdown as { getMarkdown: () => string }).getMarkdown().trim()

  // Serialize once per debounce window, not per keystroke. Text equal to what
  // the store already holds is not re-saved.
  const flushDraft = () => {
    clearTimeout(saveTimer)
    const p = pending
    pending = null
    if (!p) return
    const text = p.editor.isEmpty ? '' : markdownOf(p.editor)
    if (text === stored) return
    stored = text
    saveDraft(p.user, p.key, text)
  }

  const queueDraftSave = (e: DraftSource) => {
    if (!owned) return
    pending = { ...owned, editor: e }
    clearTimeout(saveTimer)
    saveTimer = setTimeout(flushDraft, 300)
  }

  const releaseOwned = () => {
    if (owned && draftScope) releaseDraftKey(draftScope, owned.key, draftToken)
    owned = null
    keepsDraft = false
  }

  // Follow the key: the agent pane keeps one composer across threads, so a
  // switch saves the old draft and loads the new one. Unsent text in a brand
  // new agent thread carries into the thread it just became.
  $effect(() => {
    const e = $editor
    const key = wantedKey
    const user = draftUser
    if (!e) return
    untrack(() => {
      if (owned && owned.key === key && owned.user === user) return
      flushDraft()
      const prev = owned
      releaseOwned()
      if (!key || !user || !draftScope || !claimDraftKey(draftScope, key, draftToken)) return
      owned = { user, key }
      keepsDraft = true
      stored = ''
      const carry = !!prev && parseNewAgentDraftKey(prev.key) !== null && parseNewAgentDraftKey(key) === null
      if (carry && !e.isEmpty) {
        stored = markdownOf(e)
        saveDraft(user, key, stored)
        // The text now lives under the conversation; the fresh-thread key it
        // was typed under would otherwise pre-fill the next new thread.
        if (prev) clearDraft(prev.user, prev.key)
        return
      }
      const draft = readDraft(user, key)
      // A first load into an empty editor, or a switch between conversations.
      if (draft !== null) {
        stored = draft
        e.commands.setContent(draft, true)
      }
      else if (prev) e.commands.clearContent(true)
    })
  })

  onDestroy(() => {
    flushDraft()
    releaseOwned()
  })

  // `disabled` can flip after mount; keep the live editor in sync. Guarded so
  // the resulting transaction doesn't re-trigger us forever.
  $effect(() => {
    const e = $editor
    if (e && e.isEditable !== !disabled) e.setEditable(!disabled)
  })

  // ── Imperative handle (ChatComposerHandle, see ./chat-composer) ────────────
  export function focus(): void {
    $editor?.chain().focus().run()
  }
  export function insertText(text: string): void {
    $editor?.chain().focus().insertContent(text).run()
  }
  export function isEmpty(): boolean {
    return $editor?.isEmpty ?? true
  }
  /** Send the draft into the reply that is streaming (see `onSendNow`). The
   *  visible control belongs to the host's rail — beside the stop tile, where
   *  the pair reads as the two things you can do to a turn in flight — so the
   *  host needs a way in; ⌘⏎ takes the same door. */
  export function sendNow(): void {
    const e = $editor
    if (!e || e.isEmpty || !onSendNow) return
    onSendNow(outgoing(e))
  }
  export function clear(): void {
    $editor?.commands.clearContent(true)
    // Sent: the draft goes with it (the update above queued an empty save;
    // land it now rather than leave a window where a reload restores it).
    flushDraft()
  }

  // Slack's split, Mercury's skin: the inset prompt well (text only) on top;
  // the gold send tile is the LAST item on the 36px chip rail below, with
  // host controls — attach, emoji, formatting, pickers — all before it. While
  // a reply streams, the SAME tile is the stop square (see `onStop`).
  const sendEnabled = $derived(!disabled && (canSend ?? !empty))
</script>

<div class="flex min-w-0 flex-1 flex-col gap-2" data-desktop-draft={keepsDraft ? '' : undefined}>
  <div class="rounded-md border border-line bg-surface">
    <EditorContent editor={$editor} />
  </div>
  {#if controlRail}
    <div class="flex h-10 min-w-0 items-center gap-1.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {@render controlRail()}
      {#if onStop}
        <StopButton onClick={onStop} />
      {:else}
        <SendButton enabled={sendEnabled} onClick={submit} />
      {/if}
    </div>
  {:else}
    <!-- Overflow guard: a crowded rail on a narrow pane (the plan's split)
         scrolls instead of pushing the send tile past the panel's edge. -->
    <div class="flex h-10 min-w-0 items-center gap-1.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {@render leftControls?.()}
      {#if leftControls}<span class="mx-1 h-5 border-l border-line"></span>{/if}
      {#if editor}
        <ComposerToolbar {editor} class={compactOnNarrow ? 'hidden sm:flex' : undefined} />
      {/if}
      <span class="flex-1"></span>
      {@render rightControls?.()}
      {#if onStop}
        <StopButton onClick={onStop} />
      {:else}
        <SendButton enabled={sendEnabled} onClick={submit} />
      {/if}
    </div>
  {/if}
</div>
