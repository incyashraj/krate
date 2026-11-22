/* Krate website, new design: the behaviour every page shares.
   Header drawers, the phone menu, light/dark, arrival on scroll, and the
   table of contents that follows the reader. No page logic lives here. */
(() => {
  const $ = (s, r = document) => r.querySelector(s), $$ = (s, r = document) => [...r.querySelectorAll(s)];
  // header drawers: one panel that moves to the item under the pointer
  const menu = $('#menu'), drawer = $('#drawer');
  if (menu && drawer) {
    const sets = $$('.set', drawer); let closeT = null;
    const close = () => { drawer.classList.remove('on'); $$('.mi', menu).forEach(m => m.classList.remove('open')); };
    const open = item => {
      clearTimeout(closeT); const key = item.dataset.set;
      $$('.mi', menu).forEach(m => m.classList.toggle('open', m === item && !!key));
      if (!key) { close(); return; }
      const set = sets.find(s => s.dataset.set === key); sets.forEach(s => s.classList.toggle('on', s === set));
      const w = Math.max(200, set.scrollWidth + 16), h = set.scrollHeight + 20, left = item.offsetLeft - 8;
      if (!drawer.classList.contains('on')) { drawer.style.transition = 'none'; Object.assign(drawer.style, { left: left + 'px', width: w + 'px', height: h + 'px' }); void drawer.offsetWidth; drawer.style.transition = ''; }
      Object.assign(drawer.style, { left: left + 'px', width: w + 'px', height: h + 'px' }); drawer.classList.add('on');
    };
    $$('.mi', menu).forEach(m => { m.addEventListener('mouseenter', () => open(m)); m.addEventListener('focus', () => open(m)); });
    menu.addEventListener('mouseleave', () => { closeT = setTimeout(close, 120); });
    drawer.addEventListener('mouseenter', () => clearTimeout(closeT));
    addEventListener('keydown', e => { if (e.key === 'Escape') close(); });
  }
  // the phone menu
  const burger = $('#burger'), mnav = $('#mnav');
  if (burger && mnav) burger.addEventListener('click', () => { const on = mnav.classList.toggle('on'); burger.setAttribute('aria-expanded', on); document.body.style.overflow = on ? 'hidden' : ''; });
  // light / dark, remembered
  const thm = $('#thm');
  if (thm) thm.addEventListener('click', () => { const t = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'; document.documentElement.dataset.theme = t; try { localStorage.setItem('krate-theme', t); } catch (e) {} });
  // signed in on this site (the token /login/done keeps): the header's
  // "Sign in" becomes "Account", in the bar and in the phone menu
  try {
    if (localStorage.getItem('krate_tok')) $$('a.signin[href="/login/"], .mnav a[href="/login/"]').forEach(a => { a.textContent = 'Account'; a.setAttribute('href', '/account/'); });
  } catch (e) {}
  // a hairline under the header once the page moves
  const hd = $('.hd'); const onScroll = () => hd && hd.classList.toggle('scrolled', scrollY > 4); addEventListener('scroll', onScroll, { passive: true }); onScroll();
  // things arrive as they come into view
  const io = 'IntersectionObserver' in window ? new IntersectionObserver(es => es.forEach(e => { if (e.isIntersecting) { e.target.classList.add('in'); io.unobserve(e.target); } }), { rootMargin: '0px 0px -8% 0px' }) : null;
  $$('.rv').forEach(el => io ? io.observe(el) : el.classList.add('in'));
  // the table of contents marks where the reader is
  const toc = $('.toc'); if (toc && 'IntersectionObserver' in window) {
    const links = $$('a[href^="#"]', toc), targets = links.map(a => document.getElementById(a.getAttribute('href').slice(1))).filter(Boolean);
    const io2 = new IntersectionObserver(es => es.forEach(e => { if (e.isIntersecting) links.forEach(a => a.classList.toggle('on', a.getAttribute('href') === '#' + e.target.id)); }), { rootMargin: '-20% 0px -70% 0px' });
    targets.forEach(t => io2.observe(t));
  }
})();

/* Tools an AI agent in the browser can use on any page (WebMCP, IC-997):
   search the public Krate gallery, open an app from its link, and read the
   facts written for AI readers. Registered only where the browser offers
   document.modelContext (or the older navigator.modelContext); every other
   visitor runs none of this. Each tool does what its description says and
   nothing else: no sign-in, no writes. */
(() => {
  const mc = (typeof document !== 'undefined' && document.modelContext) || (typeof navigator !== 'undefined' && navigator.modelContext);
  if (!mc || typeof mc.registerTool !== 'function') return;
  const HUB = 'https://hub.krate.tech';
  const text = (t) => ({ content: [{ type: 'text', text: t }] });
  const tools = [
    {
      name: 'search_krate_apps',
      description: 'Search the public Krate gallery for apps by what they do. Each result has a link that opens the app in any browser, phones included, and the same .krate file runs natively on macOS, Windows and Linux.',
      inputSchema: { type: 'object', properties: { query: { type: 'string', description: 'What the app should do, in plain words, e.g. "notes" or "label printer"' } }, required: ['query'] },
      annotations: { readOnlyHint: true },
      async execute({ query }) {
        const r = await fetch(`${HUB}/apps?q=${encodeURIComponent(String(query || '').slice(0, 200))}`);
        if (!r.ok) return text('The Krate gallery did not answer just now. Try again in a moment.');
        const apps = ((await r.json()).apps || []).slice(0, 10);
        if (!apps.length) return text(`No public Krate app matches "${query}". A person can make one at https://krate.tech/app/ or by asking Claude with the Krate connector (https://hub.krate.tech/mcp).`);
        return text(apps.map((a) => `${a.meta.name}${a.meta.description ? ' -- ' + a.meta.description : ''}\n  ${a.url}`).join('\n'));
      },
    },
    {
      name: 'open_krate_app',
      description: 'Open a Krate app in this tab from its hub.krate.tech link, as found by search_krate_apps.',
      inputSchema: { type: 'object', properties: { link: { type: 'string', description: 'The app link, https://hub.krate.tech/a/...' } }, required: ['link'] },
      async execute({ link }) {
        const ok = /^https:\/\/hub\.krate\.tech\/(a|run)\/[A-Za-z0-9]+$/.test(String(link || ''));
        if (!ok) return text('That is not a Krate app link. App links look like https://hub.krate.tech/a/v1a2b3c4d5e.');
        location.assign(String(link).replace('/a/', '/run/'));
        return text(`Opening ${link}.`);
      },
    },
    {
      name: 'get_krate_facts',
      description: 'What Krate is, when to choose it, how it compares with Electron, Tauri and web apps, and its recorded measurements, written for AI readers.',
      inputSchema: { type: 'object', properties: {} },
      annotations: { readOnlyHint: true },
      async execute() {
        const r = await fetch('/llms.txt');
        return text(r.ok ? await r.text() : 'See https://krate.tech/llms.txt');
      },
    },
  ];
  for (const tool of tools) {
    try { mc.registerTool(tool); } catch (e) { /* an older or stricter browser: the page works as before */ }
  }
})();
