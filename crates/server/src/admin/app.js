'use strict';

// MyLauncher Admin – ohne Build-Schritt und ohne externe Bibliotheken.
// Alle Inhalte werden per textContent/DOM-Knoten eingesetzt, nie als HTML.

const state = { authed: false, status: null, packs: [], ui: {} };
const LOADERS = [
  ['', 'Vanilla'],
  ['fabric', 'Fabric'],
  ['quilt', 'Quilt'],
  ['forge', 'Forge'],
  ['neoforge', 'NeoForge'],
];
const JUNK = /(^|\/)(Thumbs\.db|desktop\.ini|\.DS_Store)$/i;
const HASH_LIMIT = 64 * 1024 * 1024;
const PARALLEL_UPLOADS = 3;

// ---------- Hilfen ----------

function h(tag, attrs, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'value') el.value = v;
    else if (k === 'checked') el.checked = !!v;
    else if (v === true) el.setAttribute(k, '');
    else el.setAttribute(k, v);
  }
  for (const c of children.flat(Infinity)) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : String(c));
  }
  return el;
}

const badge = (text, tone) => h('span', { class: 'badge' + (tone ? ' ' + tone : '') }, text);

function fmtBytes(n) {
  if (!n) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB'];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), u.length - 1);
  return (n / 1024 ** i).toFixed(i ? 1 : 0).replace('.', ',') + ' ' + u[i];
}

const fmtDate = ms => (ms ? new Date(ms).toLocaleString('de-AT', { dateStyle: 'medium', timeStyle: 'short' }) : '–');
const today = () => new Date().toISOString().slice(0, 10);

function loaderText(l) {
  if (!l || !l.type || l.type === 'vanilla') return 'Vanilla';
  const name = (LOADERS.find(x => x[0] === l.type) || [l.type, l.type])[1];
  return `${name} ${l.version || ''}`.trim();
}

/** `1.2.3` → `1.2.4`, `1.0.0-beta.1` → `1.0.0-beta.2` */
function bump(v) {
  if (!v) return '1.0.0';
  const m = v.match(/^(.*?)(\d+)(\D*)$/);
  return m ? m[1] + (Number(m[2]) + 1) + m[3] : v + '.1';
}

/** Wie der Launcher: numerisch, Vorabversionen sind älter. */
function compareVersions(a, b) {
  const split = v => {
    const [core, pre = ''] = v.trim().replace(/^[vV]/, '').split(/-(.*)/s);
    return [core.split('.').map(p => parseInt(p, 10) || 0), pre];
  };
  const [an, ap] = split(a);
  const [bn, bp] = split(b);
  for (let i = 0; i < Math.max(an.length, bn.length); i++) {
    const d = (an[i] || 0) - (bn[i] || 0);
    if (d) return Math.sign(d);
  }
  if (!ap && !bp) return 0;
  if (!ap) return 1;
  if (!bp) return -1;
  return ap < bp ? -1 : ap > bp ? 1 : 0;
}

function slug(text) {
  return text.toLowerCase()
    .replace(/ä/g, 'ae').replace(/ö/g, 'oe').replace(/ü/g, 'ue').replace(/ß/g, 'ss')
    .replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 64);
}

function joinPath(prefix, path) {
  const p = (prefix || '').replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  return p ? `${p}/${path}` : path;
}

async function sha1Hex(file) {
  if (!window.crypto || !crypto.subtle || file.size > HASH_LIMIT) return null;
  try {
    const digest = await crypto.subtle.digest('SHA-1', await file.arrayBuffer());
    return [...new Uint8Array(digest)].map(b => b.toString(16).padStart(2, '0')).join('');
  } catch {
    return null;
  }
}

function copyField(text) {
  return h('div', { class: 'copy' },
    h('code', { title: text }, text),
    h('button', {
      class: 'small', type: 'button',
      onclick: async () => {
        try {
          await navigator.clipboard.writeText(text);
          toast('Kopiert', 'ok');
        } catch {
          toast('Kopieren nicht möglich – bitte markieren und kopieren.', 'error');
        }
      },
    }, 'Kopieren'));
}

// ---------- Server ----------

function handle(status, text) {
  if (status === 401 && state.authed) {
    state.authed = false;
    renderLogin('Sitzung abgelaufen – bitte neu anmelden.');
  }
  if (status < 200 || status >= 300) throw new Error(text || `HTTP ${status}`);
  return text ? JSON.parse(text) : null;
}

async function api(method, path, body) {
  const headers = { 'X-MyLauncher': '1' };
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  const res = await fetch('/admin/api' + path, {
    method,
    headers,
    credentials: 'same-origin',
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return handle(res.status, await res.text());
}

function upload(path, file, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('PUT', '/admin/api' + path);
    xhr.setRequestHeader('X-MyLauncher', '1');
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    xhr.upload.onprogress = e => onProgress && onProgress(e.loaded, e.total);
    xhr.onload = () => {
      try {
        resolve(handle(xhr.status, xhr.responseText));
      } catch (e) {
        reject(e);
      }
    };
    xhr.onerror = () => reject(new Error('Netzwerkfehler beim Hochladen'));
    xhr.send(file);
  });
}

/** Knopf sperren, solange die Aktion läuft; Fehler als Meldung. */
async function busy(button, fn) {
  if (button) button.disabled = true;
  try {
    return await fn();
  } catch (e) {
    toast(e.message, 'error');
  } finally {
    if (button) button.disabled = false;
  }
}

// ---------- Meldungen & Dialoge ----------

function toast(text, tone = 'info') {
  const el = h('div', { class: 'toast ' + tone, role: tone === 'error' ? 'alert' : 'status' }, text);
  document.getElementById('toasts').append(el);
  setTimeout(() => el.remove(), tone === 'error' ? 8000 : 3500);
}

/**
 * Eigener Dialog statt prompt()/confirm().
 * fields: [{ name, label, value, type, options, placeholder, hint }]
 * Ergebnis: Objekt mit den Feldwerten oder null bei Abbruch.
 */
