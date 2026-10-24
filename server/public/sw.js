// Oyen Poni phone page service worker.
// Keeps the page opening without a connection (network first, cache as the
// fallback; /api/ is never cached) and receives Android's share sheet:
// POST /share (multipart/form-data, field "files") goes into IndexedDB
// ("oyen-drop" / "shared"), then the page opens at /?share=<id>.

const CACHE = 'oyen-drop-shell-v2';
const SHELL = [
  '/', '/app.js', '/app.css', '/drop-core.js', '/island.js', '/manifest.webmanifest', '/icon-192.png', '/icon-512.png', '/apple-touch-icon.png',
  '/oyen-peek.webp', '/oyen-sit.webp', '/oyen-asleep.webp', '/oyen-shelf.webp', '/oyen-note.webp',
  '/outfit-400.woff2', '/outfit-500.woff2', '/outfit-600.woff2', '/outfit-700.woff2',
];
const DAY = 24 * 3600 * 1000;

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE)
      .then((c) => c.addAll(SHELL))
      .catch(() => {})
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    for (const key of await caches.keys()) if (key !== CACHE) await caches.delete(key);
    await self.clients.claim();
  })());
});

self.addEventListener('fetch', (event) => {
  const req = event.request;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  if (req.method === 'POST' && url.pathname === '/share') {
    event.respondWith(receiveShare(req));
    return;
  }
  if (req.method !== 'GET' || url.pathname.startsWith('/api/')) return;
  event.respondWith(networkFirst(req, url));
});

async function networkFirst(req, url) {
  const navigate = req.mode === 'navigate';
  const key = navigate ? '/' : url.pathname;
  try {
    const res = await fetch(req);
    if (res.ok && res.type === 'basic' && (navigate || SHELL.includes(url.pathname))) {
      const copy = res.clone();
      caches.open(CACHE).then((c) => c.put(key, copy)).catch(() => {});
    }
    return res;
  } catch (err) {
    const hit = await caches.match(key, { ignoreSearch: true });
    if (hit) return hit;
    throw err;
  }
}

function go(path) {
  return Response.redirect(new URL(path, self.location.origin).href, 303);
}

async function receiveShare(req) {
  try {
    const form = await req.formData();
    const files = form.getAll('files').filter((f) => f && typeof f === 'object' && typeof f.size === 'number');
    const field = (name) => String(form.get(name) || '').trim().slice(0, 4000);
    const id = randomId();
    await putShared(id, { at: Date.now(), files, title: field('title'), text: field('text'), url: field('url') });
    return go(`/?share=${id}`);
  } catch {
    return go('/?shared=lost');
  }
}

function randomId() {
  const b = crypto.getRandomValues(new Uint8Array(12));
  return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}

function openDb() {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open('oyen-drop', 1);
    r.onupgradeneeded = () => {
      if (!r.result.objectStoreNames.contains('shared')) r.result.createObjectStore('shared');
    };
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

async function putShared(id, value) {
  const db = await openDb();
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction('shared', 'readwrite');
      const store = tx.objectStore('shared');
      store.put(value, id);
      // shares nobody sent within a day are dropped
      const all = store.openCursor();
      all.onsuccess = () => {
        const c = all.result;
        if (!c) return;
        if (c.key !== id && (!c.value || !(Date.now() - c.value.at < DAY))) c.delete();
        c.continue();
      };
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}
