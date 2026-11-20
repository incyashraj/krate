// The one look every system email from the hub has: the sign-in link, team
// invitations and notices, "your app is live" and the rest. The founder's
// own notes (the welcome and announcements, mail-announce.js) keep their
// typed-letter look on purpose and do not use this.
//
// It follows krate.tech: the same mark as the site's header
// (/krate-mark-3d-96.png), Geist first, the site's light tokens (--bg,
// --ink, --ink-2, --line, --accent) and its primary button, a black pill
// with white words. Under prefers-color-scheme: dark it takes the site's
// dark tokens, and the button turns light, as on the site.
//
// Email-client rules it keeps:
//   - tables for layout, every style inline; the one <style> block only
//     adds dark mode and the phone padding, and the mail reads the same
//     without it (Gmail on the web drops the dark half),
//   - no web font to load: Geist when the reader has it, then the system's,
//     and Arial for Outlook on Windows, which cannot walk a font list,
//   - a 560px column (a fixed table for Outlook, max-width for the rest),
//   - the button is a table cell with bgcolor as well as a CSS background,
//     which is what Outlook paints, and the link is also written out under
//     it, so the mail works with styles, images or buttons stripped,
//   - the logo is a hosted https image with its size and alt text,
//   - colours that stay readable when Gmail or Outlook invert a light mail
//     on their own: dark words on white, a dark button with white words.
//
// Every value put in is text, never markup: names, reasons and notes are
// typed by people and are escaped here.

export const SITE = "https://krate.tech";
export const LOGO_URL = `${SITE}/krate-mark-3d-96.png`;

const FONT = "Geist,-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif";
const MONO = "'Geist Mono',SFMono-Regular,Menlo,Consolas,'Liberation Mono',monospace";

// The site's light tokens (docs/landing/kit/site.css).
const L = {
  bg: "#f5f5f7", // --soft: the page around the card
  card: "#ffffff", // --s1
  line: "#ececee", // --line
  ink: "#1c1c1e", // --ink
  ink2: "#52525b", // --ink-2: body words
  mute: "#6b6b72", // --mute: small print that must still read
  soft: "#f5f5f7", // --s2: the link box
  accent: "#3d6df0", // --accent
  btn: "#0d0d0d", // --btn
  btnInk: "#ffffff", // --btn-ink
};

export const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

/// One paragraph's words, with any https link in them made a link.
function linked(s) {
  return esc(s).replace(
    /https?:\/\/[^\s<]*[^\s<.,;:!?)\]'"]/g,
    (u) => `<a href="${u}" class="k-a" style="color:${L.accent};text-decoration:underline;">${u}</a>`,
  );
}

/// One block of the body. A string is a paragraph; `{quote}` is words a
/// person typed (a request, a reason, a note), set apart; `{rows}` is a
/// short list of `[label, value]`.
function block(b) {
  if (typeof b === "string") {
    return `<p class="k-ink2" style="margin:0 0 16px 0;font:400 15px/1.6 ${FONT};color:${L.ink2};">${linked(b).replace(/\n/g, "<br>")}</p>`;
  }
  if (b && b.quote != null) {
    return `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 16px 0;"><tr>
<td class="k-quote" style="border-left:3px solid ${L.line};padding:2px 0 2px 14px;font:400 15px/1.6 ${FONT};color:${L.ink};">${linked(b.quote).replace(/\n/g, "<br>")}</td>
</tr></table>`;
  }
  if (b && Array.isArray(b.rows)) {
    const rows = b.rows
      .map(
        ([k, v]) => `<tr>
<td valign="top" class="k-mute" style="padding:0 16px 6px 0;white-space:nowrap;font:400 14px/1.5 ${FONT};color:${L.mute};">${esc(k)}</td>
<td valign="top" class="k-ink" style="padding:0 0 6px 0;font:400 14px/1.5 ${FONT};color:${L.ink};word-break:break-word;">${esc(v)}</td>
</tr>`,
      )
      .join("\n");
    return `<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 16px 0;">${rows}</table>`;
  }
  return "";
}