function ask({ title, text, fields = [], ok = 'OK', danger = false }) {
  return new Promise(resolve => {
    const inputs = {};
    const close = result => {
      backdrop.remove();
      document.removeEventListener('keydown', onKey);
      resolve(result);
    };
    const onKey = e => {
      if (e.key === 'Escape') close(null);
    };
    const form = h('form', { class: 'dialog', role: 'dialog', 'aria-modal': 'true' },
      h('h2', {}, title),
      text ? h('p', {}, text) : null,
      fields.map(f => {
        let input;
        if (f.type === 'select') {
          input = h('select', {}, f.options.map(([v, l]) => h('option', { value: v }, l)));
          input.value = f.value ?? '';
        } else if (f.type === 'textarea') {
          input = h('textarea', { rows: 4, placeholder: f.placeholder }, f.value ?? '');
        } else if (f.type === 'checkbox') {
          input = h('input', { type: 'checkbox', checked: f.value });
          inputs[f.name] = input;
          return h('label', { class: 'check' }, input, f.label);
        } else {
          input = h('input', { type: f.type || 'text', value: f.value ?? '', placeholder: f.placeholder });
        }
        inputs[f.name] = input;
        return h('label', { class: 'field' }, h('span', {}, f.label), input, f.hint ? h('small', {}, f.hint) : null);
      }),
      h('div', { class: 'actions' },
        h('button', { type: 'button', onclick: () => close(null) }, 'Abbrechen'),
        h('button', { type: 'submit', class: danger ? 'danger' : 'primary' }, ok)));
    form.addEventListener('submit', e => {
      e.preventDefault();
      const values = {};
      for (const [k, el] of Object.entries(inputs)) values[k] = el.type === 'checkbox' ? el.checked : el.value;
      close(values);
    });
    const backdrop = h('div', { class: 'backdrop', onclick: e => e.target === backdrop && close(null) }, form);
    document.body.append(backdrop);
    document.addEventListener('keydown', onKey);
    (form.querySelector('input, textarea, select') || form.querySelector('button[type=submit]')).focus();
  });
}

const confirmAction = (title, text, ok = 'Löschen') => ask({ title, text, ok, danger: true }).then(r => !!r);

// ---------- Anmeldung ----------

function renderLogin(message) {
  const password = h('input', { type: 'password', autocomplete: 'current-password', required: true });
  const error = h('div', { class: 'error' }, message || '');
  const submit = h('button', { class: 'primary', type: 'submit' }, 'Anmelden');
  const form = h('form', {},
    h('div', { class: 'logo' }, h('img', { src: '/admin/logo.png', alt: '' }), h('h1', {}, 'Admin')),
    h('p', { class: 'muted' }, 'Modpacks, News und Launcher-Updates verwalten.'),
    h('label', { class: 'field' }, h('span', {}, 'Admin-Passwort'), password),
    error,
    submit);
  form.addEventListener('submit', async e => {
    e.preventDefault();
    submit.disabled = true;
    error.textContent = '';
    try {
      await api('POST', '/login', { password: password.value });
      await boot();
    } catch (err) {
      error.textContent = err.message;
      password.select();
    } finally {
      submit.disabled = false;
    }
  });
  document.getElementById('root').replaceChildren(h('div', { class: 'login' }, form));
  password.focus();
}

async function logout() {
  try {
    await api('POST', '/logout');
  } catch { /* egal */ }
  state.authed = false;
  renderLogin();
}

// ---------- Rahmen & Navigation ----------

const NAV = [
  ['packs', 'Modpacks'],
  ['news', 'News'],
  ['launcher', 'Launcher'],
  ['updates', 'Updates'],
  ['system', 'System'],
];

function renderShell() {
  const nav = h('nav', { class: 'nav', id: 'nav' });
  document.getElementById('root').replaceChildren(
    h('div', { class: 'app' },
      h('header', { class: 'topbar' },
        h('img', { src: '/admin/logo.png', alt: '' }),
        h('div', { class: 'brand' }, 'MyLauncher ', h('b', {}, 'Admin')),
        h('span', { class: 'muted small hide-narrow' }, state.status.publicUrl),
        h('button', { class: 'ghost small right', onclick: logout }, 'Abmelden')),
      nav,
      h('main', { id: 'view' })));
  renderNav();
}

function renderNav() {
  const nav = document.getElementById('nav');
  if (!nav) return;
  const section = (location.hash.slice(2) || 'packs').split('/')[0];
  nav.replaceChildren(...NAV.map(([key, label]) => {
    let extra = null;
    if (key === 'packs' && state.packs.length) {
      extra = state.packs.some(p => p.draft.dirty) ? h('span', { class: 'dot', title: 'Unveröffentlichte Änderungen' }) : h('span', { class: 'count' }, state.packs.length);
    }
    return h('a', { href: '#/' + key, class: section === key ? 'active' : '' }, label, extra);
  }));
}

function head(title, text, ...actions) {
  return h('div', { class: 'head' },
    h('div', { class: 'grow' }, h('h1', {}, title), text ? h('p', {}, text) : null),
    actions.length ? h('div', { class: 'row' }, actions) : null);
}

async function route() {
  if (!state.authed) return;
  const [section = 'packs', ...rest] = location.hash.slice(2).split('/');
  const main = document.getElementById('view');
  renderNav();
  try {
    if (section === 'packs' && rest[0]) await viewPack(main, decodeURIComponent(rest[0]));
    else if (section === 'news') await viewNews(main);
    else if (section === 'launcher') await viewLauncher(main);
    else if (section === 'updates') await viewUpdates(main);
    else if (section === 'system') await viewSystem(main);
    else await viewPacks(main);
  } catch (e) {
    if (state.authed) main.replaceChildren(h('div', { class: 'panel' }, h('div', { class: 'empty' }, e.message)));
  }
  renderNav();
}

async function refreshPacks() {
  state.packs = await api('GET', '/packs');
  renderNav();
}

// ---------- Modpacks: Übersicht ----------

async function viewPacks(main) {
  await refreshPacks();
  const cards = state.packs.map(p => h('a', { class: 'card', href: '#/packs/' + encodeURIComponent(p.id) },
    h('div', { class: 'row' },
      h('h2', { class: 'grow' }, p.name),
      p.current ? badge('v' + p.current, 'green') : badge('unveröffentlicht')),
    h('div', { class: 'muted small mono' }, p.id),
    h('div', { class: 'meta' },
      badge('MC ' + p.minecraft),
      badge(loaderText(p.loader)),
      p.listed ? badge('im Launcher', 'gold') : badge('ausgeblendet'),
      p.current && p.draft.dirty ? badge('Entwurf geändert', 'blue') : null),
    h('div', { class: 'muted small' }, `${p.fileCount} Dateien · ${fmtBytes(p.totalSize)} · geändert ${fmtDate(p.updatedAt)}`)));
  main.replaceChildren(
    head('Modpacks', 'Dateien hochladen, als Version veröffentlichen – der Launcher holt sich den Rest.',
      h('button', { class: 'primary', onclick: newPack }, '+ Neues Modpack')),
    cards.length
      ? h('div', { class: 'cards' }, cards)
      : h('div', { class: 'panel' }, h('div', { class: 'empty' },
        h('p', {}, 'Noch keine Modpacks.'),
        h('button', { class: 'primary', onclick: newPack }, 'Erstes Modpack anlegen'))));
}

