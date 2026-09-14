'use strict';

const MODDIR = '/data/adb/modules/imsforge';
const CONFIG = '/data/adb/imsforge/carriers.json';
const PROBE = `sh ${MODDIR}/probe.sh`;
let cbId = 0;

function exec(cmd) {
  return new Promise((resolve) => {
    const key = `_imsforge_cb_${Date.now()}_${cbId++}`;
    let timer;
    const finish = (errno, stdout, stderr) => {
      clearTimeout(timer);
      delete window[key];
      resolve({ errno: Number(errno), stdout: stdout || '', stderr: stderr || '' });
    };
    window[key] = finish;
    timer = setTimeout(() => finish(1, '', 'The root command timed out. Refresh to check its result.'), 30000);
    try {
      if (typeof ksu === 'undefined' || !ksu.exec) throw new Error('Open this page from a root manager with WebUI support.');
      ksu.exec(cmd, '{}', key);
    } catch (error) { finish(1, '', String(error.message || error)); }
  });
}

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
};
const clone = (value) => JSON.parse(JSON.stringify(value));

function toast(message) {
  if (typeof ksu !== 'undefined' && ksu.toast) {
    try { ksu.toast(message); return; } catch (_) { /* use the offline toast */ }
  }
  $('toast').textContent = message;
  $('toast').hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { $('toast').hidden = true; }, 2500);
}

function showBanner(text, actionLabel, onClick) {
  $('banner-text').textContent = text;
  $('banner-action').textContent = actionLabel || '';
  $('banner-action').hidden = !actionLabel;
  $('banner-action').onclick = onClick || null;
  $('banner').hidden = false;
}

function addRow(parent, label, value, cls = 'idle') {
  const row = el('div', 'row');
  row.append(el('div', 'label', label));
  const cell = el('div', 'value');
  cell.append(el('span', `pill ${cls}`, value));
  row.append(cell);
  parent.append(row);
}

const state = {
  config: { auto: true, carriers: [], skip: [] },
  saved: null, sims: [], status: {}, observations: {}, meta: 'unknown',
  expanded: new Set(), ready: false, dirty: false, busy: false, loadId: 0, errors: [],
};

// Preserve unknown keys so the Rust validator can reject them instead of silently dropping them.
function normalize(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Configuration must be an object.');
  const config = { ...value, auto: value.auto === undefined ? true : value.auto,
    carriers: value.carriers === undefined ? [] : value.carriers,
    skip: value.skip === undefined ? [] : value.skip };
  if (typeof config.auto !== 'boolean' || !Array.isArray(config.carriers) || !Array.isArray(config.skip)) {
    throw new Error('Expected a boolean auto and arrays carriers and skip.');
  }
  if (config.skip.some((name) => typeof name !== 'string') || config.carriers.some((carrier) =>
    !carrier || typeof carrier !== 'object' || Array.isArray(carrier) || typeof carrier.canonical_name !== 'string')) {
    throw new Error('Each carrier needs a canonical_name; skip must contain names.');
  }
  return config;
}

function sections(stdout) {
  const result = {};
  let key;
  for (const line of stdout.split('\n')) {
    if (line.startsWith('@@')) { key = line.slice(2).trim(); result[key] = []; }
    else if (key) result[key].push(line);
  }
  for (const name of Object.keys(result)) result[name] = result[name].join('\n').trim();
  return result;
}

function source(sections, name) {
  if (sections[`${name}_ok`] !== '0') throw new Error(`${name}: ${sections[`${name}_error`] || 'data unavailable'}`);
  return sections[name] || '';
}

function needsRestart() {
  const run = currentRun();
  return state.ready && (state.status.config_changed === true || !run);
}

