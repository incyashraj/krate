// A .krate file shared to the installed Player (Android's share sheet, the
// manifest's share_target): the browser hands it over as a form POSTed to
// /run/file. This worker takes that POST in the browser, so the file never
// reaches the network: it keeps the file in this origin's cache for the page
// to read once, and sends the tab to /run/file?shared=<id>. Its scope is
// /run/file and it answers nothing else: every other request goes on to the
// network untouched, as if it were not here.
const SHARED = 'krate-file-share';

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));
self.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== 'POST' || url.pathname !== '/run/file') return;
  e.respondWith((async () => {
    let to = '/run/file?share=empty';
    try {
      const form = await e.request.formData();
      const file = form.getAll('file').find((f) => f && typeof f === 'object' && typeof f.size === 'number');
      if (file) {
        const id = crypto.randomUUID();
        const cache = await caches.open(SHARED);
        await cache.put(new Request(`/run/file/shared/${id}`), new Response(file, {
          headers: { 'x-krate-name': encodeURIComponent(file.name || 'app.krate') },
        }));
        to = `/run/file?shared=${id}`;
      }
    } catch (_) { /* the page says the file did not arrive */ }
    return Response.redirect(new URL(to, self.location.origin).href, 303);
  })());
});
