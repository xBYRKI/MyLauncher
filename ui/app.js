// ---------- Brücke zum Rust-Backend (Tauri) ----------

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const appWindow = window.__TAURI__.window.getCurrentWindow();

function debounce(fn, ms) {
  let t;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
}

const L = {
  info: () => invoke('app_info'),
  getSettings: () => invoke('get_settings'),
  setSettings: (patch) => invoke('set_settings', { patch }),
  prismStatus: () => invoke('prism_status'),
  instances: () => invoke('instances'),
  accounts: () => invoke('accounts'),
  launch: ({ instanceId, server }) => invoke('launch', { instanceId, server: server ?? null }),
  showInPrism: (instanceId) => invoke('show_in_prism', { instanceId: instanceId ?? null }),
  openFolder: (instanceId) => invoke('open_folder', { instanceId: instanceId ?? null }),
  packs: () => invoke('packs'),
  syncPack: (source) => invoke('sync_pack', { source }),
  playPack: (source) => invoke('play_pack', { source }),
  news: () => invoke('news'),
  serverStatus: (address) => invoke('server_status', { address }),
  pickFile: () => invoke('pick_file'),
  pickDir: () => invoke('pick_dir'),
  openExternal: (url) => invoke('open_external', { url }),
  waypointOverview: () => invoke('waypoint_overview'),
  waypointSync: (instanceId) => invoke('waypoint_sync', { instanceId: instanceId ?? null }),
  waypointSetHooks: (instanceId, enabled) => invoke('waypoint_set_hooks', { instanceId, enabled }),
  waypointTest: () => invoke('waypoint_test'),
  onInstancesChanged: (cb) => listen('instances-changed', debounce(() => cb(), 400)),
  onPackProgress: (cb) => listen('pack-progress', (e) => cb(e.payload)),
  onFocus: (cb) => window.addEventListener('focus', debounce(cb, 300)),
};

const $ = (sel) => document.querySelector(sel);

const state = {
  info: null,
  settings: null,
  prism: null,
  instances: [],
  accounts: [],
  packs: [],
  busy: new Set(),     // laufende Pack-Syncs (manifest-URL)
  progress: {},        // manifest-URL -> letzter Fortschritt
};

// ---------- Helfer ----------

const ICONS = {
  home: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10.5 12 3l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/></svg>',
  grid: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/></svg>',
  box: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M21 8 12 3 3 8v8l9 5 9-5z"/><path d="m3 8 9 5 9-5M12 13v8"/></svg>',
  gear: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>',
  play: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5z"/></svg>',
  folder: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  external: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/></svg>',
  refresh: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7"/></svg>',
  plus: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>',
  pin: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M12 2 20 12 12 22 4 12z"/><path d="M12 8.5 15.5 12 12 15.5 8.5 12z"/></svg>',
  sync: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12a8 8 0 0 1 13.7-5.6L20 9M20 4v5h-5M20 12a8 8 0 0 1-13.7 5.6L4 15M4 20v-5h5"/></svg>',
  download: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14"/></svg>',
};

function icon(name) {
  const span = document.createElement('span');
  span.className = 'ico';
  span.innerHTML = ICONS[name] || '';
  return span;
}