function controls() {
  const pending = needsRestart();
  $('save').disabled = !state.ready || state.busy || !state.dirty;
  $('reboot').disabled = !state.ready || state.busy || state.dirty;
  $('raw').disabled = !state.ready || state.busy;
  $('discard').disabled = state.busy || !state.dirty;
  $('refresh').disabled = state.busy;
  $('raw-format').disabled = !state.ready || state.busy;
  $('action-bar').hidden = !state.dirty && !pending;
  $('save').hidden = !state.dirty;
  $('discard').hidden = !state.dirty;
  $('reboot').hidden = state.dirty || !pending;
  $('action-note').textContent = state.dirty ? 'Changes are not saved' : 'Saved settings need a restart';
  document.body.classList.toggle('has-actions', !$('action-bar').hidden);
}

async function loadAll(discard = false) {
  if (state.busy) return;
  if (state.dirty && !discard) { toast('Save or discard your changes before refreshing.'); return; }
  const id = ++state.loadId;
  state.busy = true;
  state.ready = false;
  controls();
  try {
    const result = await exec(PROBE);
    if (id !== state.loadId) return;
    if (result.errno !== 0) throw new Error(result.stderr || 'Could not read module state.');
    const data = sections(result.stdout);
    const config = normalize(JSON.parse(source(data, 'config')));
    const detected = JSON.parse(source(data, 'detect'));
    const status = JSON.parse(source(data, 'status'));
    if (!status || typeof status !== 'object' || Array.isArray(status)) throw new Error('Invalid status response.');
    if (!Array.isArray(detected.sims) || detected.sims.some((sim) => !sim || !Number.isInteger(sim.slot) || sim.slot < 0)) {
      throw new Error('SIM inventory does not contain valid slot IDs.');
    }
    state.config = config;
    state.saved = clone(config);
    state.sims = detected.sims;
    state.status = status;
    state.dirty = false;
    state.errors = [];
    state.meta = 'unknown';
    try { state.meta = source(data, 'meta'); } catch (e) { state.errors.push(e.message); }
    state.observations = {};
    for (const name of ['radio', 'carrier', 'ims']) {
      try {
        for (const match of source(data, name).matchAll(/^(pcscf|vops|volte|ims|voice|last_transport) (\d+) (yes|no|true|false|wifi|cellular|\d+)$/gm)) {
          const slot = Number(match[2]);
          state.observations[slot] ||= {};
          state.observations[slot][match[1]] = match[3];
        }
      } catch (e) { state.errors.push(e.message); }
    }
    if (!detected.complete) state.errors.push('SIM inventory is still settling; some slots may be missing.');
    $('checked-at').textContent = `Last checked: ${new Date().toLocaleTimeString()}`;
    $('version').textContent = (data.version || '').match(/^version=(.*)$/m)?.[1] || '';
    $('log').textContent = data.log_ok === '0' ? data.log : 'Boot log unavailable.';
    state.ready = true;
    syncRaw();
    renderSims();
    renderStatus();
  } catch (error) {
    setHealth('bad', 'Could not refresh', 'Previously read data may be out of date.');
    showBanner(`Could not refresh: ${error.message}. Saving is disabled until a successful refresh.`, 'Retry', () => loadAll(discard));
  } finally {
    state.busy = false;
    controls();
    renderSims();
  }
}

function currentRun() {
  const run = state.status.run;
  return state.status.current_boot && run?.format === 2 ? run : null;
}

