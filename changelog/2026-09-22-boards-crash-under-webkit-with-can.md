- **Boards crash under WebKit with "Can't find variable: requestIdleCallback".**
  `BoardLayout.svelte` feature-detected the global with
  `requestIdleCallback ?? fallback` — but reading an absent global by name
  throws ReferenceError before `??` ever runs, so WebKit (Safari, and
  WebKitGTK — the desktop shell's engine) died opening any board. The guard
  is now `typeof requestIdleCallback !== 'function'`, with the fallback and
  its cleanup correctly paired. Found in the desktop shell on 2026-09-15;
  it was equally broken for browser Safari.
