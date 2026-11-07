// Send the founder's welcome -- the exact note the hub sends a new person,
// by the same route -- to one address, to see where it lands.
//
// Through Gmail itself (what the hub does once GMAIL_APP_PASSWORD is set):
//   GMAIL_APP_PASSWORD='abcd efgh ijkl mnop' node --experimental-wasm-modules cloud/worker/scripts/send-welcome-test.mjs you@example.com "Your Name"
// Through Resend (the hub's fallback):
//   RESEND_API_KEY=re_... node --experimental-wasm-modules cloud/worker/scripts/send-welcome-test.mjs you@example.com "Your Name"
import tls from "node:tls";
import { Readable, Writable } from "node:stream";
import { sendWelcome } from "../src/index.js";

const [to, name = ""] = process.argv.slice(2);
const env = {
  GMAIL_APP_PASSWORD: process.env.GMAIL_APP_PASSWORD,
  RESEND_API_KEY: process.env.RESEND_API_KEY,
  // The welcome carries an unsubscribe link, which needs the hub's secret;
  // without it the link is made with a stand-in and will not work.
  KEY_WRAP_SECRET: process.env.KEY_WRAP_SECRET || "send-welcome-test-only",
  PUBLIC_BASE: "https://hub.krate.tech",
  // cloudflare:sockets' connect, from Node's TLS: the hub's own code path.
  SMTP_CONNECT: ({ hostname, port }) => {
    const socket = tls.connect({ host: hostname, port, servername: hostname });
    return {
      readable: Readable.toWeb(socket),
      writable: Writable.toWeb(socket),
      close: async () => socket.end(),
    };
  },
};
if (!to || (!env.GMAIL_APP_PASSWORD && !env.RESEND_API_KEY)) {
  console.error("usage: GMAIL_APP_PASSWORD='...' (or RESEND_API_KEY=re_...) node --experimental-wasm-modules cloud/worker/scripts/send-welcome-test.mjs you@example.com \"Your Name\"");
  process.exit(2);
}
const failed = await sendWelcome(env, { id: "send-welcome-test", email: to, name });
console.log(failed ? `not sent: ${failed}` : `sent to ${to} via ${env.GMAIL_APP_PASSWORD ? "Gmail" : "Resend"}`);
process.exit(failed ? 1 : 0);