function carrierPlan(name) {
  if (!name) return { on: false, disabled: true, what: 'Unknown carrier — nothing to patch', tone: 'muted' };
  const sim = state.sims.find((item) => item.canonical_name === name);
  const explicit = state.config.carriers.some((carrier) => carrier.canonical_name === name);
  const on = !state.config.skip.includes(name) && (explicit || (state.config.auto && sim?.certified === false));
  const run = currentRun();
  const entry = run?.carriers?.find((c) => c.canonical_name === name);
  const patched = run?.phase === 'applied' && entry?.outcome === 'patched';
  let what, tone = 'muted';
  if (state.dirty) what = on ? 'Selected after save and restart' : 'Excluded after save and restart';
  else if (run?.phase === 'failed' && on) { what = 'Patch update failed'; tone = 'bad'; }
  else if (entry?.outcome === 'missing' && on) { what = 'Carrier settings entry not found'; tone = 'warn'; }
  else if (on) {
    if (patched && state.status.config_changed === false) { what = '✓ Applied on this boot'; tone = 'good'; }
    else { what = 'Enabled · restart to apply'; tone = 'warn'; }
  } else if (!explicit && !state.config.skip.includes(name) && sim?.certified === true) what = 'Stock settings already enable VoLTE';
  else if (state.config.auto && sim?.certified == null && !explicit && !state.config.skip.includes(name)) what = 'Stock settings unavailable · automatic decision unknown';
  else what = patched ? 'Disabled · restart to remove patch' : 'Excluded from patching';
  if (!state.ready) { what = 'Status may be out of date · refresh required'; tone = 'muted'; }
  return { on, disabled: !state.ready || state.busy, what, tone };
}

function imsSummary(observed) {
  if (observed.ims === '2') return { text: '✓ IMS registered', tone: 'good' };
  if (observed.ims === '1') return { text: 'IMS registration in progress', tone: 'muted' };
  if (observed.ims === '0') return { text: 'IMS not registered', tone: 'muted' };
  return { text: 'IMS status unavailable', tone: 'muted' };
}

function renderSims() {
  const focused = document.activeElement?.id;
  const box = $('sims');
  box.replaceChildren();
  if (!state.sims.length) { box.append(el('p', 'hint', state.ready ? 'No SIM reported.' : 'SIM inventory unavailable.')); return; }
  for (const sim of state.sims) {
    const plan = carrierPlan(sim.canonical_name);
    const card = el('div', 'sim');
    const head = el('div', 'sim-head');
    head.append(el('div', 'sim-name', sim.spn || sim.mccmnc || 'Unknown carrier'));
    head.append(el('div', 'sim-meta', `SIM ${sim.slot + 1}`));
    const control = el('div', 'sim-control');
    const label = el('label', '', 'Apply patch');
    const button = el('button', `switch${plan.on ? ' on' : ''}`);
    button.id = `patch-slot-${sim.slot}`;
    label.setAttribute('for', button.id);
    button.setAttribute('role', 'switch');
    button.setAttribute('aria-label', `Apply patch for ${sim.spn || sim.mccmnc}, SIM ${sim.slot + 1}`);
    button.setAttribute('aria-checked', String(plan.on));
    button.disabled = plan.disabled;
    button.onclick = () => toggleCarrier(sim.canonical_name, !plan.on);
    button.append(el('span', 'knob'));
    control.append(label, button);
    card.append(head, control, el('div', `sim-state ${plan.tone}`, plan.what));
    const observed = state.ready ? state.observations[sim.slot] || {} : {};
    const ims = imsSummary(observed);
    // A deliberately excluded SIM should not look like a module failure.
    if (plan.on || observed.ims === '2') card.append(el('div', `sim-ims ${ims.tone}`, ims.text));
    const detail = el('details');
    const detailKey = `sim-${sim.slot}`;
    detail.open = state.expanded.has(detailKey);
    detail.ontoggle = () => detail.open ? state.expanded.add(detailKey) : state.expanded.delete(detailKey);
    detail.append(el('summary', '', 'Connection details'));
    const rows = el('div', 'rows');
    addRow(rows, 'IMS registration', ims.text.replace('✓ ', ''), observed.ims === '2' ? 'ok' : 'idle');
    addRow(rows, 'Voice capability', observed.voice === 'true' ? 'available' : observed.voice === 'false' ? 'unavailable' : 'not reported', observed.ims === '2' && observed.voice === 'true' ? 'ok' : 'idle');
    addRow(rows, 'Last registration transport', observed.ims === '2' && observed.last_transport ? (observed.last_transport === 'wifi' ? 'Wi-Fi' : 'cellular') : 'not reported');
    addRow(rows, 'Android VoLTE setting', observed.volte === 'true' ? 'enabled' : observed.volte === 'false' ? 'disabled' : 'not reported', observed.volte === 'true' ? 'ok' : 'idle');
    addRow(rows, 'P-CSCF address', observed.pcscf === 'yes' ? 'observed' : observed.pcscf === 'no' ? 'not observed' : 'not reported');
    addRow(rows, 'Carrier ID', sim.canonical_name || sim.mccmnc || 'unknown');
    detail.append(rows, el('p', 'hint', 'Registration and voice capability do not verify a completed call. Last registration transport is a historical observation, not the current radio technology.'));
    if (observed.vops === '3') detail.append(el('p', 'hint', 'The mobile network reports VoPS unsupported. Wi-Fi calling is independent.'));
    card.append(detail);
    box.append(card);
  }
  if (focused?.startsWith('patch-slot-')) document.getElementById(focused)?.focus();
}

