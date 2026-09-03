// The browser tab's title, decided in exactly one place. The company name
// comes from the identity beacon (instance-branding.svelte.ts); this pure
// module exists so the title's shape stays pinned by a plain vitest import —
// a .svelte.ts module pulls in the query stack, which tests never touch.

/** "Talaria" alone when no company name is configured, "Talaria - <name>"
 *  when one is. A whitespace-only name (or a failed read's undefined) falls
 *  back to the bare product name — the tab can never render "Talaria - ". */
export function tabTitle(companyName: string | null | undefined): string {
  const name = (companyName ?? '').trim()
  return name ? `Talaria - ${name}` : 'Talaria'
}
