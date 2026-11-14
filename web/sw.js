self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', event => event.waitUntil(self.clients.claim()));
// Game data exists only in the user's browser. Never send missing game files to the server.
self.addEventListener('fetch', event => {
  if (event.request.method !== 'GET' && event.request.method !== 'HEAD') return;
  const url = new URL(event.request.url);
  const gameBase = new URL('game/', self.registration.scope);
  if (url.origin !== gameBase.origin || !url.pathname.startsWith(gameBase.pathname)) return;
  event.respondWith(caches.open('mc2-local-game-v1').then(async cache =>
    event.request.method === 'HEAD' ? new Response(null, {status: 404}) :
    await cache.match(event.request.url) || new Response('Game file not imported', {status: 404})
  ));
});