async function newPack() {
  const templates = state.packs.map(p => [p.id, `${p.name} (${p.id})`]);
  const v = await ask({
    title: 'Neues Modpack',
    fields: [
      { name: 'name', label: 'Name', placeholder: 'Survival 2026' },
      { name: 'id', label: 'ID', placeholder: 'wird aus dem Namen gebildet', hint: 'Kleinbuchstaben, Ziffern, - und _. Steht in der URL und lässt sich nicht mehr ändern.' },
      { name: 'minecraft', label: 'Minecraft-Version', placeholder: '1.21.4' },
      { name: 'loader', label: 'Loader', type: 'select', options: LOADERS, value: 'fabric' },
      { name: 'loaderVersion', label: 'Loader-Version', placeholder: '0.16.10' },
      { name: 'listed', label: 'Im Launcher anzeigen', type: 'checkbox', value: true },
      templates.length ? { name: 'copyFrom', label: 'Dateien und Einstellungen übernehmen von', type: 'select', options: [['', '– nichts –'], ...templates] } : null,
    ].filter(Boolean),
    ok: 'Anlegen',
  });
  if (!v) return;
  const id = v.id.trim() || slug(v.name);
  try {
    await api('POST', '/packs', {
      id,
      name: v.name,
      minecraft: v.minecraft,
      loader: { type: v.loader, version: v.loaderVersion },
      listed: v.listed,
      copyFrom: v.copyFrom || null,
    });
    toast(`Modpack „${v.name || id}“ angelegt`, 'ok');
    location.hash = '#/packs/' + encodeURIComponent(id);
  } catch (e) {
    toast(e.message, 'error');
  }
}

// ---------- Modpacks: Detail ----------

function packUi(id) {
  if (!state.ui[id]) state.ui[id] = { filter: '', selected: new Set(), target: 'mods', keep: false, strip: false };
  return state.ui[id];
}

async function viewPack(main, id) {
  const pack = await api('GET', '/packs/' + encodeURIComponent(id));
  renderPack(main, pack);
}

function renderPack(main, pack) {
  const id = encodeURIComponent(pack.id);
  const manifestUrl = `${state.status.publicUrl}/launcher/packs/${pack.id}/manifest.json`;
  const reload = async detail => {
    renderPack(main, detail || await api('GET', '/packs/' + id));
    refreshPacks().catch(() => {});
  };

  main.replaceChildren(
    h('div', { class: 'head' },
      h('div', { class: 'grow' },
        h('a', { class: 'crumb', href: '#/packs' }, '← Modpacks'),
        h('h1', {}, pack.name),
        h('div', { class: 'row' },
          h('span', { class: 'muted mono small' }, pack.id),
          badge('MC ' + pack.minecraft),
          badge(loaderText(pack.loader)),
          pack.current ? badge('ausgeliefert: v' + pack.current, 'green') : badge('noch nicht veröffentlicht'),
          pack.listed ? badge('im Launcher', 'gold') : badge('ausgeblendet')))),
    publishPanel(pack, reload),
    filesPanel(pack, reload),
    settingsPanel(pack, reload),
    versionsPanel(pack, reload),
    h('section', { class: 'panel' },
      h('header', {}, h('h2', {}, 'Adresse')),
      h('div', { class: 'body stack' },
        h('p', { class: 'muted small', }, 'Ist das Pack „im Launcher“ sichtbar, bekommen Spieler es automatisch über die Server-Konfiguration. Die Manifest-Adresse brauchst du nur für Launcher ohne remoteConfig.'),
        copyField(manifestUrl))),
    h('section', { class: 'panel danger' },
      h('header', {}, h('h2', {}, 'Modpack löschen')),
      h('div', { class: 'body row' },
        h('p', { class: 'muted grow' }, 'Entfernt Pack, Entwurf und alle Versionen vom Server. Installierte Instanzen der Spieler bleiben erhalten.'),
        h('button', {
          class: 'danger',
          onclick: async e => {
            if (!await confirmAction('Modpack löschen?', `„${pack.name}“ mit ${pack.versions.length} Version(en) endgültig löschen?`)) return;
            await busy(e.target, async () => {
              await api('DELETE', '/packs/' + id);
              toast('Modpack gelöscht', 'ok');
              location.hash = '#/packs';
            });
          },
        }, 'Löschen'))));
}

function diffBadges(draft) {
  if (draft.unpublished) return [badge(`${draft.added} Dateien`, 'blue'), badge('erste Version', 'gold')];
  if (!draft.dirty) return [badge('keine Änderungen seit der letzten Version')];
  return [
    draft.added ? badge(`+${draft.added} neu`, 'green') : null,
    draft.changed ? badge(`~${draft.changed} geändert`, 'blue') : null,
    draft.removed ? badge(`−${draft.removed} entfernt`, 'red') : null,
    draft.meta ? badge('Einstellungen geändert', 'gold') : null,
  ];
}

function publishPanel(pack, reload) {
  const id = encodeURIComponent(pack.id);
  const latest = pack.versions.length ? pack.versions[pack.versions.length - 1].version : '';
  const version = h('input', { type: 'text', id: 'publish-version', value: bump(latest), 'aria-label': 'Version' });
  const notes = h('input', { type: 'text', placeholder: 'Was ist neu? (optional)', 'aria-label': 'Änderungen' });
  const canPublish = pack.draft.dirty || !pack.current;
  const publishBtn = h('button', {
    class: 'primary',
    disabled: !canPublish,
    onclick: e => busy(e.target, async () => {
      const v = version.value.trim();
      if (latest && compareVersions(v, latest) <= 0 &&
        !await confirmAction('Version ist nicht höher', `${v} ist nicht höher als ${latest}. Trotzdem veröffentlichen?`, 'Veröffentlichen')) return;
      const detail = await api('POST', `/packs/${id}/publish`, { version: v, notes: notes.value });
      toast(`Version ${v} veröffentlicht – Spieler bekommen sie beim nächsten Start.`, 'ok');
      await reload(detail);
    }),
  }, 'Veröffentlichen');
  const discard = pack.current && pack.draft.dirty ? h('button', {
    class: 'ghost',
    onclick: async e => {
      if (!await confirmAction('Entwurf verwerfen?', `Alle Änderungen seit v${pack.current} gehen verloren.`, 'Verwerfen')) return;
      await busy(e.target, async () => reload(await api('POST', `/packs/${id}/reset`, { version: pack.current })));
    },
  }, 'Entwurf verwerfen') : null;

  return h('section', { class: 'panel' },
    h('header', {},
      h('h2', {}, 'Veröffentlichen'),
      h('div', { class: 'diff' }, diffBadges(pack.draft))),
    h('div', { class: 'body' },
      h('p', { class: 'muted small', }, 'Spieler sehen nur veröffentlichte Versionen. Alles unten ist dein Entwurf – erst dieser Knopf friert ihn als neue Version ein.'),
      h('div', { class: 'row' },
        h('label', { class: 'field' }, h('span', {}, 'Version'), version),
        h('label', { class: 'field grow' }, h('span', {}, 'Änderungen'), notes)),
      h('div', { class: 'row', }, h('div', { class: 'grow' }), discard, publishBtn)));
}

// ----- Dateien -----

