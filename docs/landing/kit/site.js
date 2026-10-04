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
