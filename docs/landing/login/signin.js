/* The proof that THIS browser started a sign-in (K-909).
 *
 * A finished sign-in is a session handed to /login/done in the URL
 * fragment. Nothing used to tie it to the browser that asked, so a link
 * carrying somebody else's finished sign-in -- an attacker's own, started
 * and completed on their own machine -- signed the visitor into the
 * attacker's account, and whatever they then made landed where the
 * attacker could read it.
 *
 * So every door that starts a sign-in (the /login page, the /make sheet)
 * asks for a nonce here first. It is kept in this origin's storage, sent to
 * the hub, carried through GitHub, Google or the email link untouched, and
 * handed back beside the session. /login/done takes the session only when
 * the nonce is one this browser is holding, and uses it up.
 *
 * localStorage rather than sessionStorage because an email link opens in a
 * new tab. Another site cannot write here, which is the whole point.
 */
(function (root) {
  var KEY = "krate_signin";
  // Long enough to read an email and click the link (the link itself
  // expires in 15 minutes), short enough that a stale one is not a key.
  var MAX_AGE_MS = 60 * 60 * 1000;
  // A few at once: two tabs, or a page loaded twice.
  var KEEP = 5;

  function read() {
    try {
      var list = JSON.parse(root.localStorage.getItem(KEY) || "[]");
      var now = Date.now();
      return Array.isArray(list)
        ? list.filter(function (e) { return e && typeof e.n === "string" && now - e.at < MAX_AGE_MS; })
        : [];
    } catch (e) {
      return [];
    }
  }

  function write(list) {
    try { root.localStorage.setItem(KEY, JSON.stringify(list.slice(-KEEP))); } catch (e) {}
  }

  function fresh() {
    var bytes = new Uint8Array(16);
    root.crypto.getRandomValues(bytes);
    var out = "";
    for (var i = 0; i < bytes.length; i++) out += (bytes[i] + 256).toString(16).slice(1);
    return out;
  }

  /// Start a sign-in: a new nonce, remembered here.
  function begin() {
    var n = fresh();
    var list = read();
    list.push({ n: n, at: Date.now() });
    write(list);
    return n;
  }

  /// Did this browser start the sign-in that came back with `n`? Yes at
  /// most once: a match is used up, so the same link cannot be replayed.
  function accept(n) {
    if (!n || !/^[A-Za-z0-9_-]{16,64}$/.test(n)) return false;
    var list = read();
    var at = -1;
    for (var i = 0; i < list.length; i++) if (list[i].n === n) at = i;
    if (at < 0) return false;
    list.splice(at, 1);
    write(list);
    return true;
  }

  root.KrateSignIn = { begin: begin, accept: accept };
})(typeof window !== "undefined" ? window : globalThis);
