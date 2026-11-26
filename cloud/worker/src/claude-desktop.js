// Krate for Claude Desktop (IC-1005): the one file that adds Krate to
// Claude Desktop, and the stable addresses it is handed out at.
//
// A .mcpb is a Claude Desktop extension: the person opens the file and
// clicks Install. There is no link that installs one from a URL, so the
// smart delivery is a download link Claude itself hands over -- from the
// connector's own tool (mcp.js) and from krate.tech/claude/.
//
// The files are release assets (release.yml builds them from that
// release's own signed binaries): krate-<version>-mac.mcpb, one universal
// binary for Apple silicon and Intel, and krate-<version>-windows.mcpb.
// The addresses here never change between releases:
//
//   hub.krate.tech/claude/krate-mac.mcpb      -> the latest release's file
//   hub.krate.tech/claude/krate-windows.mcpb
//
// and krate.tech/claude/krate-<os>.mcpb redirects to these. Until a release
// carries the files, the address says plainly that it is coming, never 404.

/** The page that explains it, with the button. */
export const CLAUDE_PAGE = "https://krate.tech/claude/";

/** The two systems Claude Desktop runs on, as a person names them. */
export const DESKTOP_SYSTEMS = {
  mac: {
    label: "Mac",
    file: "krate-mac.mcpb",
    asset: /^krate-\d[\w.-]*-mac\.mcpb$/,
    // What `krate setup rust` fetches once, summed from the archives
    // crates/cli/src/rust_toolchain.rs pins (scripts/build-mcpb.py's
    // builder_words reads the same numbers): 126.5 MB on Apple silicon,
    // 140.7 MB on Intel.
    builder: "about 127-141 MB",
  },
  windows: {
    label: "Windows PC",
    file: "krate-windows.mcpb",
    asset: /^krate-\d[\w.-]*-windows\.mcpb$/,
    // Windows takes the .tar.gz archives (every Windows tar reads them):
    // 225.6 MB.
    builder: "about 226 MB",
  },
};

/** The stable download address for one system. */
export const downloadUrl = (os) => `https://krate.tech/claude/${DESKTOP_SYSTEMS[os].file}`;

/** The two steps, as the person will see them. */
export const STEPS = [
  "Open the file you downloaded (double-click it, or drag it onto Claude Desktop).",
  "Claude Desktop shows Krate with an Install button. Click Install.",
];

/** The release asset for `os` in a /release/latest answer, or null. */
export function assetFor(release, os) {
  const system = DESKTOP_SYSTEMS[os];
  if (!system || !release || !Array.isArray(release.assets)) return null;
  return release.assets.find((a) => a && system.asset.test(String(a.name || ""))) || null;
}

/** The system a /claude/<file> path names, or null. */
export function systemOfPath(pathname) {
  for (const [os, system] of Object.entries(DESKTOP_SYSTEMS)) {
    if (pathname === `/claude/${system.file}`) return os;
  }
  return null;
}

/**
 * GET /claude/krate-<os>.mcpb on the hub: a redirect to the latest
 * release's file, or, before any release carries it, a page that says so.
 * `release` is the /release/latest answer (null when GitHub could not be
 * read); `page(title, body, status)` is the hub's own page shell.
 */
export function downloadResponse(os, release, page) {
  const hit = assetFor(release, os);
  if (hit && /^https:\/\/github\.com\//.test(String(hit.browser_download_url || ""))) {
    return new Response(null, {
      status: 302,
      headers: {
        location: hit.browser_download_url,
        // Short: the next release moves it.
        "cache-control": "public, max-age=300",
        "access-control-allow-origin": "*",
      },
    });
  }
  const system = DESKTOP_SYSTEMS[os];
  const words = release
    ? `Krate for Claude Desktop on a ${system.label} comes with the next Krate release. This address will download it then.`
    : "The download could not be looked up just now. Try again in a minute.";
  const res = page(
    "Krate for Claude Desktop",
    `<h1>Krate for Claude Desktop</h1><p>${words}</p>` +
      `<p><a href="${CLAUDE_PAGE}">What it does and how to add it</a></p>`,
    release ? 200 : 503,
  );
  res.headers.set("cache-control", "public, max-age=300");
  return res;
}

/**
 * What the connector's get_krate_for_claude_desktop answers: the download
 * for the person's system (both when it is not known), what it gives, the
 * two steps, and the one-time builder download, said truthfully.
 */
export function desktopOffer(os, release) {
  const systems = os ? [os] : ["mac", "windows"];
  const downloads = systems.map((s) => {
    const hit = assetFor(release, s);
    return {
      os: s,
      label: DESKTOP_SYSTEMS[s].label,
      url: downloadUrl(s),
      available: Boolean(hit),
      size_bytes: hit && Number.isFinite(Number(hit.size)) ? Number(hit.size) : null,
      builder_download: DESKTOP_SYSTEMS[s].builder,
    };
  });
  const ready = downloads.filter((d) => d.available);
  const gives =
    "Krate for Claude Desktop lets Claude make an app and open it right on this computer, in its own window, " +
    "with no other download, no terminal and no settings.";
  let words;
  if (!ready.length) {
    words =
      `${gives} It comes with the next Krate release; ${CLAUDE_PAGE} will have the download then. ` +
      "Until then the person can open any app Krate makes from its browser link.";
  } else {
    const lines = ready.map((d) => `- ${d.label}: ${d.url}`);
    words =
      `${gives}\n\n${os ? "Download" : "Download the one for this computer"}:\n${lines.join("\n")}\n\n` +
      `Then:\n1. ${STEPS[0]}\n2. ${STEPS[1]}\n\n` +
      `The first app it builds sets up Krate's builder, a one-time download of ${ready.map((d) => d.builder_download).join(" on a Mac, ")}` +
      `${ready.length > 1 ? " on Windows" : ""}. More: ${CLAUDE_PAGE}`;
    if (!os) words += "\n\nIf you know which computer the person uses, give only that link.";
  }
  return {
    content: [{ type: "text", text: words }],
    structuredContent: { downloads, steps: STEPS, page: CLAUDE_PAGE, message: words },
  };
}

/** The one line a finished app's answer carries, or "" when no release has the file. */
export function openHereLine(release) {
  const any = ["mac", "windows"].some((os) => assetFor(release, os));
  if (!any) return "";
  return `To open it in its own window on this computer, add Krate to Claude Desktop once: ${CLAUDE_PAGE}`;
}
