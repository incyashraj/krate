/* The docs-grade chrome mdBook does not ship: a right-hand "On this page"
 * rail built from the page's own headings, with the current section
 * highlighted as you scroll, and a language header on every code block with
 * the copy control inside it. All client-side, all optional -- with scripts
 * off the page is an ordinary mdBook page.
 */
(function () {
  "use strict";

  var content = document.querySelector(".content main");
  if (!content) return;

  /* ---------------------------------------------------- on this page rail */

  var headings = content.querySelectorAll("h2, h3");
  if (headings.length >= 2 && window.matchMedia("(min-width: 1400px)").matches) {
    var rail = document.createElement("nav");
    rail.className = "krate-rail";
    rail.setAttribute("aria-label", "On this page");

    var title = document.createElement("div");
    title.className = "krate-rail-title";
    title.textContent = "On this page";
    rail.appendChild(title);

    var links = [];
    headings.forEach(function (h) {
      // mdBook wraps heading text in a self-link; the id lives on the heading.
      var id = h.id || (h.querySelector("a[id]") || {}).id;
      if (!id) return;
      var a = document.createElement("a");
      a.href = "#" + id;
      a.textContent = h.textContent.replace(/^\s*|\s*$/g, "");
      a.className = "krate-rail-link" + (h.tagName === "H3" ? " sub" : "");
      rail.appendChild(a);
      links.push({ a: a, h: h });
    });

    if (links.length >= 2) {
      document.body.appendChild(rail);

      var setActive = function (active) {
        links.forEach(function (l) {
          l.a.classList.toggle("active", l.a === active);
        });
      };
      var spy = new IntersectionObserver(
        function (entries) {
          entries.forEach(function (e) {
            if (e.isIntersecting) {
              var hit = links.find(function (l) { return l.h === e.target; });
              if (hit) setActive(hit.a);
            }
          });
        },
        { rootMargin: "-80px 0px -70% 0px" }
      );
      links.forEach(function (l) { spy.observe(l.h); });
    }
  }

  /* ------------------------------------------------- code block headers */

  content.querySelectorAll("pre > code[class*='language-']").forEach(function (code) {
    var pre = code.parentElement;
    var lang = (code.className.match(/language-(\w+)/) || [])[1] || "";
    // mermaid fences become rendered diagrams (mermaid-render.js); a code
    // header on a diagram is chrome on the wrong thing.
    if (lang === "mermaid") return;
    var pretty = { sh: "Bash", bash: "Bash", shell: "Bash", console: "Bash",
                   rust: "Rust", toml: "TOML", json: "JSON", wit: "WIT",
                   powershell: "PowerShell", python: "Python",
                   text: "Output", txt: "Output" }[lang] || lang;
    if (!pretty) return;

    var head = document.createElement("div");
    head.className = "krate-code-head";
    var label = document.createElement("span");
    label.textContent = pretty;
    head.appendChild(label);

    // mdBook puts its copy button inside the <pre>; move it into the header
    // so the block reads like a titled card rather than code with a hoverer.
    var copy = pre.querySelector(".clip-button, .copy-button, button");
    if (copy) head.appendChild(copy);

    pre.parentElement.insertBefore(head, pre);
    pre.classList.add("has-head");
  });
})();
