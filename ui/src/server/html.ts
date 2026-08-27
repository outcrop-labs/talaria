// Tiny server leaf for the few HTML strings Talaria builds by hand (email
// bodies; KB search snippets escape through this same function where it used
// to live as a private const in kb.ts). One definition, because "escaped for
// HTML" must mean the same five replacements everywhere it is claimed.
export const escapeHtml = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')