function setHealth(tone, title, note) {
  $('health').className = `health ${tone}`;
  $('health-title').textContent = title;
  $('health-note').textContent = note;
  $('health-symbol').textContent = tone === 'ok' ? '✓' : tone === 'bad' ? '!' : tone === 'warn' ? '↻' : '…';
}

function renderStatus() {
  const rows = $('status-rows');
  rows.replaceChildren();
  const run = currentRun();
  addRow(rows, 'Mount backend', state.meta, state.meta === 'missing' ? 'bad' : 'idle');
  const published = run?.phase === 'applied';
  const hasFiles = Object.keys(run?.files || {}).length > 0;
  addRow(rows, 'Files installed this boot', published ? (hasFiles ? 'yes' : 'no patch needed') : run?.phase || 'not confirmed', published ? 'ok' : run?.phase === 'failed' ? 'bad' : 'idle');
  addRow(rows, 'Expected files visible here', published && hasFiles ? (state.status.matches_run ? 'yes' : 'not observed') : 'not checked', published && hasFiles && state.status.matches_run ? 'ok' : 'idle');
  const missing = (run?.carriers || []).filter((c) => c.outcome === 'missing').map((c) => c.canonical_name);
  $('config-note').textContent = missing.length ? `No CarrierSettings entry for: ${missing.join(', ')}.` : '';
  $('config-note').hidden = !missing.length;
  $('banner').hidden = true;
  if (state.meta === 'missing') setHealth('bad', 'Mount backend missing', 'Install a mount backend to apply the patch.');
  else if (run?.phase === 'failed') {
    setHealth('bad', 'Patch update failed', 'Open module diagnostics for the boot error.');
    showBanner(`The boot update failed: ${run.error || 'see the boot log'}. Check the log before rebooting.`);
  } else if (run?.phase === 'preparing') setHealth('warn', 'Boot update incomplete', 'Check the boot log before restarting.');
  else if (state.dirty) setHealth('warn', 'Unsaved changes', 'Save first. Changes apply after a restart.');
  else if (needsRestart()) setHealth('warn', 'Restart to apply settings', 'Saved settings have not been applied on this boot.');
  else if (!published) setHealth('idle', 'Boot status unavailable', 'Refresh or inspect module diagnostics.');
  else if (missing.length) setHealth('warn', 'Some carriers could not be patched', 'Open module diagnostics for the missing entries.');
  else if (hasFiles && !state.status.matches_run) {
    setHealth('idle', 'Patch installed · visibility unconfirmed', 'This viewer does not see the expected files.');
    showBanner('Mount namespaces can differ. This does not establish what telephony reads.');
  } else setHealth('ok', hasFiles ? 'Patch applied' : 'No patch needed', 'Your saved settings are active.');
  if (state.errors.length && $('banner').hidden) showBanner('Some diagnostics are unavailable. Open module diagnostics for details.');
  $('module-summary').textContent = state.errors.length || missing.length || !published || state.meta === 'missing' ? 'Module diagnostics · attention needed' : 'Module diagnostics';
  $('diagnostic-errors').textContent = state.errors.join('\n');
  $('diagnostic-errors').hidden = !state.errors.length;
  controls();
}

