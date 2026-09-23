- **Google sign-in no longer dies as ERR_TOO_MANY_REDIRECTS.** The OAuth
  relocation (added with the pinned-origin rule) answered "relocate to the
  pin" whenever the derived origin string differed from `AUTH_PUBLIC_URL` —
  and when the difference was a LIE the proxy chain told, the redirect
  pointed back at the URL the browser was already on: follow it, resend the
  same headers, derive the same "wrong" origin, redirect again, until the
  browser's budget died. The lies that triggered it are ordinary deployment
  shapes: an outer proxy that states the host but not the scheme (the app
  host backfills `x-forwarded-proto: http`, and an https pin never equals
  an http hit), an explicit `:443` on the forwarded host, a typed-uppercase
  pin, a comma-joined proxy chain. The one-origin check is now a HOST
  check — cookies (the thing the relocation protects) are host-scoped, and
  scheme and port never partition the jar, so a browser already on the
  pinned host is already home and no redirect could have helped it. A
  genuinely different host (the LAN URL the pin exists to retire) still
  moves home in one hop, path and query intact; all five start/callback
  routes share the one fixed function.
