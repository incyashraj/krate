(function () {
  function loadMermaid(callback) {
    if (window.mermaid) {
      callback();
      return;
    }

    var script = document.createElement("script");
    // Served from the docs themselves (mermaid 10.9.3, MIT), never a CDN:
    // opening a page must not hand the reader's address to a third party.
    script.src = (typeof path_to_root === "string" ? path_to_root : "") + "assets/mermaid.min.js";
    script.onload = callback;
    script.onerror = function () {
      console.warn("Krate docs could not load Mermaid diagrams.");
    };
    document.head.appendChild(script);
  }

  function renderMermaidBlocks() {
    var blocks = document.querySelectorAll("pre code.language-mermaid");

    blocks.forEach(function (codeBlock) {
      var container = document.createElement("div");
      container.className = "mermaid";
      container.textContent = codeBlock.textContent;
      codeBlock.parentNode.replaceWith(container);
    });

    if (blocks.length === 0 || !window.mermaid) {
      return;
    }

    window.mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "default"
    });
    window.mermaid.run({ querySelector: ".mermaid" });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      loadMermaid(renderMermaidBlocks);
    });
  } else {
    loadMermaid(renderMermaidBlocks);
  }
})();