function syncRaw() {
  $('raw').value = JSON.stringify(state.config, null, 2);
  $('raw-error').textContent = '';
}

function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(key => [key, stable(value[key])]));
  return value;
}

function markDirty() {
  try { state.dirty = JSON.stringify(stable(normalize(JSON.parse($('raw').value)))) !== JSON.stringify(stable(state.saved)); }
  catch (_) { state.dirty = true; }
  renderStatus();
}

function toggleCarrier(name, on) {
  if (!state.ready || state.busy) return;
  try {
    const config = normalize(JSON.parse($('raw').value));
    config.skip = config.skip.filter((item) => item !== name);
    if (on) {
      const sim = state.sims.find((item) => item.canonical_name === name);
      if ((!config.auto || sim?.certified !== false) && !config.carriers.some((c) => c.canonical_name === name)) config.carriers.push({ canonical_name: name });
    } else {
      // Exclusion takes precedence; keep custom overrides for the next enable.
      config.skip.push(name);
    }
    state.config = config;
    syncRaw();
    markDirty();
    renderSims();
  } catch (error) { $('raw-error').textContent = error.message; toast('Fix the JSON draft before changing a switch.'); }
}

async function save() {
  if (!state.ready || state.busy) return;
  let draft;
  try { draft = normalize(JSON.parse($('raw').value)); }
  catch (error) { $('raw-error').textContent = error.message; toast('Invalid configuration'); return; }
  const b64 = btoa(unescape(encodeURIComponent(JSON.stringify(draft))));
  state.busy = true;
  controls();
  renderSims();
  let savedOk = false;
  try {
    const result = await exec(`printf '%s' '${b64}' | base64 -d | ${MODDIR}/bin/imsforge save-config --config ${CONFIG}`);
    if (result.errno !== 0) {
      const error = new Error(result.stderr || 'Could not save. Refresh to check whether the command completed.');
      error.validation = /^imsforge: invalid configuration:/m.test(result.stderr || '');
      throw error;
    }
    const saved = normalize(JSON.parse(result.stdout));
    state.config = saved;
    state.saved = clone(saved);
    state.dirty = false;
    syncRaw();
    savedOk = true;
    toast('Saved');
  } catch (error) {
    $('raw-error').textContent = error.message;
    if (error.validation) { markDirty(); toast('Configuration rejected — correct the draft and save again.'); }
    else {
      state.ready = false;
      setHealth('bad', 'Save not confirmed', 'Refresh to check the saved configuration.');
      showBanner(`Save was not confirmed: ${error.message}`, 'Refresh', () => loadAll(true));
    }
  } finally { state.busy = false; controls(); renderSims(); }
  if (savedOk) await loadAll();
}

async function reboot() {
  if (!state.ready || state.busy || state.dirty) { toast('Save or discard changes first.'); return; }
  state.busy = true;
  controls();
  setHealth('idle', 'Restarting…', 'Wait for your phone to finish restarting.');
  showBanner('Rebooting…');
  const result = await exec('svc power reboot || reboot');
  if (result.errno !== 0) { state.busy = false; controls(); showBanner(result.stderr || 'Could not reboot.'); }
}

$('refresh').onclick = () => loadAll();
$('discard').onclick = () => loadAll(true);
$('save').onclick = save;
$('reboot').onclick = reboot;
$('raw').oninput = () => {
  markDirty();
  try { state.config = normalize(JSON.parse($('raw').value)); $('raw-error').textContent = ''; }
  catch (error) { $('raw-error').textContent = error.message; }
  renderSims();
};
$('raw-format').onclick = () => {
  try { $('raw').value = JSON.stringify(JSON.parse($('raw').value), null, 2); }
  catch (error) { $('raw-error').textContent = error.message; }
};
controls();
loadAll();