function h(tag, props, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (value == null || value === false) continue;
    if (key === 'class') el.className = value;
    else if (key === 'style') Object.assign(el.style, value);
    else if (key.startsWith('on')) el.addEventListener(key.slice(2).toLowerCase(), value);
    else el.setAttribute(key, value === true ? '' : value);
  }
  for (const child of children.flat()) {
    if (child == null || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
  return el;
}

function errMsg(err) {
  return String(err?.message || err);
}

function toast(message, kind = 'info') {
  const el = h('div', { class: `toast ${kind}` }, message);
  $('#toasts').append(el);
  setTimeout(() => el.remove(), kind === 'error' ? 8000 : 4000);
}

function fmtPlaytime(sec) {
  if (!sec) return 'Noch nie gespielt';
  const hours = Math.floor(sec / 3600);
  const mins = Math.floor((sec % 3600) / 60);
  return hours ? `${hours} Std. ${mins} Min.` : `${mins} Min.`;
}

function fmtAgo(ms) {
  if (!ms) return '—';
  const s = (Date.now() - ms) / 1000;
  if (s < 60) return 'gerade eben';
  if (s < 3600) return `vor ${Math.floor(s / 60)} Min.`;
  if (s < 86400) return `vor ${Math.floor(s / 3600)} Std.`;
  const days = Math.floor(s / 86400);
  if (days < 30) return `vor ${days} Tag${days === 1 ? '' : 'en'}`;
  return new Date(ms).toLocaleDateString('de-DE');
}

function fmtBytes(n) {
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

function hueOf(text) {
  let x = 0;
  for (const c of text) x = (x * 31 + c.charCodeAt(0)) % 360;
  return x;
}

function instIcon(inst) {
  if (inst.icon) return h('img', { class: 'inst-icon', src: inst.icon, alt: '' });
  const hue = hueOf(inst.name);
  return h('div', {
    class: 'inst-icon',
    style: { background: `linear-gradient(135deg, hsl(${hue} 55% 48%), hsl(${(hue + 50) % 360} 55% 28%))` },
  }, inst.name.replace(/[^\p{L}\p{N}]/gu, '').slice(0, 2).toUpperCase() || '?');
}

function versionBadges(mc, loader) {
  return [
    h('span', { class: 'badge accent' }, mc),
    h('span', { class: 'badge' }, loader || 'Vanilla'),
  ];
}

function loaderText(inst) {
  return inst.loader ? `${inst.loader.name} ${inst.loader.version}` : 'Vanilla';
}

function packForInstance(inst) {
  return inst.managedBy && state.packs.find((p) => p.id === inst.managedBy.packId && !p.error);
}

// ---------- Aktionen ----------

async function withButton(btn, label, fn) {
  const old = btn ? [...btn.childNodes] : null;
  if (btn) {
    btn.disabled = true;
    btn.replaceChildren(label);
  }
  try {
    return await fn();
  } finally {
    if (btn) {
      setTimeout(() => {
        btn.disabled = false;
        btn.replaceChildren(...old);
      }, 1200);
    }
  }
}

async function playInstance(inst, btn) {
  const pack = packForInstance(inst);
  if (pack) return playPack(pack);
  await withButton(btn, 'Startet …', async () => {
    try {
      await L.launch({ instanceId: inst.id });
      toast(`${inst.name} wird über Prism gestartet`, 'ok');
    } catch (err) {
      toast(errMsg(err), 'error');
    }
  });
}

async function playPack(pack, installOnly = false) {
  if (state.busy.has(pack.source)) return;
  state.busy.add(pack.source);
  state.progress[pack.source] = { phase: 'check', done: 0, total: 0 };
  renderPackViews();
  try {
    const result = installOnly ? await L.syncPack(pack.source) : await L.playPack(pack.source);
    const changes = result.downloaded || result.removed
      ? ` (${result.downloaded} geladen, ${result.removed} entfernt)`
      : '';
    toast(installOnly ? `${pack.name} ist aktuell${changes}` : `${pack.name} wird gestartet${changes}`, 'ok');
  } catch (err) {
    toast(errMsg(err), 'error');
  } finally {
    state.busy.delete(pack.source);
    delete state.progress[pack.source];
    await loadPacks();
    await loadInstances();
  }
}

// ---------- Fortschritt ----------

function progressText(p) {
  if (!p) return '';
  if (p.phase === 'check') return `Prüfe Dateien … ${p.done}/${p.total}`;
  if (p.phase === 'download') {
    const size = p.totalBytes ? ` · ${fmtBytes(p.bytes)} / ${fmtBytes(p.totalBytes)}` : '';
    return `Lade ${p.done}/${p.total}${size}`;
  }
  if (p.phase === 'done') return 'Fertig – starte Prism …';
  return '';
}

function progressPercent(p) {
  if (!p) return 0;
  if (p.phase === 'done') return 100;
  if (p.phase === 'download' && p.totalBytes) return Math.min(100, (p.bytes / p.totalBytes) * 100);
  return p.total ? (p.done / p.total) * 100 : 0;
}

function progressEl(source) {
  const p = state.progress[source];
  return h('div', { class: 'progress', 'data-progress': source },
    h('div', { class: 'progress-bar' }, h('div', { class: 'progress-fill', style: { width: `${progressPercent(p)}%` } })),
    h('div', { class: 'progress-label' }, progressText(p) || 'Starte …'));
}

L.onPackProgress((p) => {
  state.progress[p.source] = p;
  for (const el of document.querySelectorAll('[data-progress]')) {
    if (el.dataset.progress !== p.source) continue;
    el.querySelector('.progress-fill').style.width = `${progressPercent(p)}%`;
    el.querySelector('.progress-label').textContent = progressText(p);
  }
});

// ---------- Render: Sidebar ----------

function renderBrand() {
  const { brand } = state.info;
  document.documentElement.style.setProperty('--accent', brand.accent);
  document.title = brand.name;
  $('#brandName').textContent = brand.name;
  $('#brandTag').textContent = brand.tagline || '';
  $('#logoMark').textContent = brand.name.slice(0, 1).toUpperCase();
  $('#appVersion').textContent = `${brand.name} ${state.info.appVersion}`;
  for (const el of document.querySelectorAll('[data-icon]')) el.replaceWith(icon(el.dataset.icon));
}

function renderPrismPill() {
  const p = state.prism;
  const pill = $('#prismPill');
  pill.replaceChildren(
    h('span', { class: `dot ${p.found ? 'ok' : 'err'}` }),
    p.found ? 'Prism verbunden' : 'Prism nicht gefunden'
  );
}

function selectedAccount() {
  return state.accounts.find((a) => a.name === state.settings.account) || state.accounts.find((a) => a.active) || state.accounts[0];
}

function renderAccount() {
  const select = $('#accountSelect');
  const active = state.accounts.find((a) => a.active);
  select.replaceChildren(
    h('option', { value: '', title: 'Standardkonto aus Prism' }, active ? active.name : 'Prism-Standard'),
    ...state.accounts.map((a) => h('option', { value: a.name, selected: a.name === state.settings.account }, a.name))
  );
  if (!state.accounts.length) select.replaceChildren(h('option', { value: '' }, 'Kein Konto – in Prism anmelden'));

  const acc = selectedAccount();
  const head = $('#accHead');
  head.replaceChildren(acc?.name?.[0]?.toUpperCase() || '?');
  if (acc?.uuid) {
    const img = h('img', { src: `https://mc-heads.net/avatar/${encodeURIComponent(acc.uuid)}/64`, alt: '' });
    img.onload = () => head.replaceChildren(img);
  }
}

// ---------- Render: Start ----------

function renderHero() {
  const hero = $('#hero');
  const pack = state.packs.find((p) => !p.error);
  const inst = state.instances[0];

  if (pack) {
    const busy = state.busy.has(pack.source);
    const status = pack.installedVersion
      ? pack.upToDate
        ? h('span', { class: 'badge ok' }, `v${pack.version} installiert`)
        : h('span', { class: 'badge warn' }, `Update ${pack.installedVersion} → ${pack.version}`)
      : h('span', { class: 'badge warn' }, 'Nicht installiert');
    hero.replaceChildren(
      h('div', { class: 'hero-kicker' }, 'Modpack'),
      h('h1', { class: 'hero-title' }, pack.name),
      h('div', { class: 'hero-meta' }, versionBadges(pack.minecraft, pack.loader), status,
        pack.server ? h('span', { class: 'badge' }, pack.server) : null),
      h('div', { class: 'hero-actions' },
        h('button', { class: 'btn big', disabled: busy, onClick: () => playPack(pack) },
          icon('play'), pack.installedVersion ? 'Spielen' : 'Installieren & spielen'),
        busy ? progressEl(pack.source) : null)
    );
    return;
  }

  if (inst) {
    hero.replaceChildren(
      h('div', { class: 'hero-kicker' }, 'Weiterspielen'),
      h('h1', { class: 'hero-title' }, inst.name),
      h('div', { class: 'hero-meta' }, versionBadges(inst.mcVersion, loaderText(inst)),
        h('span', { class: 'badge' }, `Zuletzt ${fmtAgo(inst.lastLaunch)}`)),
      h('div', { class: 'hero-actions' },
        h('button', { class: 'btn big', onClick: (e) => playInstance(inst, e.currentTarget) }, icon('play'), 'Spielen'))
    );
    return;
  }

  hero.replaceChildren(
    h('div', { class: 'hero-kicker' }, 'Willkommen'),
    h('h1', { class: 'hero-title' }, state.info.brand.name),
    h('p', { class: 'hero-empty' }, state.prism.found
      ? 'Noch keine Instanzen. Lege eine in Prism an oder trage ein Modpack in launcher.config.json ein.'
      : 'Prism Launcher wurde nicht gefunden. Installiere Prism oder setze den Pfad in den Einstellungen.'),
    h('div', { class: 'hero-actions' },
      h('button', { class: 'btn', onClick: () => (state.prism.found ? L.showInPrism() : switchView('settings')) },
        state.prism.found ? 'Prism öffnen' : 'Einstellungen'))
  );
}

function renderRecent() {
  const list = $('#recentList');
  const recent = state.instances.filter((i) => i.lastLaunch).slice(0, 4);
  if (!recent.length) {
    list.replaceChildren(h('div', { class: 'empty' }, 'Noch nichts gespielt.'));
    return;
  }
  list.replaceChildren(...recent.map((inst) =>
    h('div', { class: 'card recent' },
      instIcon(inst),
      h('div', { class: 'grow' },
        h('div', { class: 'recent-name' }, inst.name),
        h('div', { class: 'recent-sub' }, `${inst.mcVersion} · ${fmtAgo(inst.lastLaunch)}`)),
      h('button', { class: 'btn icon', title: 'Spielen', onClick: (e) => playInstance(inst, e.currentTarget) }, icon('play')))
  ));
}

async function renderNews() {
  const list = $('#newsList');
  let items = [];
  try {
    items = await L.news();
  } catch {
    // News sind optional
  }
  if (!items.length) {
    list.replaceChildren(h('div', { class: 'empty' }, 'Keine Neuigkeiten.'));
    return;
  }
  list.replaceChildren(...items.slice(0, 8).map((n) =>
    h('article', { class: 'card news' },
      n.image && /^https:\/\//.test(n.image) ? h('img', { src: n.image, alt: '' }) : null,
      h('div', { class: 'news-body' },
        n.date ? h('div', { class: 'news-date' }, new Date(n.date).toLocaleDateString('de-DE', { day: '2-digit', month: 'long', year: 'numeric' })) : null,
        h('div', { class: 'news-title' }, n.title || ''),
        h('div', { class: 'news-text' }, n.body || ''),
        n.url ? h('button', { class: 'btn ghost', onClick: () => L.openExternal(n.url) }, icon('external'), 'Mehr lesen') : null))
  ));
}

const MC_COLORS = {
  black: '#000', dark_blue: '#00a', dark_green: '#0a0', dark_aqua: '#0aa', dark_red: '#a00',
  dark_purple: '#a0a', gold: '#fa0', gray: '#aaa', dark_gray: '#555', blue: '#55f', green: '#5f5',
  aqua: '#5ff', red: '#f55', light_purple: '#f5f', yellow: '#ff5', white: '#fff',
};

function motdEl(parts) {
  return h('div', { class: 'server-motd' }, (parts || []).map((p) =>
    h('span', {
      style: {
        color: MC_COLORS[p.color] || (/^#[0-9a-f]{6}$/i.test(p.color || '') ? p.color : '#c4c6d6'),
        fontWeight: p.bold ? '700' : '',
        fontStyle: p.italic ? 'italic' : '',
      },
    }, p.text)));
}

async function renderServers() {
  const list = $('#serverList');
  const servers = state.info.servers;
  if (!servers.length) {
    list.replaceChildren(h('div', { class: 'empty' }, 'Keine Server eingetragen.'));
    return;
  }
  const cards = servers.map((srv) => {
    const card = h('div', { class: 'card server' },
      h('div', { class: 'server-icon' }),
      h('div', {}, h('div', { class: 'server-name' }, h('span', { class: 'dot' }), srv.name), h('div', { class: 'server-motd' }, srv.address)),
      h('div', { class: 'server-side' }, 'Pinge …'));
    return { srv, card };
  });
  list.replaceChildren(...cards.map((c) => c.card));

  await Promise.all(cards.map(async ({ srv, card }) => {
    const s = await L.serverStatus(srv.address);
    const inst = srv.instance && state.instances.find((i) => i.id === srv.instance);
    card.replaceChildren(
      s.favicon ? h('img', { class: 'server-icon', src: s.favicon, alt: '' }) : h('div', { class: 'server-icon' }),
      h('div', { style: { minWidth: 0 } },
        h('div', { class: 'server-name' }, h('span', { class: `dot ${s.online ? 'ok' : 'err'}` }), srv.name),
        s.online ? motdEl(s.motd) : h('div', { class: 'server-motd' }, `Offline · ${s.error || ''}`)),
      h('div', { class: 'server-side' },
        s.online ? h('div', { class: 'server-players' }, `${s.players.online.toLocaleString('de-DE')} / ${s.players.max.toLocaleString('de-DE')}`) : null,
        s.online ? h('div', {}, `${s.latency} ms`) : null,
        inst ? h('button', {
          class: 'btn',
          onClick: async () => {
            try {
              await L.launch({ instanceId: inst.id, server: srv.address });
              toast(`Verbinde mit ${srv.name} …`, 'ok');
            } catch (err) {
              toast(errMsg(err), 'error');
            }
          },
        }, 'Beitreten') : null)
    );
  }));
}

// ---------- Render: Instanzen ----------

function renderInstances() {
  $('#instCount').textContent = state.instances.length || '';
  const query = $('#instSearch').value.trim().toLowerCase();
  const grid = $('#instGrid');
  const list = state.instances.filter((i) =>
    !query || [i.name, i.mcVersion, i.group, loaderText(i)].some((v) => v && v.toLowerCase().includes(query)));

  if (!state.prism.found) {
    grid.replaceChildren(h('div', { class: 'empty' }, 'Prism Launcher nicht gefunden – siehe Einstellungen.'));
    return;
  }
  if (!list.length) {
    grid.replaceChildren(h('div', { class: 'empty' }, query ? 'Keine Treffer.' : 'Keine Instanzen in Prism.'));
    return;
  }
  grid.replaceChildren(...list.map((inst) =>
    h('div', { class: 'card inst' },
      h('div', { class: 'inst-top' },
        instIcon(inst),
        h('div', { style: { minWidth: 0 } },
          h('div', { class: 'inst-name', title: inst.name }, inst.name),
          h('div', { class: 'inst-meta' },
            versionBadges(inst.mcVersion, loaderText(inst)),
            inst.managedBy ? h('span', { class: 'badge accent' }, 'Modpack') : null,
            inst.waypointHooks ? h('span', { class: 'badge ok', title: 'Xaero-Wegpunkte werden bei Start/Ende synchronisiert' }, 'Wegpunkt-Sync') : null,
            inst.group ? h('span', { class: 'badge' }, inst.group) : null))),
      h('div', { class: 'inst-stats' },
        h('span', {}, fmtPlaytime(inst.totalPlayed)),
        h('span', {}, fmtAgo(inst.lastLaunch))),
      h('div', { class: 'inst-actions' },
        h('button', { class: 'btn', onClick: (e) => playInstance(inst, e.currentTarget) }, icon('play'), 'Spielen'),
        h('button', { class: 'btn ghost icon', title: 'In Prism bearbeiten (Mods, Einstellungen)', onClick: () => L.showInPrism(inst.id).catch((e) => toast(errMsg(e), 'error')) }, icon('external')),
        h('button', { class: 'btn ghost icon', title: 'Ordner öffnen', onClick: () => L.openFolder(inst.id).catch((e) => toast(errMsg(e), 'error')) }, icon('folder'))))
  ));
}

// ---------- Render: Modpacks ----------

function renderPacks() {
  const list = $('#packList');
  $('#packCount').textContent = state.packs.length || '';
  if (!state.packs.length) {
    list.replaceChildren(h('div', { class: 'empty' },
      'Noch keine Modpacks. Erzeuge ein Manifest mit ', h('code', {}, 'npm run manifest'),
      ' und trage es in ', h('code', {}, 'launcher.config.json'), ' unter ', h('code', {}, 'packs'), ' ein.'));
    return;
  }
  list.replaceChildren(...state.packs.map((pack) => {
    if (pack.error) {
      return h('div', { class: 'card pack' },
        h('div', { class: 'grow' },
          h('div', { class: 'pack-name' }, 'Modpack nicht erreichbar'),
          h('div', { class: 'pack-error' }, `${pack.source}\n${pack.error}`)));
    }
    const busy = state.busy.has(pack.source);
    const installed = !!pack.installedVersion;
    return h('div', { class: 'card pack' },
      instIcon({ name: pack.name }),
      h('div', { class: 'grow' },
        h('div', { class: 'pack-name' }, pack.name),
        pack.description ? h('div', { class: 'pack-desc' }, pack.description) : null,
        h('div', { class: 'inst-meta' },
          versionBadges(pack.minecraft, pack.loader),
          h('span', { class: 'badge' }, `${pack.fileCount} Dateien`),
          installed
            ? h('span', { class: `badge ${pack.upToDate ? 'ok' : 'warn'}` }, pack.upToDate ? `v${pack.version}` : `Update ${pack.installedVersion} → ${pack.version}`)
            : h('span', { class: 'badge warn' }, `v${pack.version} · nicht installiert`))),
      h('div', { class: 'pack-side' },
        busy ? progressEl(pack.source) : h('div', { class: 'row' },
          h('button', { class: 'btn ghost', onClick: () => playPack(pack, true) },
            icon(installed ? 'refresh' : 'download'), installed ? (pack.upToDate ? 'Prüfen' : 'Aktualisieren') : 'Installieren'),
          h('button', { class: 'btn', onClick: () => playPack(pack) }, icon('play'), 'Spielen'))));
  }));
}

function renderPackViews() {
  renderHero();
  renderPacks();
}

// ---------- Render: Wegpunkte ----------

function toggle(on, onChange, title) {
  const input = h('input', { type: 'checkbox', checked: on });
  input.addEventListener('change', () => onChange(input.checked, input));
  return h('label', { class: 'switch', title }, input, h('span', { class: 'slider' }));
}

async function loadWaypoints() {
  try {
    state.wp = await L.waypointOverview();
  } catch (err) {
    toast(errMsg(err), 'error');
    return;
  }
  renderWaypoints();
}

async function toggleWorld(folder, on) {
  const worlds = new Set(state.settings.waypoints.worlds);
  if (on) worlds.add(folder);
  else worlds.delete(folder);
  state.settings = await L.setSettings({ waypoints: { worlds: [...worlds] } });
  await loadWaypoints();
}

async function toggleHooks(inst, on, input) {
  try {
    await L.waypointSetHooks(inst.id, on);
    toast(on ? `${inst.name}: Wegpunkte werden bei Start und Ende synchronisiert` : `${inst.name}: Auto-Sync aus`, 'ok');
  } catch (err) {
    input.checked = !on;
    toast(errMsg(err), 'error');
  }
  await loadWaypoints();
  await loadInstances();
}

async function testWaypointServer() {
  try {
    const r = await L.waypointTest();
    toast(`Verbunden – Team „${r.team}“`, 'ok');
  } catch (err) {
    toast(errMsg(err), 'error');
  }
}

async function syncWaypoints(btn) {
  await withButton(btn, 'Synchronisiere …', async () => {
    try {
      const lines = await L.waypointSync();
      $('#wpLog').replaceChildren(...lines.map((l) =>
        h('div', { class: `wp-log-line ${l.ok ? 'ok' : 'err'}` }, h('strong', {}, l.instance), ` · ${l.message}`)));
      if (!lines.length) toast('Keine freigegebene Welt in deinen Instanzen gefunden.');
      else if (lines.every((l) => l.ok)) toast('Wegpunkte synchronisiert', 'ok');
    } catch (err) {
      toast(errMsg(err), 'error');
    }
  });
  await loadWaypoints();
}

function renderWaypoints() {
  const wp = state.wp;
  if (!wp) return;
  const ws = state.settings.waypoints;
  const set = ws.sharedSet || 'Team';
  $('#wpCount').textContent = wp.worlds.filter((w) => w.enabled).length || '';
  $('#wpIntro').textContent = ws.shareAll
    ? 'Alle Wegpunkte der freigegebenen Welten werden mit deinem Team geteilt (Todespunkte bleiben privat).'
    : `Wegpunkte im Xaero-Set „${set}“ werden mit deinem Team geteilt – alle anderen bleiben privat.`;

  const configured = !!(wp.serverUrl && ws.token);
  $('#wpStatus').replaceChildren(
    h('span', { class: `dot ${configured ? 'ok' : 'err'}` }),
    h('div', { class: 'grow' },
      h('div', { class: 'wp-status-title' }, configured ? 'Sync-Server eingerichtet' : 'Sync-Server fehlt'),
      h('div', { class: 'muted' }, configured ? wp.serverUrl : 'Trage Server-URL und Team-Schlüssel in den Einstellungen ein.')),
    configured
      ? h('button', { class: 'btn ghost', onClick: testWaypointServer }, 'Verbindung testen')
      : h('button', { class: 'btn', onClick: () => switchView('settings') }, 'Einrichten'));

  const worlds = $('#wpWorlds');
  if (!wp.worlds.length) {
    worlds.replaceChildren(h('div', { class: 'empty' },
      'Keine Xaero-Wegpunkte gefunden. Installiere Xaero’s Minimap in einer Instanz und setze einen Wegpunkt.'));
  } else {
    worlds.replaceChildren(...wp.worlds.map((w) =>
      h('div', { class: `card wp-world ${w.enabled ? 'on' : ''}` },
        h('div', { class: 'wp-world-head' },
          h('div', { class: 'grow' },
            h('div', { class: 'wp-world-name' }, w.display),
            h('div', { class: 'inst-meta' },
              h('span', { class: 'badge' }, w.multiplayer ? 'Server' : 'Einzelspieler'),
              w.enabled ? h('span', { class: 'badge ok' }, 'wird geteilt') : null)),
          toggle(w.enabled, (on) => toggleWorld(w.folder, on), 'Diese Welt mit dem Team teilen')),
        h('div', { class: 'wp-world-insts' }, w.instances.map((i) =>
          h('div', { class: 'wp-inst' },
            h('span', { class: 'wp-inst-name' }, i.name),
            h('span', { class: 'muted' },
              `${i.waypoints} Wegpunkte · ${i.shared} geteilt · Sync ${i.lastSync ? fmtAgo(i.lastSync) : 'noch nie'}`)))))));
  }

  const hooks = $('#wpHooks');
  const list = wp.instances.filter((i) => i.hasXaero);
  const rows = (list.length ? list : wp.instances).map((inst) =>
    h('div', { class: 'card wp-hook' },
      h('div', { class: 'grow' },
        h('div', { class: 'wp-inst-name' }, inst.name),
        h('div', { class: 'muted' }, inst.hooks ? 'Sync vor dem Start und nach dem Beenden' : inst.hasXaero ? 'Aus' : 'Xaero nicht gefunden')),
      toggle(inst.hooks, (on, input) => toggleHooks(inst, on, input), 'Automatischer Sync über Prism')));
  hooks.replaceChildren(
    wp.prismRunning ? h('div', { class: 'wp-warn' }, 'Prism läuft gerade. Zum Ein- oder Ausschalten bitte Prism schließen – sonst überschreibt Prism die Einstellung.') : null,
    ...rows);

  $('#wpHowto').replaceChildren(h('ol', {},
    h('li', {}, 'Im Spiel das Wegpunkt-Menü von Xaero öffnen (Standard: ', h('kbd', {}, 'U'), ').'),
    ws.shareAll
      ? h('li', {}, 'Wegpunkt ganz normal setzen – alle werden geteilt.')
      : h('li', {}, `Wegpunkt bearbeiten und dem Set „${set}“ zuordnen. Gibt es das Set noch nicht, lege es in Xaero an.`),
    h('li', {}, 'Oben die Welt mit „Teilen“ freigeben.'),
    h('li', {}, 'Auto-Sync für die Instanz einschalten – oder bei geschlossenem Spiel auf „Jetzt synchronisieren“ klicken.')));
}

// ---------- Render: Einstellungen ----------

function renderSettings() {
  const s = state.settings;
  const p = state.prism;
  $('#setPrismExe').value = s.prismExe;
  $('#setPrismExe').placeholder = p.exe ? `Automatisch: ${p.exe}` : 'Nicht gefunden – bitte auswählen';
  $('#setPrismData').value = s.prismDataDir;
  $('#setPrismData').placeholder = `Automatisch: ${p.defaultDataDir}`;
  $('#setAfterLaunch').value = s.afterLaunch;
  $('#setAutoUpdate').checked = s.autoUpdatePacks;
  $('#setJoinServer').checked = s.joinServer;
  $('#setWpUrl').value = s.waypoints.serverUrl;
  $('#setWpUrl').placeholder = state.info.waypointServer || 'https://wegpunkte.example.net';
  $('#setWpToken').value = s.waypoints.token;
  $('#setWpSet').value = s.waypoints.sharedSet;
  $('#setWpAll').checked = s.waypoints.shareAll;

  const row = (k, v) => [h('dt', {}, k), h('dd', {}, v)];
  $('#prismStatus').replaceChildren(h('dl', { class: 'prism-status', style: { margin: 0 } },
    row('Status', h('span', {}, h('span', { class: `dot ${p.found ? 'ok' : 'err'}`, style: { display: 'inline-block', marginRight: '8px' } }),
      p.found ? 'Verbunden' : !p.exeFound ? 'Programm nicht gefunden' : 'Datenordner nicht gefunden')),
    row('Programm', p.exe || '—'),
    row('Daten', `${p.dataDir}${p.portable ? ' (portable)' : ''}`),
    row('Instanzen', `${state.instances.length} in ${p.instancesDir}`),
    row('Konten', state.accounts.map((a) => a.name).join(', ') || '—')));
}

async function saveSetting(patch) {
  state.settings = await L.setSettings(patch);
  await refreshPrism();
}

// ---------- Laden ----------

async function loadInstances() {
  state.instances = await L.instances();
  renderInstances();
  renderRecent();
  renderHero();
}

async function loadPacks() {
  state.packs = await L.packs();
  renderPackViews();
  renderInstances();
}

async function refreshPrism() {
  [state.prism, state.accounts] = await Promise.all([L.prismStatus(), L.accounts()]);
  renderPrismPill();
  renderAccount();
  await loadInstances();
  renderSettings();
}

function switchView(view) {
  for (const btn of document.querySelectorAll('.nav-item')) btn.classList.toggle('active', btn.dataset.view === view);
  for (const sec of document.querySelectorAll('.view')) sec.classList.toggle('active', sec.id === `view-${view}`);
}

function bindUi() {
  $('#winMin').addEventListener('click', () => appWindow.minimize());
  $('#winMax').addEventListener('click', () => appWindow.toggleMaximize());
  $('#winClose').addEventListener('click', () => appWindow.close());

  for (const btn of document.querySelectorAll('.nav-item')) {
    btn.addEventListener('click', () => {
      switchView(btn.dataset.view);
      if (btn.dataset.view === 'waypoints') loadWaypoints();
    });
  }
  $('#btnWpSync').addEventListener('click', (e) => syncWaypoints(e.currentTarget));
  $('#btnWpRefresh').addEventListener('click', loadWaypoints);
  $('#btnWpTest').addEventListener('click', testWaypointServer);
  const wpSetting = (key, value) => saveSetting({ waypoints: { [key]: value } }).then(renderWaypoints);
  $('#setWpUrl').addEventListener('change', (e) => wpSetting('serverUrl', e.target.value.trim()));
  $('#setWpToken').addEventListener('change', (e) => wpSetting('token', e.target.value.trim()));
  $('#setWpSet').addEventListener('change', (e) => wpSetting('sharedSet', e.target.value.trim() || 'Team'));
  $('#setWpAll').addEventListener('change', (e) => wpSetting('shareAll', e.target.checked));

  $('#instSearch').addEventListener('input', renderInstances);
  $('#accountSelect').addEventListener('change', (e) => saveSetting({ account: e.target.value }));
  $('#prismPill').addEventListener('click', () => (state.prism.found ? L.showInPrism().catch((e) => toast(errMsg(e), 'error')) : switchView('settings')));
  $('#btnNewInPrism').addEventListener('click', () => L.showInPrism().catch((e) => toast(errMsg(e), 'error')));
  $('#btnOpenPrism').addEventListener('click', () => L.showInPrism().catch((e) => toast(errMsg(e), 'error')));
  $('#btnOpenData').addEventListener('click', () => L.openFolder().catch((e) => toast(errMsg(e), 'error')));
  $('#btnRefreshPacks').addEventListener('click', loadPacks);

  $('#setPrismExe').addEventListener('change', (e) => saveSetting({ prismExe: e.target.value.trim() }));
  $('#setPrismData').addEventListener('change', (e) => saveSetting({ prismDataDir: e.target.value.trim() }));
  $('#pickExe').addEventListener('click', async () => {
    const file = await L.pickFile();
    if (file) saveSetting({ prismExe: file });
  });
  $('#pickData').addEventListener('click', async () => {
    const dir = await L.pickDir();
    if (dir) saveSetting({ prismDataDir: dir });
  });
  $('#setAfterLaunch').addEventListener('change', (e) => saveSetting({ afterLaunch: e.target.value }));
  $('#setAutoUpdate').addEventListener('change', (e) => saveSetting({ autoUpdatePacks: e.target.checked }));
  $('#setJoinServer').addEventListener('change', (e) => saveSetting({ joinServer: e.target.checked }));

  L.onInstancesChanged(loadInstances);
  L.onFocus(() => refreshPrism().catch(() => {}));
}

async function init() {
  [state.info, state.settings] = await Promise.all([L.info(), L.getSettings()]);
  renderBrand();
  bindUi();
  await refreshPrism();
  renderNews();
  await loadPacks();
  renderServers();
  loadWaypoints();
  setInterval(renderServers, 60000);
}

init().catch((err) => toast(errMsg(err), 'error'));
