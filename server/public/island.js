// The island at the top of the page. Like Oyen Poni's island on the PC, it
// says what is happening right now and grows to fit: a green dot when idle,
// the road to the PC once connected, a progress ring while sending, a check
// when the files are on the Shelf. Short news (done, oops) shows for a moment
// and the island goes back to what it was showing before.

const $ = (id) => document.getElementById(id);

const PATHS = {
  wifi: '<path d="M4.5 9.6a11 11 0 0 1 15 0"/><path d="M7.4 12.9a6.6 6.6 0 0 1 9.2 0"/><path d="M10.3 16.1a2.4 2.4 0 0 1 3.4 0"/><path d="M12 19h.01"/>',
  globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17"/><path d="M12 3.5c2.3 2.4 3.5 5.2 3.5 8.5s-1.2 6.1-3.5 8.5c-2.3-2.4-3.5-5.2-3.5-8.5s1.2-6.1 3.5-8.5z"/>',
  lock: '<rect x="5.5" y="10.5" width="13" height="9.5" rx="2"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/>',
  check: '<path d="M6 12.5l4 4L18 8"/>',
  alert: '<path d="M12 7.5v6"/><path d="M12 16.8h.01"/>',
  photo: '<path d="M4 7.5A1.5 1.5 0 0 1 5.5 6h13A1.5 1.5 0 0 1 20 7.5v9a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 16.5z"/><path d="M4 15l4.5-4.5L13 15l2.5-2.5L20 17"/>',
  file: '<path d="M7.5 3H14l5 5v11.5a1.5 1.5 0 0 1-1.5 1.5h-10A1.5 1.5 0 0 1 6 19.5v-15A1.5 1.5 0 0 1 7.5 3z"/><path d="M14 3v5h5"/>',
  moon: '<path d="M19 14.5A7.5 7.5 0 0 1 9.5 5a7.5 7.5 0 1 0 9.5 9.5z"/>',
};

function icon(name) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = PATHS[name] || '';
  return s;
}

let base = { mode: 'idle', title: 'Oyen Poni' };
let shown = null;
let backTimer = null;
let thumbUrl = null;

/**
 * Show a state. `hold` (ms) makes it short news that gives way to the last
 * lasting state; without it, this becomes the lasting state.
 *   mode: idle | wait | link | off | busy | done | oops
 *   o: { title, sub, icon, progress (0..1), thumb (File), live (bool), hold }
 */
export function setIsland(mode, o = {}) {
  const s = { mode, title: o.title || 'Oyen Poni', sub: o.sub || '', icon: o.icon || null, progress: o.progress ?? null, thumb: o.thumb || null, live: Boolean(o.live) };
  if (o.hold) {
    clearTimeout(backTimer);
    render(s);
    backTimer = setTimeout(() => {
      backTimer = null;
      render(base);
    }, o.hold);
    return;
  }
  base = s;
  if (!backTimer) render(s);
}

/** Only the progress of a send: no re-layout, so it can run every frame. */
export function islandProgress(progress, sub) {
  if (base.mode !== 'busy') return;
  base.progress = progress;
  base.sub = sub;
  if (shown !== base) return;
  const ring = document.querySelector('#isl-trail .ring');
  if (ring) ring.style.setProperty('--p', String(Math.max(0, Math.min(1, progress))));
  $('isl-sub').textContent = sub;
}

function render(s) {
  const isl = $('island');
  if (!isl) return;
  const sameShape = shown && shown.mode === s.mode && shown.title === s.title && shown.icon === s.icon && shown.thumb === s.thumb;
  shown = s;
  isl.dataset.mode = s.mode;
  const lead = $('isl-lead');
  const trail = $('isl-trail');
  if (!sameShape) {
    lead.textContent = '';
    trail.textContent = '';
    if (thumbUrl) {
      URL.revokeObjectURL(thumbUrl);
      thumbUrl = null;
    }
    if (s.mode === 'idle') {
      lead.appendChild(Object.assign(document.createElement('span'), { className: 'live' }));
    } else if (s.mode === 'wait') {
      lead.appendChild(Object.assign(document.createElement('span'), { className: 'isl-spin' }));
    } else if (s.mode === 'busy') {
      if (s.thumb && /^image\/(jpeg|png|gif|webp|avif)$/.test(s.thumb.type || '')) {
        thumbUrl = URL.createObjectURL(s.thumb);
        const img = Object.assign(document.createElement('img'), { className: 'isl-thumb', alt: '', src: thumbUrl });
        lead.appendChild(img);
      } else {
        const box = Object.assign(document.createElement('span'), { className: 'isl-glyph' });
        box.appendChild(icon(s.icon || 'file'));
        lead.appendChild(box);
      }
      const ring = Object.assign(document.createElement('span'), { className: 'ring' });
      trail.appendChild(ring);
    } else {
      const box = Object.assign(document.createElement('span'), { className: `isl-glyph ${s.mode}` });
      box.appendChild(icon(s.mode === 'done' ? 'check' : s.mode === 'oops' ? 'alert' : s.mode === 'off' ? 'moon' : s.icon || 'wifi'));
      lead.appendChild(box);
      if (s.mode === 'link' && s.live) trail.appendChild(Object.assign(document.createElement('span'), { className: 'live' }));
    }
  }
  $('isl-title').textContent = s.title;
  $('isl-sub').textContent = s.sub;
  $('isl-sub').hidden = !s.sub;
  const ring = trail.querySelector('.ring');
  if (ring) ring.style.setProperty('--p', String(Math.max(0, Math.min(1, s.progress ?? 0))));
  fit();
}

// The island's size follows its content, so it can grow and shrink smoothly.
function fit() {
  const isl = $('island');
  const inner = $('isl-inner');
  const w = Math.ceil(inner.scrollWidth);
  const h = Math.ceil(inner.scrollHeight);
  isl.style.width = `${w}px`;
  isl.style.height = `${h}px`;
}

addEventListener('resize', () => requestAnimationFrame(fit));
if (document.fonts) {
  // Outfit may arrive after the first layout: measure again once it has.
  if (document.fonts.ready) document.fonts.ready.then(fit).catch(() => {});
  if (document.fonts.addEventListener) document.fonts.addEventListener('loadingdone', () => requestAnimationFrame(fit));
}

/** Swap an Oyen picture, with a little pop; `move` = 'hop' plays a happy jump. */
export function pose(id, name, move = '') {
  const img = $(id);
  if (!img) return;
  const src = `/oyen-${name}.webp`;
  const changed = !img.getAttribute('src') || !img.getAttribute('src').endsWith(src);
  if (changed) {
    const dims = { peek: [640, 353], sit: [420, 564], asleep: [560, 407], shelf: [320, 440], note: [320, 318] }[name];
    img.setAttribute('src', src);
    if (dims) {
      img.width = dims[0];
      img.height = dims[1];
    }
    img.dataset.pose = name;
  }
  const anim = move || (changed ? 'pop' : '');
  if (anim) {
    img.classList.remove('pop', 'hop');
    void img.offsetWidth; // restart the animation
    img.classList.add(anim);
  }
}

/** Oyen carries the files while they travel. */
export function carrying(id, on) {
  const img = $(id);
  if (img) img.classList.toggle('carrying', on);
}