async function collectDrop(dataTransfer) {
  // webkitGetAsEntry muss synchron im Drop-Ereignis aufgerufen werden.
  const entries = [...dataTransfer.items]
    .map(i => (i.webkitGetAsEntry ? i.webkitGetAsEntry() : null))
    .filter(Boolean);
  if (!entries.length) return [...dataTransfer.files].map(file => ({ file, path: file.name }));
  const out = [];
  const walk = async (entry, prefix) => {
    if (entry.isFile) {
      const file = await new Promise((res, rej) => entry.file(res, rej));
      out.push({ file, path: prefix + entry.name });
    } else if (entry.isDirectory) {
      const reader = entry.createReader();
      for (;;) {
        const batch = await new Promise((res, rej) => reader.readEntries(res, rej));
        if (!batch.length) break;
        for (const child of batch) await walk(child, prefix + entry.name + '/');
      }
    }
  };
  for (const e of entries) await walk(e, '');
  return out;
}

async function uploadAll(pack, items, ui, area, reload) {
  const existing = new Map(pack.files.map(f => [f.path, f]));
  const jobs = items
    .map(({ file, path }) => {
      let rel = path.replace(/\\/g, '/');
      if (ui.strip && rel.includes('/')) rel = rel.slice(rel.indexOf('/') + 1);
      return { file, path: joinPath(ui.target, rel) };
    })
    .filter(j => !JUNK.test(j.path));
  if (!jobs.length) return;

  const bar = h('div');
  const label = h('span', { class: 'muted small' });
  const errors = h('div', { class: 'uploads' });
  area.replaceChildren(h('div', { class: 'progress' }, bar), label, errors);
  const total = jobs.reduce((s, j) => s + j.file.size, 0) || 1;
  const loaded = new Map();
  let done = 0, skipped = 0, failed = 0;
  const update = () => {
    const sum = [...loaded.values()].reduce((s, n) => s + n, 0);
    bar.style.width = Math.min(100, (sum / total) * 100) + '%';
    label.textContent = `${done + skipped + failed} von ${jobs.length} Dateien · ${fmtBytes(sum)} von ${fmtBytes(total)}`;
  };
  update();

  const queue = [...jobs];
  const worker = async () => {
    for (let job = queue.shift(); job; job = queue.shift()) {
      const known = existing.get(job.path);
      // Unveränderte Dateien nicht erneut hochladen
      if (known && known.size === job.file.size && await sha1Hex(job.file) === known.sha1) {
        skipped++;
        loaded.set(job, job.file.size);
        update();
        continue;
      }
      const query = `?path=${encodeURIComponent(job.path)}${ui.keep ? '&overwrite=false' : ''}`;
      try {
        await upload(`/packs/${encodeURIComponent(pack.id)}/files${query}`, job.file, n => {
          loaded.set(job, n);
          update();
        });
        loaded.set(job, job.file.size);
        done++;
      } catch (e) {
        failed++;
        errors.append(h('div', { class: 'line' }, h('span', { class: 'path' }, job.path), h('span', { class: 'badge red' }, e.message)));
      }
      update();
    }
  };
  await Promise.all(Array.from({ length: PARALLEL_UPLOADS }, worker));
  const parts = [`${done} hochgeladen`];
  if (skipped) parts.push(`${skipped} unverändert`);
  if (failed) parts.push(`${failed} fehlgeschlagen`);
  toast(parts.join(' · '), failed ? 'error' : 'ok');
  if (!failed) area.replaceChildren();
  await reload();
}

