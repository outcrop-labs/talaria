- **Skeletons were invisible.** The entire skeleton system (and tiptap
  table borders, and the grey status dots) painted with `var(--theme-line)`
  — a variable that never existed as raw CSS. One root alias fixes every
  shimmer at once.