/// The mail's HTML. `heading`: the one line it is about. `blocks`: the
/// words (see `block`). `button`: `{label, url}`, the one thing to press,
/// with the link written out under it (`written: false` leaves that out, for
/// a link too long to read, such as a prefilled Gmail compose window; the
/// text part still carries it). `after`: blocks below the button.
/// `note`: small print under a hairline (expiry, "if you did not ask").
/// `footer`: lines under the card (why they got it); `unsubscribe`: the
/// one-click link on a mail nobody asked for. `preheader`: the grey line a
/// mail app shows beside the subject.
export function emailHtml({ title, preheader, heading, blocks = [], button, after = [], note = [], footer = [], unsubscribe }) {
  const notes = (Array.isArray(note) ? note : [note]).filter(Boolean);
  const feet = (Array.isArray(footer) ? footer : [footer]).filter(Boolean);
  const href = button ? esc(button.url) : "";
  const buttonHtml = button
    ? `<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:8px 0 0 0;"><tr>
<td align="center" bgcolor="${L.btn}" class="k-btn" style="border-radius:999px;background:${L.btn};">
<a href="${href}" class="k-btn-a" style="display:inline-block;padding:13px 24px;border:1px solid ${L.btn};border-radius:999px;background:${L.btn};color:${L.btnInk};font:500 15px/18px ${FONT};letter-spacing:-0.01em;text-decoration:none;">${esc(button.label)}</a>
</td></tr></table>${
        button.written === false
          ? ""
          : `
<p class="k-mute" style="margin:24px 0 6px 0;font:400 13px/1.5 ${FONT};color:${L.mute};">Or copy this link into your browser:</p>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 4px 0;"><tr>
<td class="k-box" bgcolor="${L.soft}" style="padding:10px 12px;border-radius:10px;background:${L.soft};font:400 13px/1.5 ${MONO};word-break:break-all;"><a href="${href}" class="k-a" style="color:${L.accent};text-decoration:none;word-break:break-all;">${esc(button.url)}</a></td>
</tr></table>`
      }`
    : "";
  const afterHtml = after.length ? `<div style="height:20px;line-height:20px;font-size:0;">&#8203;</div>${after.map(block).join("\n")}` : "";
  const noteHtml = notes.length
    ? `<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="margin:24px 0 0 0;"><tr><td class="k-rule" style="border-top:1px solid ${L.line};font-size:0;line-height:0;height:1px;">&#8203;</td></tr></table>
${notes.map((n) => `<p class="k-mute" style="margin:16px 0 0 0;font:400 13px/1.55 ${FONT};color:${L.mute};">${linked(n)}</p>`).join("\n")}`
    : "";
  const footHtml = [
    ...feet.map((f) => `<p class="k-mute" style="margin:0 0 8px 0;font:400 12.5px/1.6 ${FONT};color:${L.mute};">${linked(f)}</p>`),
    `<p class="k-mute" style="margin:0 0 8px 0;font:400 12.5px/1.6 ${FONT};color:${L.mute};">Krate &middot; <a href="${SITE}" class="k-mute" style="color:${L.mute};text-decoration:underline;">krate.tech</a></p>`,
    unsubscribe
      ? `<p class="k-mute" style="margin:0;font:400 12.5px/1.6 ${FONT};color:${L.mute};">Don't want these emails? <a href="${esc(unsubscribe)}" class="k-mute" style="color:${L.mute};text-decoration:underline;">Unsubscribe</a></p>`
      : "",
  ].join("\n");
  return `<!doctype html>
<html lang="en" xmlns="http://www.w3.org/1999/xhtml">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="x-apple-disable-message-reformatting">
<meta name="color-scheme" content="light dark">
<meta name="supported-color-schemes" content="light dark">
<title>${esc(title || heading || "Krate")}</title>
<!--[if mso]><style>body,table,td,p,a,span,h1{font-family:Arial,Helvetica,sans-serif !important;}</style><![endif]-->
<style>
:root{color-scheme:light dark;}
@media (max-width:600px){.k-pad{padding:28px 22px 24px 22px !important;}.k-outer{padding:16px 10px !important;}}
@media (prefers-color-scheme:dark){
.k-page{background:#08080a !important;}
.k-card{background:#18181b !important;border-color:#232327 !important;}
.k-ink,.k-h{color:#f4f4f5 !important;}
.k-ink2{color:#a1a1aa !important;}
.k-quote{color:#f4f4f5 !important;border-left-color:#2c2c31 !important;}
.k-mute{color:#8e8e96 !important;}
.k-a{color:#6f94ff !important;}
.k-box{background:#202024 !important;}
.k-rule{border-top-color:#232327 !important;}
.k-btn{background:#f4f4f5 !important;}
.k-btn-a{background:#f4f4f5 !important;border-color:#f4f4f5 !important;color:#08080a !important;}
}
[data-ogsc] .k-btn-a{color:#ffffff !important;}
</style>
</head>
<body class="k-page" style="margin:0;padding:0;background:${L.bg};-webkit-text-size-adjust:100%;">
<div style="display:none;max-height:0;max-width:0;overflow:hidden;opacity:0;mso-hide:all;">${esc(preheader || heading || "")}&#8203;&nbsp;&#8203;&nbsp;&#8203;&nbsp;&#8203;&nbsp;&#8203;&nbsp;&#8203;&nbsp;</div>
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" class="k-page" bgcolor="${L.bg}" style="background:${L.bg};">
<tr><td align="center" class="k-outer" style="padding:32px 16px;">
<!--[if mso]><table role="presentation" width="560" align="center" cellpadding="0" cellspacing="0" border="0"><tr><td><![endif]-->
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:560px;">
<tr><td class="k-card k-pad" bgcolor="${L.card}" style="background:${L.card};border:1px solid ${L.line};border-radius:16px;padding:36px 40px 32px 40px;text-align:left;">
<table role="presentation" cellpadding="0" cellspacing="0" border="0" style="margin:0 0 28px 0;"><tr>
<td valign="middle" style="padding:0 10px 0 0;"><a href="${SITE}" style="text-decoration:none;"><img src="${LOGO_URL}" width="28" height="28" alt="Krate" style="display:block;width:28px;height:28px;border:0;outline:none;text-decoration:none;"></a></td>
<td valign="middle" class="k-ink" style="font:600 17px/1 ${FONT};letter-spacing:-0.035em;color:${L.ink};">Krate</td>
</tr></table>
${heading ? `<h1 class="k-h" style="margin:0 0 14px 0;font:600 24px/1.25 ${FONT};letter-spacing:-0.025em;color:${L.ink};">${esc(heading)}</h1>` : ""}
${blocks.map(block).join("\n")}
${buttonHtml}
${afterHtml}
${noteHtml}
</td></tr>
<tr><td style="padding:20px 8px 0 8px;text-align:left;">
${footHtml}
</td></tr>
</table>
<!--[if mso]></td></tr></table><![endif]-->
</td></tr>
</table>
</body>
</html>`;
}