function filesPanel(pack, reload) {
  const ui = packUi(pack.id);
  const id = encodeURIComponent(pack.id);
  const area = h('div');
  const start = async items => {
    if (!items.length) return;
    await uploadAll(pack, items, ui, area, reload);
  };

  const pickFiles = h('input', {
    type: 'file', multiple: true, class: 'hidden',
    onchange: e => start([...e.target.files].map(file => ({ file, path: file.name }))),
  });
  const pickDir = h('input', {
    type: 'file', multiple: true, class: 'hidden', webkitdirectory: true,
    onchange: e => start([...e.target.files].map(file => ({ file, path: file.webkitRelativePath || file.name }))),
  });
  const target = h('input', {
    type: 'text', value: ui.target, list: 'folders', placeholder: '(Spielordner)',
    oninput: e => { ui.target = e.target.value; },
  });
  const drop = h('div', { class: 'drop' },
    h('div', { class: 'grow' },
      h('div', {}, 'Dateien oder Ordner hierher ziehen'),
      h('div', { class: 'hint small' }, 'Gleiche Dateien werden übersprungen. Neue Dateien landen im Zielordner.')),
    h('label', { class: 'field' }, h('span', {}, 'Zielordner'), target,
      h('datalist', { id: 'folders' }, ['mods', 'config', 'resourcepacks', 'shaderpacks', 'kubejs', ''].map(f => h('option', { value: f })))),
    h('div', { class: 'stack' },
      h('label', { class: 'check small' }, h('input', { type: 'checkbox', checked: ui.keep, onchange: e => { ui.keep = e.target.checked; } }), 'als Vorgabedatei'),
      h('label', { class: 'check small' }, h('input', { type: 'checkbox', checked: ui.strip, onchange: e => { ui.strip = e.target.checked; } }), 'obersten Ordner weglassen')),
    h('div', { class: 'row' },
      h('button', { onclick: () => pickFiles.click() }, 'Dateien wählen'),
      h('button', { onclick: () => pickDir.click() }, 'Ordner wählen')),
    pickFiles, pickDir);
  drop.addEventListener('dragover', e => { e.preventDefault(); drop.classList.add('over'); });
  drop.addEventListener('dragleave', () => drop.classList.remove('over'));
  drop.addEventListener('drop', e => {
    e.preventDefault();
    drop.classList.remove('over');
    collectDrop(e.dataTransfer).then(start).catch(err => toast(err.message, 'error'));
  });

  // Auswahl auf vorhandene Dateien begrenzen
  const paths = new Set(pack.files.map(f => f.path));
  for (const p of [...ui.selected]) if (!paths.has(p)) ui.selected.delete(p);

  const tbody = h('tbody');
  const selInfo = h('span', { class: 'muted small' });
  const actions = h('div', { class: 'row' });
  const renderRows = () => {
    const q = ui.filter.trim().toLowerCase();
    const files = pack.files.filter(f => !q || f.path.toLowerCase().includes(q));
    const groups = new Map();
    for (const f of files) {
      const top = f.path.includes('/') ? f.path.slice(0, f.path.indexOf('/')) : '';
      if (!groups.has(top)) groups.set(top, []);
      groups.get(top).push(f);
    }
    const rows = [];
    for (const [folder, list] of [...groups].sort((a, b) => a[0].localeCompare(b[0]))) {
      const all = list.every(f => ui.selected.has(f.path));
      rows.push(h('tr', { class: 'folder' },
        h('td', {}, h('input', {
          type: 'checkbox', checked: all, 'aria-label': `${folder || 'Spielordner'} auswählen`,
          onchange: e => { for (const f of list) e.target.checked ? ui.selected.add(f.path) : ui.selected.delete(f.path); renderRows(); },
        })),
        h('td', { colspan: 4 }, `${folder ? folder + '/' : '(Spielordner)'} · ${list.length} Dateien · ${fmtBytes(list.reduce((s, f) => s + f.size, 0))}`)));
      for (const f of list) {
        rows.push(h('tr', {},
          h('td', {}, h('input', {
            type: 'checkbox', checked: ui.selected.has(f.path), 'aria-label': `${f.path} auswählen`,
            onchange: e => { e.target.checked ? ui.selected.add(f.path) : ui.selected.delete(f.path); renderRows(); },
          })),
          h('td', { class: 'path' }, h('span', { class: 'mono' }, f.path), ' ',
            f.status === 'new' ? badge('neu', 'green') : f.status === 'changed' ? badge('geändert', 'blue') : null),
          h('td', {}, h('label', { class: 'check small', title: 'Vorgabedatei: wird beim Spieler nur angelegt, wenn sie fehlt – danach gehört sie ihm.' },
            h('input', {
              type: 'checkbox', checked: !f.overwrite,
              onchange: e => busy(e.target, async () => {
                await api('POST', `/packs/${id}/files/overwrite`, { paths: [f.path], overwrite: !e.target.checked });
                await reload();
              }),
            }), 'Vorgabe')),
          h('td', { class: 'num nowrap' }, fmtBytes(f.size)),
          h('td', { class: 'num' }, h('button', {
            class: 'ghost small icon', title: 'Entfernen', 'aria-label': `${f.path} entfernen`,
            onclick: e => busy(e.target, async () => {
              await api('POST', `/packs/${id}/files/delete`, { paths: [f.path] });
              await reload();
            }),
          }, '✕'))));
      }
    }
    if (!q) {
      for (const p of pack.removedFiles || []) {
        rows.push(h('tr', { class: 'removed' }, h('td'), h('td', { class: 'path mono' }, p), h('td', {}, badge('entfernt', 'red')), h('td'), h('td')));
      }
    }
    if (!rows.length) {
      rows.push(h('tr', {}, h('td', { colspan: 5 }, h('div', { class: 'empty' }, q ? 'Keine Treffer.' : 'Noch keine Dateien – zieh deinen mods-Ordner hier oben hinein.'))));
    }
    tbody.replaceChildren(...rows);

    const sel = [...ui.selected];
    selInfo.textContent = sel.length ? `${sel.length} ausgewählt` : `${pack.files.length} Dateien · ${fmtBytes(pack.files.reduce((s, f) => s + f.size, 0))}`;
    actions.replaceChildren(...(sel.length ? [
      h('button', {
        class: 'small',
        onclick: e => busy(e.target, async () => {
          await api('POST', `/packs/${id}/files/overwrite`, { paths: sel, overwrite: false });
          await reload();
        }),
      }, 'Als Vorgabe'),
      h('button', {
        class: 'small',
        onclick: e => busy(e.target, async () => {
          await api('POST', `/packs/${id}/files/overwrite`, { paths: sel, overwrite: true });
          await reload();
        }),
      }, 'Immer aktualisieren'),
      sel.length === 1 ? h('button', {
        class: 'small',
        onclick: async () => {
          const v = await ask({ title: 'Umbenennen / verschieben', fields: [{ name: 'to', label: 'Neuer Pfad', value: sel[0] }], ok: 'Übernehmen' });
          if (!v) return;
          await busy(null, async () => {
            await api('POST', `/packs/${id}/files/move`, { from: sel[0], to: v.to });
            ui.selected.clear();
            await reload();
          });
        },
      }, 'Umbenennen') : null,
      h('button', {
        class: 'small danger',
        onclick: async e => {
          if (!await confirmAction('Dateien entfernen?', `${sel.length} Datei(en) aus dem Entwurf entfernen? Bei Spielern verschwinden sie mit der nächsten Version.`, 'Entfernen')) return;
          await busy(e.target, async () => {
            await api('POST', `/packs/${id}/files/delete`, { paths: sel });
            ui.selected.clear();
            await reload();
          });
        },
      }, 'Entfernen'),
      h('button', { class: 'small ghost', onclick: () => { ui.selected.clear(); renderRows(); } }, 'Auswahl aufheben'),
    ] : [
      h('button', {
        class: 'small ghost',
        onclick: async () => {
          const v = await ask({
            title: 'Ordner umbenennen',
            text: 'Verschiebt alle Dateien eines Ordners, z. B. „mods/optional“ nach „mods“.',
            fields: [{ name: 'from', label: 'Ordner', placeholder: 'mods/alt' }, { name: 'to', label: 'Neuer Ordner', placeholder: 'mods/neu' }],
            ok: 'Verschieben',
          });
          if (!v) return;
          await busy(null, async () => {
            const r = await api('POST', `/packs/${id}/files/move`, { from: v.from, to: v.to });
            toast(`${r.moved} Datei(en) verschoben`, 'ok');
            await reload();
          });
        },
      }, 'Ordner umbenennen…'),
    ]));
  };
  renderRows();

  const filter = h('input', {
    type: 'text', value: ui.filter, placeholder: 'Dateien filtern…', 'aria-label': 'Dateien filtern',
    oninput: e => { ui.filter = e.target.value; renderRows(); },
  });

  return h('section', { class: 'panel' },
    h('header', {},
      h('h2', {}, 'Dateien'),
      h('span', { class: 'muted small' }, 'Pfade relativ zum Spielordner (.minecraft). Eigene Mods der Spieler werden nie angefasst.')),
    h('div', { class: 'body' }, drop, area),
    h('div', { class: 'toolbar' }, filter, selInfo, h('div', { class: 'grow' }), actions),
    h('table', {},
      h('thead', {}, h('tr', {}, h('th', {}), h('th', {}, 'Pfad'), h('th', {}, 'Art'), h('th', { class: 'num' }, 'Größe'), h('th'))),
      tbody));
}

// ----- Einstellungen -----

