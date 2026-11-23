// Turn adjacent C++ and Rust code blocks into a tabbed example.
//
// The Markdown source keeps both blocks as plain fenced code (C++ first,
// Rust second), so the book still reads correctly without JavaScript.
(() => {
  "use strict";

  const STORAGE_KEY = "libsirius-lang";
  const LANGS = [
    { id: "cpp", label: "C++" },
    { id: "rust", label: "Rust" },
  ];

  const readPreference = () => {
    try {
      return localStorage.getItem(STORAGE_KEY) || "cpp";
    } catch {
      return "cpp";
    }
  };

  const writePreference = (lang) => {
    try {
      localStorage.setItem(STORAGE_KEY, lang);
    } catch {
      // Storage may be unavailable (private windows, file:// URLs).
    }
  };

  const languageOf = (pre) => {
    const code = pre && pre.tagName === "PRE" ? pre.querySelector("code") : null;
    if (!code) return null;
    for (const { id } of LANGS) {
      if (code.classList.contains(`language-${id}`)) return id;
    }
    return null;
  };

  const groups = [];

  const select = (lang) => {
    for (const group of groups) {
      for (const [id, { button, pre }] of Object.entries(group)) {
        const active = id === lang;
        button.setAttribute("aria-selected", String(active));
        button.tabIndex = active ? 0 : -1;
        pre.hidden = !active;
      }
    }
  };

  for (const first of document.querySelectorAll("main pre")) {
    if (languageOf(first) !== "cpp") continue;
    const second = first.nextElementSibling;
    if (languageOf(second) !== "rust") continue;

    const wrapper = document.createElement("div");
    wrapper.className = "lang-tabs";
    const bar = document.createElement("div");
    bar.className = "lang-tabs-bar";
    bar.setAttribute("role", "tablist");
    wrapper.appendChild(bar);
    first.before(wrapper);

    const group = {};
    for (const [index, pre] of [first, second].entries()) {
      const { id, label } = LANGS[index];
      const button = document.createElement("button");
      button.type = "button";
      button.className = "lang-tabs-button";
      button.setAttribute("role", "tab");
      button.textContent = label;
      button.addEventListener("click", () => {
        writePreference(id);
        select(id);
      });
      bar.appendChild(button);
      wrapper.appendChild(pre);
      group[id] = { button, pre };
    }
    groups.push(group);
  }

  if (groups.length > 0) select(readPreference());
})();
