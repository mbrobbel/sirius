// Render ```mermaid fences with Mermaid, loaded from a pinned CDN build.
//
// Without network access the diagrams stay visible as their source text.
(() => {
  "use strict";

  const MERMAID_URL = "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.esm.min.mjs";
  const DARK_THEMES = ["coal", "navy", "ayu"];

  const blocks = Array.from(document.querySelectorAll("main pre > code.language-mermaid"));
  if (blocks.length === 0) return;

  const nodes = blocks.map((code) => {
    const pre = code.parentElement;
    const div = document.createElement("div");
    div.className = "mermaid";
    div.textContent = code.textContent;
    pre.replaceWith(div);
    return div;
  });

  const dark = DARK_THEMES.some((theme) => document.documentElement.classList.contains(theme));

  import(MERMAID_URL)
    .then(({ default: mermaid }) => {
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: "strict",
        theme: dark ? "dark" : "neutral",
      });
      return mermaid.run({ nodes });
    })
    .catch((error) => {
      console.warn("Sirius book: Mermaid unavailable, showing diagram source.", error);
      for (const node of nodes) node.classList.add("mermaid-source");
    });
})();