function settingsPanel(pack, reload) {
  const f = {
    name: h('input', { type: 'text', value: pack.name }),
    description: h('textarea', { rows: 2 }, pack.description || ''),
    minecraft: h('input', { type: 'text', value: pack.minecraft, placeholder: '1.21.4' }),
    loader: h('select', {}, LOADERS.map(([v, l]) => h('option', { value: v }, l))),
    loaderVersion: h('input', { type: 'text', value: pack.loader?.version || '', placeholder: '0.16.10' }),
    server: h('input', { type: 'text', value: pack.server || '', placeholder: 'play.example.net' }),
    memory: h('input', { type: 'number', min: 0, step: 256, value: pack.memory || '' , placeholder: 'Prism-Vorgabe' }),
    instanceId: h('input', { type: 'text', value: pack.instanceId || '', placeholder: pack.name }),
    listed: h('input', { type: 'checkbox', checked: pack.listed }),
  };
  f.loader.value = pack.loader?.type && pack.loader.type !== 'vanilla' ? pack.loader.type : '';
  const syncLoader = () => { f.loaderVersion.disabled = !f.loader.value; };
  f.loader.addEventListener('change', syncLoader);
  syncLoader();

  const save = h('button', {
    class: 'primary',
    onclick: e => busy(e.target, async () => {
      const detail = await api('PUT', '/packs/' + encodeURIComponent(pack.id), {
        name: f.name.value,
        description: f.description.value,
        minecraft: f.minecraft.value,
        loader: { type: f.loader.value, version: f.loaderVersion.value },
        server: f.server.value,
        memory: Number(f.memory.value) || 0,
        instanceId: f.instanceId.value,
        listed: f.listed.checked,
      });
      toast('Gespeichert', 'ok');
      await reload(detail);
    }),
  }, 'Speichern');

  const field = (label, input, hint, cls) => h('label', { class: 'field' + (cls ? ' ' + cls : '') }, h('span', {}, label), input, hint ? h('small', {}, hint) : null);
  return h('section', { class: 'panel' },
    h('header', {}, h('h2', {}, 'Einstellungen'), h('span', { class: 'muted small' }, 'Name, Version und Loader werden mit der nächsten Version wirksam – „im Launcher“ sofort.')),
    h('div', { class: 'body stack' },
      h('div', { class: 'grid3' },
        field('Name', f.name, null, 'span2'),
        field('Minecraft-Version', f.minecraft),
        field('Beschreibung', f.description, null, 'span3'),
        field('Loader', f.loader),
        field('Loader-Version', f.loaderVersion),
        field('Arbeitsspeicher (MB)', f.memory, 'Nur beim Anlegen der Instanz'),
        field('Server beim Start', f.server, '„Spielen“ verbindet direkt'),
        field('Instanz-Ordner', f.instanceId, 'Leer = Name des Packs. Nicht mehr ändern, wenn Spieler es installiert haben.', 'span2')),
      h('div', { class: 'row' },
        h('label', { class: 'check' }, f.listed, 'Im Launcher anzeigen'),
        h('div', { class: 'grow' }),
        save)));
}

// ----- Versionen -----

function versionsPanel(pack, reload) {
  const id = encodeURIComponent(pack.id);
  const rows = [...pack.versions].reverse().map(v => {
    const isCurrent = v.version === pack.current;
    return h('tr', {},
      h('td', { class: 'nowrap' }, h('span', { class: 'mono' }, v.version), ' ', isCurrent ? badge('ausgeliefert', 'green') : null),
      h('td', { class: 'nowrap muted' }, fmtDate(v.publishedAt)),
      h('td', { class: 'num' }, v.fileCount),
      h('td', { class: 'num nowrap' }, fmtBytes(v.totalSize)),
      h('td', { class: 'muted' }, v.notes || ''),
      h('td', { class: 'num' }, h('div', { class: 'row', }, h('div', { class: 'grow' }),
        isCurrent ? null : h('button', {
          class: 'small',
          title: 'Diese Version an Spieler ausliefern (z. B. Rollback)',
          onclick: async e => {
            if (!await confirmAction('Version ausliefern?', `Spieler bekommen beim nächsten Start wieder v${v.version}.`, 'Ausliefern')) return;
            await busy(e.target, async () => reload(await api('POST', `/packs/${id}/current`, { version: v.version })));
          },
        }, 'Ausliefern'),
        h('button', {
          class: 'small ghost',
          title: 'Entwurf durch diese Version ersetzen',
          onclick: async e => {
            if (!await confirmAction('In Entwurf laden?', `Der aktuelle Entwurf wird durch v${v.version} ersetzt.`, 'Laden')) return;
            await busy(e.target, async () => reload(await api('POST', `/packs/${id}/reset`, { version: v.version })));
          },
        }, 'In Entwurf'),
        isCurrent ? null : h('button', {
          class: 'small ghost icon', title: 'Version löschen', 'aria-label': `Version ${v.version} löschen`,
          onclick: async e => {
            if (!await confirmAction('Version löschen?', `v${v.version} endgültig löschen?`)) return;
            await busy(e.target, async () => reload(await api('DELETE', `/packs/${id}/versions/${encodeURIComponent(v.version)}`)));
          },
        }, '✕'))));
  });
  return h('section', { class: 'panel' },
    h('header', {}, h('h2', {}, 'Versionen')),
    rows.length
      ? h('table', {},
        h('thead', {}, h('tr', {}, h('th', {}, 'Version'), h('th', {}, 'Veröffentlicht'), h('th', { class: 'num' }, 'Dateien'), h('th', { class: 'num' }, 'Größe'), h('th', {}, 'Änderungen'), h('th'))),
        h('tbody', {}, rows))
      : h('div', { class: 'empty' }, 'Noch keine Version veröffentlicht.'));
}

// ---------- News ----------

async function viewNews(main) {
  const site = await api('GET', '/site');
  const list = h('div');
  const saveBtn = h('button', { class: 'primary', disabled: true }, 'Speichern');
  const markDirty = () => {
    saveBtn.disabled = false;
    window.onbeforeunload = () => true;
  };

  const render = () => {
    list.replaceChildren(...(site.news.length ? site.news.map((n, i) => {
      const bind = (key, el) => {
        el.addEventListener('input', () => { n[key] = el.value; markDirty(); });
        return el;
      };
      return h('div', { class: 'list-item' },
        h('div', { class: 'grid3' },
          h('label', { class: 'field span2' }, h('span', {}, 'Titel'), bind('title', h('input', { type: 'text', value: n.title }))),
          h('label', { class: 'field' }, h('span', {}, 'Datum'), bind('date', h('input', { type: 'date', value: n.date }))),
          h('label', { class: 'field span3' }, h('span', {}, 'Text'), bind('body', h('textarea', { rows: 3 }, n.body || ''))),
          h('label', { class: 'field' }, h('span', {}, 'Bild (https-URL, optional)'), bind('image', h('input', { type: 'url', value: n.image, placeholder: 'https://…' }))),
          h('label', { class: 'field span2' }, h('span', {}, 'Link (optional)'), bind('url', h('input', { type: 'url', value: n.url, placeholder: 'https://…' })))),
        h('div', { class: 'row', }, h('div', { class: 'grow' }),
          h('button', { class: 'small ghost', disabled: i === 0, onclick: () => { site.news.splice(i - 1, 0, site.news.splice(i, 1)[0]); markDirty(); render(); } }, '↑ Nach oben'),
          h('button', { class: 'small ghost', disabled: i === site.news.length - 1, onclick: () => { site.news.splice(i + 1, 0, site.news.splice(i, 1)[0]); markDirty(); render(); } }, '↓ Nach unten'),
          h('button', { class: 'small danger', onclick: () => { site.news.splice(i, 1); markDirty(); render(); } }, 'Entfernen')));
    }) : [h('div', { class: 'panel' }, h('div', { class: 'empty' }, 'Keine News – der Launcher zeigt dann die eingebauten.'))]));
  };
  render();

  saveBtn.addEventListener('click', e => busy(e.target, async () => {
    Object.assign(site, await api('PUT', '/site', site));
    saveBtn.disabled = true;
    window.onbeforeunload = null;
    toast('News gespeichert – Launcher zeigen sie beim nächsten Abruf.', 'ok');
    render();
  }));

  main.replaceChildren(
    head('News', 'Erscheinen auf der Startseite des Launchers, oberste zuerst.',
      h('button', { onclick: () => { site.news.unshift({ title: '', date: today(), body: '', image: '', url: '' }); markDirty(); render(); } }, '+ News'),
      saveBtn),
    list);
}

