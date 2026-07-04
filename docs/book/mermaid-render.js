(function () {
  function loadMermaid(callback) {
    if (window.mermaid) {
      callback();
      return;
    }

    var script = document.createElement("script");
    script.src = "https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.min.js";
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
