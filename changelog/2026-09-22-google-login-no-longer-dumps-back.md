- **Google login no longer dumps back to /login — every `Set-Cookie` survives the
  HTTP boundary.** The Response → `res.writeHead()` conversion in both wrappers
  (prod `server-entry.js`, the vite dev middleware) read headers with
  `Object.fromEntries(response.headers.entries())`, which collapses duplicate
  keys — and `Set-Cookie` is the one header that legitimately repeats. The
  OAuth login callback answers with two cookies (session on, one-shot state
  off), so the object kept only the state clear and the session cookie died at
  that hop on every deployment since the prod server landed: Google authorized,
  the SPA booted at `/`, `/api/auth/session` said `{user: null}`, and the
  cockpit bounced to /login — while password login (one cookie) worked, and
  dev never caught it because the middleware had the same one-liner. The
  conversion now lives once as `writeHeadHeaders` in `ui/src/server/http.ts`
  (re-exported through the server bundle beside `migrate`), both wrappers
  import it, and a test pins that a two-cookie response stays two cookies.
  Deployments need a rebuilt image to pick this up — no env or Google-console
  change is involved.