// ---------- Launcher: Branding, Server ----------

async function viewLauncher(main) {
  const site = await api('GET', '/site');
  const saveBtn = h('button', { class: 'primary', disabled: true }, 'Speichern');
  const markDirty = () => {
    saveBtn.disabled = false;
    window.onbeforeunload = () => true;
  };
  const bind = (obj, key, el) => {
    el.addEventListener('input', () => { obj[key] = el.value; markDirty(); });
    return el;
  };

  const accentPicker = h('input', { type: 'color', value: /^#[0-9a-f]{6}$/i.test(site.brand.accent) ? site.brand.accent : '#ffd43f', 'aria-label': 'Akzentfarbe wählen' });
  const accentText = bind(site.brand, 'accent', h('input', { type: 'text', value: site.brand.accent, placeholder: 'eingebaut' }));
  accentPicker.addEventListener('input', () => { accentText.value = accentPicker.value; site.brand.accent = accentPicker.value; markDirty(); });

  const servers = h('tbody');
  const renderServers = () => {
    servers.replaceChildren(...(site.servers.length ? site.servers.map((s, i) => h('tr', {},
      h('td', {}, bind(s, 'name', h('input', { type: 'text', value: s.name, placeholder: 'Survival', 'aria-label': 'Name' }))),
      h('td', {}, bind(s, 'address', h('input', { type: 'text', value: s.address, placeholder: 'play.example.net', 'aria-label': 'Adresse' }))),
      h('td', {}, bind(s, 'instance', h('input', { type: 'text', value: s.instance, placeholder: 'optional', 'aria-label': 'Prism-Instanz' }))),
      h('td', { class: 'num' }, h('button', { class: 'ghost small icon', 'aria-label': 'Server entfernen', onclick: () => { site.servers.splice(i, 1); markDirty(); renderServers(); } }, '✕')))) : [
      h('tr', {}, h('td', { colspan: 4 }, h('div', { class: 'empty' }, 'Keine Server – der Launcher zeigt dann die eingebauten.'))),
    ]));
  };
  renderServers();

  saveBtn.addEventListener('click', e => busy(e.target, async () => {
    Object.assign(site, await api('PUT', '/site', site));
    saveBtn.disabled = true;
    window.onbeforeunload = null;
    toast('Gespeichert – Launcher übernehmen es beim nächsten Abruf.', 'ok');
  }));

  main.replaceChildren(
    head('Launcher', 'Leere Felder lassen die im Launcher eingebauten Werte stehen.', saveBtn),
    h('section', { class: 'panel' },
      h('header', {}, h('h2', {}, 'Branding')),
      h('div', { class: 'body grid3' },
        h('label', { class: 'field' }, h('span', {}, 'Name'), bind(site.brand, 'name', h('input', { type: 'text', value: site.brand.name, placeholder: 'eingebaut' }))),
        h('label', { class: 'field' }, h('span', {}, 'Untertitel'), bind(site.brand, 'tagline', h('input', { type: 'text', value: site.brand.tagline, placeholder: 'eingebaut' }))),
        h('label', { class: 'field' }, h('span', {}, 'Akzentfarbe'), h('div', { class: 'row' }, accentPicker, h('div', { class: 'grow' }, accentText))))),
    h('section', { class: 'panel' },
      h('header', {},
        h('h2', {}, 'Server'),
        h('span', { class: 'muted small grow' }, 'Mit Online-Status auf der Startseite. „Instanz“ = Prism-Ordner für den Beitreten-Knopf.'),
        h('button', { class: 'small', onclick: () => { site.servers.push({ name: '', address: '', instance: '' }); markDirty(); renderServers(); } }, '+ Server')),
      h('table', {},
        h('thead', {}, h('tr', {}, h('th', {}, 'Name'), h('th', {}, 'Adresse'), h('th', {}, 'Instanz'), h('th'))),
        servers)));
}

// ---------- Launcher-Updates ----------

async function viewUpdates(main) {
  renderUpdates(main, await api('GET', '/releases'));
}

function renderUpdates(main, data) {
  const render = d => renderUpdates(main, d);
  const current = data.list.find(r => r.version === data.current);
  const highest = data.list.length ? data.list[0].version : '';

  const file = h('input', { type: 'file', accept: '.exe,application/vnd.microsoft.portable-executable' });
  const version = h('input', { type: 'text', placeholder: '0.3.0' });
  const notes = h('textarea', { rows: 2, placeholder: 'Was ist neu?' });
  const activate = h('input', { type: 'checkbox', checked: true });
  const bar = h('div');
  const progress = h('div', { class: 'progress hidden' }, bar);
  file.addEventListener('change', () => {
    const m = file.files[0]?.name.match(/(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)/);
    if (m && !version.value) version.value = m[1];
  });
  const uploadBtn = h('button', {
    class: 'primary',
    onclick: e => busy(e.target, async () => {
      const f = file.files[0];
      if (!f) throw new Error('Bitte den signierten Installer wählen.');
      const v = version.value.trim();
      if (!v) throw new Error('Version fehlt.');
      if (highest && compareVersions(v, highest) <= 0 &&
        !await confirmAction('Version ist nicht höher', `Launcher installieren nur Versionen über ihrer eigenen. ${v} ist nicht höher als ${highest}. Trotzdem hochladen?`, 'Hochladen')) return;
      progress.classList.remove('hidden');
      const q = new URLSearchParams({ version: v, notes: notes.value, file: f.name, activate: activate.checked });
      const d = await upload('/releases?' + q, f, (n, t) => { bar.style.width = (n / (t || 1)) * 100 + '%'; });
      toast(activate.checked ? `Launcher ${v} wird jetzt angeboten.` : `Launcher ${v} hochgeladen.`, 'ok');
      render(d);
    }),
  }, 'Hochladen');

  const rows = data.list.map(r => {
    const isCurrent = r.version === data.current;
    return h('tr', {},
      h('td', { class: 'nowrap' }, h('span', { class: 'mono' }, r.version), ' ', isCurrent ? badge('angeboten', 'green') : null),
      h('td', { class: 'nowrap muted' }, r.pubDate),
      h('td', { class: 'num nowrap' }, fmtBytes(r.size)),
      h('td', { class: 'mono muted small', title: r.sha256 }, r.sha256.slice(0, 12) + '…'),
      h('td', { class: 'muted' }, r.notes),
      h('td', { class: 'num' }, h('div', { class: 'row' }, h('div', { class: 'grow' }),
        isCurrent
          ? h('button', {
            class: 'small',
            onclick: async e => {
              if (!await confirmAction('Kein Update mehr anbieten?', 'update.json antwortet dann mit 404, Launcher melden „kein Update“.', 'Zurückziehen')) return;
              await busy(e.target, async () => render(await api('POST', '/releases/current', { version: null })));
            },
          }, 'Zurückziehen')
          : h('button', {
            class: 'small',
            onclick: e => busy(e.target, async () => render(await api('POST', '/releases/current', { version: r.version }))),
          }, 'Anbieten'),
        h('button', {
          class: 'small ghost',
          onclick: async () => {
            const v = await ask({ title: `Notizen zu ${r.version}`, fields: [{ name: 'notes', label: 'Änderungen', type: 'textarea', value: r.notes }], ok: 'Speichern' });
            if (v) await busy(null, async () => render(await api('PUT', '/releases/' + encodeURIComponent(r.version), { notes: v.notes })));
          },
        }, 'Notizen'),
        isCurrent ? null : h('button', {
          class: 'small ghost icon', 'aria-label': `Version ${r.version} löschen`,
          onclick: async e => {
            if (!await confirmAction('Installer löschen?', `${r.file} (${r.version}) vom Server löschen?`)) return;
            await busy(e.target, async () => render(await api('DELETE', '/releases/' + encodeURIComponent(r.version))));
          },
        }, '✕'))));
  });

  main.replaceChildren(
    head('Launcher-Updates', 'Signierten Installer hochladen – Launcher finden ihn über update.json.'),
    h('section', { class: 'panel' },
      h('header', {}, h('h2', {}, 'Aktuell angeboten'), current ? badge('v' + current.version, 'green') : badge('nichts')),
      h('div', { class: 'body stack' },
        h('dl', { class: 'kv' },
          h('dt', {}, 'Feed'), h('dd', {}, copyField(state.status.updateUrl)),
          current ? [h('dt', {}, 'Installer'), h('dd', { class: 'mono small' }, `${current.file} · ${fmtBytes(current.size)}`)] : null),
        h('p', { class: 'muted small' },
          'Der Launcher nimmt ein Update nur an, wenn die Version höher ist, die SHA-256 passt und der Installer mit eurem Zertifikat signiert ist. ',
          'Die Feed-Adresse muss als update.feed in launcher.config.json eingebaut sein – sie lässt sich aus Sicherheitsgründen nicht aus der Ferne ändern.'))),
    h('section', { class: 'panel' },
      h('header', {}, h('h2', {}, 'Neue Version hochladen')),
      h('div', { class: 'body stack' },
        h('div', { class: 'grid3' },
          h('label', { class: 'field span2' }, h('span', {}, 'Installer (MyLauncher_x.y.z_x64-setup.exe aus dist\\)'), file),
          h('label', { class: 'field' }, h('span', {}, 'Version'), version),
          h('label', { class: 'field span3' }, h('span', {}, 'Änderungen'), notes)),
        progress,
        h('div', { class: 'row' }, h('label', { class: 'check' }, activate, 'Sofort anbieten'), h('div', { class: 'grow' }), uploadBtn))),
    h('section', { class: 'panel' },
      h('header', {}, h('h2', {}, 'Alle Versionen')),
      rows.length
        ? h('table', {},
          h('thead', {}, h('tr', {}, h('th', {}, 'Version'), h('th', {}, 'Datum'), h('th', { class: 'num' }, 'Größe'), h('th', {}, 'SHA-256'), h('th', {}, 'Änderungen'), h('th'))),
          h('tbody', {}, rows))
        : h('div', { class: 'empty' }, 'Noch kein Installer hochgeladen.')));
}

// ---------- System ----------

async function viewSystem(main) {
  const s = state.status = await api('GET', '/status');
  const snippet = JSON.stringify({ remoteConfig: s.configUrl, update: { feed: s.updateUrl, signerThumbprint: '…' } }, null, 2);
  main.replaceChildren(
    head('System', `mylauncher-server ${s.version}`),
    h('section', { class: 'panel' },
      h('header', {}, h('h2', {}, 'Launcher anbinden')),
      h('div', { class: 'body stack' },
        h('p', { class: 'muted' }, 'Diese Adressen in launcher.config.json eintragen und den Launcher neu bauen. Danach holt er Modpacks, News und Server von hier.'),
        h('dl', { class: 'kv' },
          h('dt', {}, 'Konfiguration'), h('dd', {}, copyField(s.configUrl)),
          h('dt', {}, 'Update-Feed'), h('dd', {}, copyField(s.updateUrl))),
        h('pre', { class: 'list-item mono small' }, snippet),
        s.publicUrl.startsWith('http://') && !/localhost|127\.0\.0\.1/.test(s.publicUrl)
          ? h('p', { class: 'badge red' }, 'Ohne HTTPS gehen Passwort und Downloads unverschlüsselt durchs Netz – Reverse-Proxy mit HTTPS davorschalten.')
          : null)),
    h('section', { class: 'panel' },
      h('header', {}, h('h2', {}, 'Speicher')),
      h('div', { class: 'body row' },
        h('p', { class: 'grow' }, `${s.blobs.count} Dateien · ${fmtBytes(s.blobs.bytes)}`,
          h('br'), h('span', { class: 'muted small' }, 'Gleiche Dateien liegen nur einmal auf dem Server, auch über Packs und Versionen hinweg.')),
        h('button', {
          onclick: e => busy(e.target, async () => {
            const r = await api('POST', '/gc');
            toast(r.removed ? `${r.removed} ungenutzte Dateien gelöscht (${fmtBytes(r.bytes)})` : 'Nichts aufzuräumen', 'ok');
            await viewSystem(main);
          }),
        }, 'Ungenutzte Dateien löschen'))));
}

// ---------- Start ----------

async function boot() {
  try {
    state.status = await api('GET', '/status');
    state.authed = true;
    renderShell();
    await route();
  } catch (e) {
    state.authed = false;
    renderLogin(e.message === 'Nicht angemeldet' ? '' : e.message);
  }
}

let lastHash = location.hash;
window.addEventListener('hashchange', () => {
  if (location.hash === lastHash) return;
  if (window.onbeforeunload && !window.confirm('Ungespeicherte Änderungen verwerfen?')) {
    location.hash = lastHash; // zurück, ohne neu zu laden (gleicher Hash wird oben ignoriert)
    return;
  }
  lastHash = location.hash;
  window.onbeforeunload = null;
  route();
});
boot();
