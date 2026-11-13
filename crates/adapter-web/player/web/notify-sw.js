// What shows a Krate app's notifications from a tab (IC-974). A phone's
// browser shows a notification only through a service worker (Chrome on
// Android has no other way; an iPhone only for an app on the Home Screen),
// so the Player uses one everywhere, and the same code runs on every
// browser. It serves nothing: its scope (/player/notify/) holds no page, and
// it has no fetch handler. A tap on a notification brings the app's tab
// forward; the app itself learns nothing about it, as on desktop.
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()));
self.addEventListener('notificationclick', (e) => {
  e.notification.close();
  const from = e.notification.data && e.notification.data.page;
  e.waitUntil(self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then((tabs) => {
    const tab = tabs.find((t) => t.url === from) || tabs.find((t) => new URL(t.url).pathname.startsWith('/run/'));
    if (tab) return tab.focus();
    return from ? self.clients.openWindow(from) : undefined;
  }));
});
