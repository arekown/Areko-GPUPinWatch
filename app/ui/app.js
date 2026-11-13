'use strict';
// Areko GPUPinWatch window. All values come from the service snapshot (Tauri event "gpw-snapshot");
// rendering is taken from the accepted draft. Settings changes are sent back as SetConfig.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const PHASE = { ok:'ok', throttled:'thr', alarm:'alarm' };
const RANK = { ok:0, thr:1, alarm:2 };
const SRC = { astral_chip:'srcCard', wire_view:'srcWv', msi_psu:'srcPsu' };
const LVL = { info:'i', warn:'w', alarm:'b' };
const HOLD1 = [500, 1000, 2000, 5000], HOLD2 = [500, 1000, 1500, 3000], RATES = [1, 2, 4];
const TARGETS = [{ kind:'watts', value:350 }, { kind:'watts', value:400 }, { kind:'watts', value:450 }, { kind:'percent', value:75 }];
// After a local change the next snapshots may still carry the old config; do not reset the controls meanwhile.
const LOCAL_HOLD_MS = 2000;

const S = { connected:false, snap:null, theme:'areko', lang:'', textsLang:null, cfgKey:'', localUntil:0, evKey:'', devKey:'' };
const cardEls = new Map();

const $ = id => document.getElementById(id);
const esc = v => String(v ?? '').replace(/[&<>"']/g, c => ({ '&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;' }[c]));
const sysLang = () => { const l = (navigator.language || 'en').slice(0, 2).toLowerCase(); return I18N[l] ? l : 'en'; };
const lang = () => (S.lang && I18N[S.lang]) ? S.lang : sysLang();
const tr = (k, p) => { let s = (I18N[lang()][k] ?? I18N.de[k]); if (p) for (const n in p) s = s.replace('{' + n + '}', () => p[n]); return s; };
const fmtA = v => v.toFixed(2);
const nf = v => { const l = lang(); return l === 'de' || l === 'it' || l === 'es' || l === 'pt' ? String(v).replace('.', ',') : String(v); };
const css = n => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
const cfg = () => S.snap && S.snap.config;
// stage 2 set to "alarm only": no shutdown, the card can be released from the window
const alarmOnly = () => !!(cfg() && cfg().stage2.action === 'alarm_only');
const warnA = () => cfg() ? cfg().stage1.warn_a : 8.5;
const critA = () => cfg() ? cfg().stage2.crit_a : 9.2;

// ------------------------------------------------------------------ rendering (from the draft)
function plugSvg(vals) {
  // like the real 12V-2x6 plug: top row the six 12 V pins (measured), bottom row the six ground pins (no sensor sees them)
  const WARN = warnA(), CRIT = critA();
  const d = 50, g = 10, pad = 16, lab = 34, w = 6 * d + 5 * g + 2 * pad, h = 2 * d + g + 2 * pad;
  const latch = css('--plug'), bg = css('--plugBg'), dim = css('--dim'), mono = css('--mono');
  let s = `<svg viewBox="${-lab - 4} -14 ${w + lab + 8} ${h + 18}">`
    + `<rect x="${w / 2 - 40}" y="-10" width="80" height="12" rx="3" fill="none" stroke="${latch}" stroke-width="1.5"/>`
    + `<rect x="0" y="0" width="${w}" height="${h}" rx="${S.theme === 'areko' ? 2 : 10}" fill="${bg}" stroke="${latch}" stroke-width="1.5"/>`
    + `<text x="-8" y="${pad + d / 2 + 5}" text-anchor="end" font-family="${mono}" font-size="13" fill="${dim}">12V</text>`
    + `<text x="-8" y="${pad + d + g + d / 2 + 5}" text-anchor="end" font-family="${mono}" font-size="13" fill="${dim}">GND</text>`;
  for (let i = 0; i < 6; i++) {
    const a = vals ? vals[i] : null;
    const cx = pad + d / 2 + i * (d + g), cy = pad + d / 2, r = d / 2 - 3;
    s += `<circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="${css('--track')}" stroke-width="5"/>`;
    if (a == null) {
      s += `<text x="${cx}" y="${cy + 5}" text-anchor="middle" font-family="${mono}" font-size="14" font-weight="600" fill="${dim}">—</text>`;
    } else {
      const col = a >= CRIT ? css('--bad') : a >= WARN ? css('--warn') : css('--acc');
      const f = Math.max(0, Math.min(1, a / CRIT)), ang = -Math.PI / 2 + f * 2 * Math.PI;
      const x1 = cx, y1 = cy - r, x2 = cx + r * Math.cos(ang), y2 = cy + r * Math.sin(ang);
      s += (f >= .999 ? `<circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="${col}" stroke-width="5"/>`
          : f > 0 ? `<path d="M${x1} ${y1} A${r} ${r} 0 ${f > .5 ? 1 : 0} 1 ${x2.toFixed(2)} ${y2.toFixed(2)}" fill="none" stroke="${col}" stroke-width="5"/>` : '')
        + `<text x="${cx}" y="${cy + 5}" text-anchor="middle" font-family="${mono}" font-size="14" font-weight="600" fill="${a >= WARN ? col : css('--pinTxt')}">${fmtA(a)}</text>`;
    }
    const gy = cy + d + g;
    s += `<circle cx="${cx}" cy="${gy}" r="${r}" fill="none" stroke="${css('--track')}" stroke-width="3" stroke-dasharray="3 4"/>`
      + `<text x="${cx}" y="${gy + 4}" text-anchor="middle" font-family="${mono}" font-size="12" fill="${dim}">GND</text>`;
  }
  return s + '</svg>';
}
function histSvg(h) {
  const WARN = warnA(), CRIT = critA();
  const W = 400, H = 64, lo = 0, hi = 10, y = v => H - (v - lo) / (hi - lo) * H;
  const pts = h.length < 2 ? '' : h.map((v, i) => `${(i / (h.length - 1) * W).toFixed(1)},${y(v).toFixed(1)}`).join(' ');
  return `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none">`
    + `<line x1="0" x2="${W}" y1="${y(WARN)}" y2="${y(WARN)}" stroke="${css('--warn')}" stroke-dasharray="4 4" stroke-width="1" vector-effect="non-scaling-stroke"/>`
    + `<line x1="0" x2="${W}" y1="${y(CRIT)}" y2="${y(CRIT)}" stroke="${css('--bad')}" stroke-dasharray="4 4" stroke-width="1" vector-effect="non-scaling-stroke"/>`
    + `<polyline points="${pts}" fill="none" stroke="${css('--acc')}" stroke-width="1.6" vector-effect="non-scaling-stroke"/></svg>`;
}

// on-card sensor: the board name (e.g. ROG Astral RTX 5090 OC); external sources: the GPU they feed
const cardTitle = c => (c.source_kind === 'astral_chip' && c.source_label) || c.gpu_name || c.source_label;
const gpuName = pci => { const c = S.snap && S.snap.cards.find(x => x.gpu_pci === pci); return (c && c.gpu_name) || pci; };
const shortPci = p => p.replace(/^0000:/, '');

function cardSkeleton(id) {
  const el = document.createElement('div');
  el.dataset.id = id;
  el.innerHTML = '<div class="chead"><div><h3></h3><div class="sub"></div></div><span class="src"></span></div>'
    + '<div class="plugwrap"></div><div class="stats"></div><div class="histo"></div>'
    + '<div class="banner"><span class="grow"></span><button class="btn"></button></div>';
  return el;
}
// Card elements stay in place between snapshots so the alarm pulse runs on and "Release" stays clickable.
function fillCard(el, c) {
  const st = PHASE[c.phase] || 'ok', v = c.amps;
  const cls = 'card ' + st + (st === 'alarm' && alarmOnly() ? ' alarmonly' : '');
  if (el.className !== cls) el.className = cls;
  el.querySelector('h3').textContent = cardTitle(c);
  el.querySelector('.sub').textContent = c.binding === 'matching' ? tr('matching') : c.gpu_pci ? 'PCI ' + shortPci(c.gpu_pci) : '';
  el.querySelector('.src').textContent = tr(SRC[c.source_kind] || 'srcCard');
  el.querySelector('.plugwrap').innerHTML = plugSvg(v);
  let stats;
  if (v) {
    const tot = v.reduce((a, b) => a + b, 0), spread = Math.max(...v) - Math.min(...v);
    stats = [[tr('total'), nf(tot.toFixed(1)) + ' A', ''],
      [tr('power'), c.power_w != null ? Math.round(c.power_w) + ' W' : '—', ''],
      [tr('spread'), nf(spread.toFixed(2)) + ' A', spread > 2.5 ? 'bad' : spread > 1.2 ? 'warn' : '']];
  } else {
    stats = [[tr('total'), '—', ''], [tr('power'), c.power_w != null ? Math.round(c.power_w) + ' W' : '—', ''], [tr('spread'), '—', '']];
  }
  stats.push([tr('limit'), c.limit_w != null ? Math.round(c.limit_w) + ' W' : '—', st === 'ok' ? '' : 'warn']);
  el.querySelector('.stats').innerHTML = stats.map(([k, val, k2]) => `<div class="stat"><span>${esc(k)}</span><b class="${k2}">${val}</b></div>`).join('');
  el.querySelector('.histo').innerHTML = `<div class="lbl"><span>${esc(tr('hist'))}</span><span>${nf(warnA())} / ${nf(critA())} A</span></div>${histSvg(c.history_max || [])}`;
  let banner = '';
  if (st !== 'ok') {
    const [pin, amps] = c.trigger || [v ? v.indexOf(Math.max(...v)) : 0, v ? Math.max(...v) : 0];
    const a = nf(fmtA(amps)), p = pin + 1;
    banner = st === 'thr' ? tr('bThr', { w:c.limit_w != null ? Math.round(c.limit_w) : '—', p, a }) : tr(alarmOnly() ? 'bAlarmOnly' : 'bAlarm', { p, a });
  }
  el.querySelector('.banner .grow').textContent = banner;
  const btn = el.querySelector('.btn');
  btn.textContent = tr('release');
  btn.dataset.pci = c.gpu_pci || '';
}
function drawCards() {
  const box = $('cards');
  const note = !S.connected ? 'noService' : !S.snap.cards.length ? 'noCards' : null;
  if (note) {
    cardEls.clear();
    box.innerHTML = `<p class="note">${esc(tr(note))}</p>`;
    $('lastRead').textContent = '';
    return;
  }
  const cards = S.snap.cards, ids = cards.map(c => c.source_id);
  for (const id of [...cardEls.keys()]) if (!ids.includes(id)) cardEls.delete(id);
  cards.forEach(c => { if (!cardEls.has(c.source_id)) cardEls.set(c.source_id, cardSkeleton(c.source_id)); fillCard(cardEls.get(c.source_id), c); });
  const now = [...box.children].map(e => e.dataset.id);
  if (now.join('\n') !== ids.join('\n')) box.replaceChildren(...ids.map(id => cardEls.get(id)));
  $('lastRead').textContent = tr('last', { s:nf((S.snap.age_ms / 1000).toFixed(1)) });
}

function drawHead() {
  const pill = $('pill');
  if (!S.connected) {
    pill.style.display = ''; pill.className = 'pill off'; $('pillT').textContent = tr('noService'); $('svc').textContent = '';
    return;
  }
  const cards = S.snap.cards;
  pill.style.display = cards.length ? '' : 'none';
  const worst = cards.map(c => PHASE[c.phase] || 'ok').reduce((a, b) => RANK[b] > RANK[a] ? b : a, 'ok');
  pill.className = 'pill ' + worst;
  $('pillT').textContent = tr(worst);
  $('svc').textContent = tr('svc', { hz:S.snap.poll_hz, n:S.snap.sources.filter(s => s.ok).length });
}

function drawSources() {
  const src = S.snap ? S.snap.sources : [];
  const key = lang() + JSON.stringify(src);
  if (key === S.devKey) return;
  S.devKey = key;
  $('devs').innerHTML = src.map(d =>
    `<div class="dev"><span class="dot ${d.ok ? '' : 'off'}"></span><span class="grow">${esc(tr(SRC[d.kind] || 'srcCard'))}<br><small>${esc(d.label)}${d.detail ? ' · ' + esc(d.detail) : ''}</small></span><small>${esc(tr(d.ok ? 'found' : 'notFound'))}</small></div>`).join('');
}

function evText(k) {
  switch (k.event) {
    case 'started': return tr('eStart');
    case 'source_found': return tr('eFound', { s:k.label });
    case 'source_lost': return tr('eLost', { s:k.label });
    case 'matched': return tr('eMatched', { s:k.label, g:gpuName(k.gpu) });
    case 'throttled': return tr('eThr', { w:Math.round(k.target_w), p:k.pin + 1, a:nf(fmtA(k.amps)) });
    case 'released': return tr('eRel', { w:Math.round(k.limit_w) });
    case 'alarm': return tr(k.shutdown ? 'eAlarm' : 'eAlarmOnly', { p:k.pin + 1, a:nf(fmtA(k.amps)) });
    case 'error': return tr('eError', { m:k.message });
    default: return k.event;
  }
}
function drawEvents() {
  const ev = S.snap ? S.snap.events : [];
  const key = lang() + ev.length + ':' + (ev.length ? ev[ev.length - 1].ts_ms : 0);
  if (key === S.evKey) return;
  S.evKey = key;
  $('evt').innerHTML = ev.slice().reverse().map(e => {
    const l = LVL[e.level] || 'i', t = new Date(e.ts_ms).toTimeString().slice(0, 8);
    const msg = (e.gpu ? gpuName(e.gpu) + ' · ' : '') + evText(e.event);
    return `<tr><td class="t">${t}</td><td class="k"><span class="lvl ${l}">${esc(l === 'b' ? tr('alarm') : l === 'w' ? tr('thr') : 'INFO')}</span></td><td>${esc(msg)}</td></tr>`;
  }).join('');
}

// ------------------------------------------------------------------ settings
const secs = ms => nf(ms / 1000) + ' s';
const targetLabel = t => t.kind === 'percent' ? t.value + ' %' : t.value + ' W';
function fillSelect(sel, items, current, label) {
  const list = items.slice();
  if (!list.some(x => JSON.stringify(x) === JSON.stringify(current))) list.push(current);
  sel.innerHTML = list.map(x => `<option value="${esc(JSON.stringify(x))}"${JSON.stringify(x) === JSON.stringify(current) ? ' selected' : ''}>${esc(label(x))}</option>`).join('');
}
function drawSettings(force) {
  const c = cfg();
  if (!c) return;
  const key = lang() + JSON.stringify(c);
  if (!force && (key === S.cfgKey || Date.now() < S.localUntil)) return;
  S.cfgKey = key;
  $('warn').value = c.stage1.warn_a; $('warnV').textContent = nf(c.stage1.warn_a.toFixed(1)) + ' A';
  $('crit').value = c.stage2.crit_a; $('critV').textContent = nf(c.stage2.crit_a.toFixed(1)) + ' A';
  fillSelect($('hold1'), HOLD1, c.stage1.hold_ms, secs);
  fillSelect($('target'), TARGETS, c.stage1.target, targetLabel);
  fillSelect($('hold2'), HOLD2, c.stage2.hold_ms, secs);
  fillSelect($('actSel'), ['shutdown', 'alarm_only'], c.stage2.action, a => tr('acts')[a === 'shutdown' ? 0 : 1]);
  fillSelect($('rate'), RATES, c.poll_hz, hz => hz + ' Hz');
  $('swDesk').classList.toggle('on', c.desktop_alarm);
  $('swSound').classList.toggle('on', c.sound);
}
function drawLookSelects() {
  $('themeSel').innerHTML = THEMES.map(([v, n]) => `<option value="${v}"${v === S.theme ? ' selected' : ''}>${n}</option>`).join('');
  $('langSel').innerHTML = [['', tr('sysLang')]].concat(Object.keys(I18N).map(k => [k, I18N[k].name]))
    .map(([v, n]) => `<option value="${v}"${v === S.lang ? ' selected' : ''}>${esc(n)}</option>`).join('');
}
function sendConfig(change) {
  const c = cfg();
  if (!c) return;
  const next = JSON.parse(JSON.stringify(c));
  change(next);
  S.snap.config = next;
  S.localUntil = Date.now() + LOCAL_HOLD_MS;
  invoke('set_config', { config:next }).catch(e => console.error('set_config:', e));
}
function bindSettings() {
  const sel = (id, apply) => $(id).addEventListener('change', e => { const v = JSON.parse(e.target.value); sendConfig(c => apply(c, v)); });
  $('warn').addEventListener('input', e => { $('warnV').textContent = nf(Number(e.target.value).toFixed(1)) + ' A'; });
  $('crit').addEventListener('input', e => { $('critV').textContent = nf(Number(e.target.value).toFixed(1)) + ' A'; });
  $('warn').addEventListener('change', e => sendConfig(c => { c.stage1.warn_a = Number(e.target.value); }));
  $('crit').addEventListener('change', e => sendConfig(c => { c.stage2.crit_a = Number(e.target.value); }));
  sel('hold1', (c, v) => { c.stage1.hold_ms = v; });
  sel('target', (c, v) => { c.stage1.target = v; });
  sel('hold2', (c, v) => { c.stage2.hold_ms = v; });
  sel('actSel', (c, v) => { c.stage2.action = v; });
  sel('rate', (c, v) => { c.poll_hz = v; });
  $('swDesk').addEventListener('click', e => { const on = !e.currentTarget.classList.contains('on'); e.currentTarget.classList.toggle('on', on); sendConfig(c => { c.desktop_alarm = on; }); });
  $('swSound').addEventListener('click', e => { const on = !e.currentTarget.classList.contains('on'); e.currentTarget.classList.toggle('on', on); sendConfig(c => { c.sound = on; }); });
  $('swAuto').addEventListener('click', e => {
    const sw = e.currentTarget, on = !sw.classList.contains('on');
    invoke('autostart_set', { on }).then(() => sw.classList.toggle('on', on)).catch(err => console.error('autostart:', err));
  });
  $('themeSel').addEventListener('change', e => { S.theme = e.target.value; sendConfig(c => { c.theme = S.theme; }); applyLook(); });
  $('langSel').addEventListener('change', e => { S.lang = e.target.value; sendConfig(c => { c.lang = S.lang; }); applyLook(); });
  invoke('autostart_get').then(on => $('swAuto').classList.toggle('on', on)).catch(err => console.error('autostart:', err));
}

// ------------------------------------------------------------------ look, texts, sound
function applyLook() {
  document.documentElement.dataset.theme = S.theme;
  document.documentElement.lang = lang();
  document.querySelectorAll('[data-t]').forEach(e => { e.textContent = tr(e.dataset.t); });
  drawLookSelects();
  drawSettings(true);
  syncTexts();
  render();
}
// Templates for notifications and the tray (Rust side); follow language and the stage 2 action.
function syncTexts() {
  const key = lang() + (alarmOnly() ? '/alarm_only' : '/shutdown');
  if (S.textsLang === key) return;
  S.textsLang = key;
  const texts = { dec:nf('1.5').charAt(1) };
  for (const k of ['tThr', 'bThr', 'ok', 'thr', 'alarm', 'noService', 'noCards', 'quit']) texts[k] = tr(k);
  texts.tAlarm = tr(alarmOnly() ? 'tAlarmOnly' : 'tAlarm');
  texts.bAlarm = tr(alarmOnly() ? 'bAlarmOnly' : 'bAlarm');
  invoke('set_texts', { texts }).catch(e => console.error('set_texts:', e));
}
function render() {
  syncTexts();
  drawHead();
  drawCards();
  drawSources();
  drawEvents();
  drawSettings(false);
}

let audio = null;
function beep(alarm) {
  try {
    audio = audio || new AudioContext();
    const t0 = audio.currentTime, n = alarm ? 3 : 1;
    for (let i = 0; i < n; i++) {
      const o = audio.createOscillator(), g = audio.createGain();
      o.type = 'square'; o.frequency.value = alarm ? 880 : 660;
      g.gain.setValueAtTime(0.0001, t0 + i * 0.3);
      g.gain.exponentialRampToValueAtTime(0.25, t0 + i * 0.3 + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, t0 + i * 0.3 + 0.22);
      o.connect(g).connect(audio.destination);
      o.start(t0 + i * 0.3); o.stop(t0 + i * 0.3 + 0.25);
    }
  } catch (e) { console.error('sound:', e); }
}

function onSnapshot(snap) {
  const lookChanged = !S.snap || snap.config.theme !== S.theme || snap.config.lang !== S.lang;
  S.connected = true;
  S.snap = snap;
  if (lookChanged && Date.now() >= S.localUntil) {
    S.theme = snap.config.theme; S.lang = snap.config.lang;
    applyLook();
  } else {
    render();
  }
}

// ------------------------------------------------------------------ wiring
$('tabs').addEventListener('click', e => {
  const b = e.target.closest('button'); if (!b) return;
  document.querySelectorAll('#tabs button').forEach(x => x.classList.toggle('on', x === b));
  document.querySelectorAll('.page').forEach(p => p.classList.toggle('on', p.id === 'p-' + b.dataset.p));
});
$('cards').addEventListener('click', e => {
  const b = e.target.closest('.btn');
  if (!b || !b.dataset.pci) return;
  invoke('release', { pci:b.dataset.pci }).catch(err => console.error('release:', err));
});
document.querySelector('.about a.val').addEventListener('click', e => {
  e.preventDefault();
  invoke('open_repo').catch(err => console.error('open_repo:', err));
});
bindSettings();

(async () => {
  await listen('gpw-snapshot', e => onSnapshot(e.payload));
  await listen('gpw-service', e => { if (!e.payload) { S.connected = false; S.snap = null; render(); } });
  await listen('gpw-alert', e => { if (cfg() && cfg().sound) beep(e.payload.phase === 'alarm'); });
  const st = await invoke('get_state');
  if (st.connected && st.snapshot) onSnapshot(st.snapshot);
  else applyLook();
})();
