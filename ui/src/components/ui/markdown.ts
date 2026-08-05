import { unified } from 'unified'
import remarkParse from 'remark-parse'
import remarkGfm from 'remark-gfm'
import remarkBreaks from 'remark-breaks'
import remarkRehype from 'remark-rehype'
import rehypeHighlight from 'rehype-highlight'
import rehypeStringify from 'rehype-stringify'
import { cn } from '@/lib/cn'
import { focusGold } from '@/components/chat/chat-chrome'

// Markdown → HTML for Markdown.svelte: GFM (tables, task lists, strikethrough,
// autolinks), soft line breaks, and syntax-highlighted fenced code. Raw HTML is
// NOT rendered (remark-rehype's safe default drops it — the same guarantee
// react-markdown gave us), so no sanitization plumbing is needed.
// Mercury-styled via semantic utilities: what react-markdown did with component
// overrides, rehypeMercury does with a hast pass below. Reuse — do not
// re-render markdown inline.

// ── @mention highlighting ────────────────────────────────────────────────────
// Same token shape the server notifies on (mentions.ts) plus the channel
// `:tier` suffix; requires a boundary before the @ so emails don't match.
// Matched tokens become `mention:` links, which rehypeMercury renders as
// a styled span — one plugin, every markdown surface at once. Text nodes
// never occur inside code/inlineCode in mdast, so code is inherently safe.
const MENTION_RE = /(^|[\s(])@([a-z0-9][a-z0-9-]*(?::[a-z0-9-]+)?)/gi

interface MdNode {
  type: string
  value?: string
  url?: string
  children?: MdNode[]
}

function remarkMentions() {
  return (tree: unknown) => {
    const walk = (node: MdNode): void => {
      if (!node.children) return
      const next: MdNode[] = []
      for (const child of node.children) {
        if (child.type === 'text' && child.value?.includes('@')) {
          const parts: MdNode[] = []
          let last = 0
          MENTION_RE.lastIndex = 0
          for (let m = MENTION_RE.exec(child.value); m; m = MENTION_RE.exec(child.value)) {
            const start = m.index + m[1]!.length
            if (start > last) parts.push({ type: 'text', value: child.value.slice(last, start) })
            parts.push({ type: 'link', url: `mention:${m[2]}`, children: [{ type: 'text', value: `@${m[2]}` }] })
            last = start + m[2]!.length + 1
          }
          if (parts.length) {
            if (last < child.value.length) parts.push({ type: 'text', value: child.value.slice(last) })
            next.push(...parts)
            continue
          }
        }
        // Don't descend into links — a nested link is invalid mdast.
        if (child.type !== 'link') walk(child)
        next.push(child)
      }
      node.children = next
    }
    walk(tree as MdNode)
  }
}

// ── Mercury element styling (react-markdown `components` → hast pass) ───────

interface HastNode {
  type: string
  tagName?: string
  properties?: Record<string, unknown>
  children?: HastNode[]
  value?: string
}

const el = (tagName: string, properties: Record<string, unknown>, children: HastNode[]): HastNode => ({
  type: 'element',
  tagName,
  properties,
  children,
})
const text = (value: string): HastNode => ({ type: 'text', value })

// Mirror react-markdown's defaultUrlTransform, which the old pipeline applied
// to every href/src: keep http(s)/mailto/relative URLs, neuter `javascript:`
// and friends. The unified pipeline has no such default, so we do it here.
const SAFE_PROTOCOL = /^(https?|ircs?|mailto|xmpp)$/i
function safeUrl(value: string): string {
  const colon = value.indexOf(':')
  const questionMark = value.indexOf('?')
  const numberSign = value.indexOf('#')
  const slash = value.indexOf('/')
  if (
    colon < 0 ||
    (slash > -1 && colon > slash) ||
    (questionMark > -1 && colon > questionMark) ||
    (numberSign > -1 && colon > numberSign) ||
    SAFE_PROTOCOL.test(value.slice(0, colon))
  ) {
    return value
  }
  return ''
}

// Straight class swaps — byte-identical to the old react-markdown overrides.
const CLASSES: Record<string, string> = {
  h1: 'mb-2 mt-4 text-xl font-semibold text-fg first:mt-0',
  h2: 'mb-2 mt-4 text-lg font-semibold text-fg first:mt-0',
  h3: 'mb-1.5 mt-3 text-base font-semibold text-fg first:mt-0',
  h4: 'mb-1.5 mt-3 text-sm font-semibold text-fg first:mt-0',
  p: 'leading-relaxed text-fg',
  ul: 'my-1 ml-5 list-disc space-y-1 marker:text-muted',
  ol: 'my-1 ml-5 list-decimal space-y-1 marker:text-muted',
  li: 'leading-relaxed text-fg',
  blockquote: 'my-2 border-l-2 border-[var(--theme-accent-border)] pl-3 italic text-muted',
  strong: 'font-semibold text-fg',
  em: 'italic',
  hr: 'my-3 border-line-subtle',
  thead: 'border-b border-line',
  th: 'px-3 py-2 text-left font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-ink-dim',
  td: 'border-t border-line-subtle px-3 py-2 align-top text-fg',
}

const classList = (v: unknown): string => (Array.isArray(v) ? v.join(' ') : String(v ?? ''))

/** The chrome CodeBlock.svelte renders, as static hast — language label row +
 *  a copy button Markdown.svelte drives by delegation (data-copy-code). Keep
 *  the markup in sync with CodeBlock.svelte. */
function codeBlockChrome(language: string | undefined, code: HastNode): HastNode {
  // Ground inset well (spec §8): --code-bg + hairline, control radius 6.
  return el('div', { className: 'group my-3 overflow-hidden rounded-md border border-line bg-[var(--code-bg)]', dataCodeBlock: '' }, [
    // Section-header row: 10px mono uppercase ink-dim label + right-aligned ghost action.
    el('div', { className: 'flex items-center justify-between border-b border-line-subtle px-3 py-1.5' }, [
      el('span', { className: 'font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim' }, [text(language || 'text')]),
      el(
        'button',
        {
          type: 'button',
          dataCopyCode: '',
          className: cn('font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg', focusGold),
        },
        [text('copy')],
      ),
    ]),
    el('pre', { className: 'overflow-x-auto px-4 py-3 font-mono text-[0.85rem] leading-relaxed' }, [code]),
  ])
}

function transformChildren(node: HastNode): void {
  if (!node.children) return
  node.children = node.children.map(transformNode).filter((n): n is HastNode => n !== null)
}

function transformNode(node: HastNode): HastNode | null {
  if (node.type !== 'element') {
    transformChildren(node)
    return node
  }

  // Fenced code first, before generic recursion, so the inner <code> keeps its
  // hljs classes and any other <code> we meet below is by definition inline.
  // (react-markdown wrapped block code in <pre>; CodeBlock renders its own.)
  if (node.tagName === 'pre') {
    const code = node.children?.find((c) => c.type === 'element' && c.tagName === 'code')
    if (!code) return node
    const language = /language-(\w+)/.exec(classList(code.properties?.className))?.[1]
    return codeBlockChrome(language, code)
  }

  transformChildren(node)
  const props = (node.properties ??= {})

  switch (node.tagName) {
    case 'code':
      props.className = 'rounded border border-line bg-card2 px-1.5 py-0.5 font-mono text-[0.85em] text-fg'
      return node
    case 'a': {
      const href = String(props.href ?? '')
      // Mentions render as a styled span — never a navigable link.
      if (href.startsWith('mention:')) {
        return el('span', { className: 'rounded bg-accent-soft px-1 font-medium text-accent' }, node.children ?? [])
      }
      props.href = safeUrl(href)
      props.target = '_blank'
      props.rel = 'noopener noreferrer'
      props.className =
        'text-accent underline decoration-[var(--theme-accent-border)] underline-offset-2 transition-colors duration-[120ms] hover:decoration-accent'
      return node
    }
    // Agent-produced images (served out of an agent container) get the
    // save-to-artifacts affordance — Markdown.svelte mounts AgentMediaImage
    // into this placeholder post-render; ordinary images render plain.
    case 'img': {
      const src = safeUrl(String(props.src ?? ''))
      if (!src) return null
      const alt = String(props.alt ?? '')
      if (src.startsWith('/api/agent-media/')) {
        return el('span', { dataAgentMedia: '', dataSrc: src, dataAlt: alt }, [])
      }
      return el('img', { src, alt, className: 'my-2 max-h-96 rounded-lg border border-line' }, [])
    }
    // Tables per spec §8: hairline separators, sans cells, mono uppercase dim
    // column headers, row hover on the body.
    case 'table':
      props.className = 'w-full border-collapse text-sm [&>tbody>tr]:transition-colors [&>tbody>tr:hover]:bg-card2'
      return el('div', { className: 'my-3 max-w-full overflow-x-auto rounded-lg border border-line' }, [node])
    default: {
      const cls = CLASSES[node.tagName!]
      if (cls) props.className = cls
      return node
    }
  }
}

function rehypeMercury() {
  return (tree: unknown) => {
    transformChildren(tree as HastNode)
  }
}

// The processor is stateless per run — build once, reuse for every render.
const processor = unified()
  .use(remarkParse)
  .use(remarkGfm)
  .use(remarkBreaks)
  .use(remarkMentions)
  .use(remarkRehype)
  // detect covers unlabeled fences; v7 skips unknown languages by default
  // (the old `ignoreMissing` flag is gone).
  .use(rehypeHighlight, { detect: true })
  .use(rehypeMercury)
  .use(rehypeStringify)

export function renderMarkdown(markdown: string): string {
  return String(processor.processSync(markdown))
}
