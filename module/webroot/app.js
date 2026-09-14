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
  ready: false, dirty: false, busy: false, loadId: 0, errors: [],
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

function controls() {
  $('save').disabled = !state.ready || state.busy;
  $('reboot').disabled = !state.ready || state.busy || state.dirty;
  $('raw').disabled = !state.ready || state.busy;
  $('discard').disabled = state.busy || !state.dirty;
  $('refresh').disabled = state.busy;
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
    for (const name of ['radio', 'carrier']) {
      try {
        for (const match of source(data, name).matchAll(/^(pcscf|vops|volte) (\d+) (yes|no|true|false|\d+)$/gm)) {
          const slot = Number(match[2]);
          state.observations[slot] ||= {};
          state.observations[slot][match[1]] = match[3];
        }
      } catch (e) { state.errors.push(e.message); }
    }
    if (!detected.complete) state.errors.push('SIM inventory is still settling; some slots may be missing.');
    $('version').textContent = (data.version || '').match(/^version=(.*)$/m)?.[1] || '';
    $('log').textContent = data.log_ok === '0' ? data.log : 'Boot log unavailable.';
    state.ready = true;
    syncRaw();
    renderSims();
    renderStatus();
  } catch (error) {
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
  if (!name) return { on: false, disabled: true, what: 'Unknown carrier — nothing to patch' };
  const sim = state.sims.find((item) => item.canonical_name === name);
  const explicit = state.config.carriers.some((carrier) => carrier.canonical_name === name);
  const on = !state.config.skip.includes(name) && (explicit || (state.config.auto && sim?.certified === false));
  const patched = currentRun()?.phase === 'applied' && currentRun().carriers.some((c) => c.canonical_name === name && c.outcome === 'patched');
  let what;
  if (state.dirty) what = on ? 'Will be enabled when saved' : 'Will be disabled when saved';
  else if (on) what = patched && state.status.config_changed === false ? 'Patch files installed on this boot' : 'Enabled in configuration — applies on reboot';
  else if (!explicit && !state.config.skip.includes(name) && sim?.certified === true) what = 'Google enables VoLTE in the stock settings';
  else if (state.config.auto && sim?.certified == null && !explicit && !state.config.skip.includes(name)) what = 'Stock settings unavailable — automatic decision unknown';
  else what = patched ? 'Disabled in configuration — reboot to remove the patch' : 'Not selected for patching';
  return { on, disabled: !state.ready || state.busy, what };
}

function renderSims() {
  const box = $('sims');
  box.replaceChildren();
  if (!state.sims.length) { box.append(el('p', 'hint', state.ready ? 'No SIM reported.' : 'SIM inventory unavailable.')); return; }
  for (const sim of state.sims) {
    const plan = carrierPlan(sim.canonical_name);
    const card = el('div', 'sim');
    const head = el('div', 'sim-head');
    const titles = el('div');
    titles.append(el('div', 'sim-name', sim.spn || sim.mccmnc));
    titles.append(el('div', 'sim-meta', `Slot ${sim.slot + 1} · ${sim.canonical_name || sim.mccmnc}`));
    head.append(titles);
    const button = el('button', `switch${plan.on ? ' on' : ''}`);
    button.setAttribute('role', 'switch');
    button.setAttribute('aria-checked', String(plan.on));
    button.disabled = plan.disabled;
    button.onclick = () => toggleCarrier(sim.canonical_name, !plan.on);
    button.append(el('span', 'knob'));
    head.append(button);
    card.append(head, el('div', 'sim-state', plan.what));
    const observed = state.observations[sim.slot] || {};
    card.append(el('div', 'sim-ims muted', observed.volte === undefined ? 'Reported VoLTE flag: unavailable' : `Reported VoLTE flag: ${observed.volte === 'true' ? 'enabled' : 'disabled'}`));
    const pcscf = observed.pcscf === 'yes' ? 'P-CSCF address observed' : observed.pcscf === 'no' ? 'No P-CSCF address observed' : 'P-CSCF observation unavailable';
    card.append(el('div', 'sim-ims muted', `${pcscf}. IMS registration is not verified by this check.`));
    if (observed.vops === '3') card.append(el('div', 'sim-ims warn', 'The current mobile network reports VoPS unsupported. Wi-Fi calling is a separate service.'));
    box.append(card);
  }
}

function renderStatus() {
  const rows = $('status-rows');
  rows.replaceChildren();
  const run = currentRun();
  addRow(rows, 'Mount backend', state.meta, state.meta === 'missing' ? 'bad' : 'idle');
  const published = run?.phase === 'applied';
  addRow(rows, 'Files installed this boot', published ? (run.files && Object.keys(run.files).length ? 'yes' : 'no patch needed') : run?.phase || 'not confirmed', published ? 'ok' : 'idle');
  addRow(rows, 'Expected files visible here', published && Object.keys(run.files || {}).length ? (state.status.matches_run ? 'yes' : 'not observed') : 'not checked');
  const missing = (run?.carriers || []).filter((c) => c.outcome === 'missing').map((c) => c.canonical_name);
  $('config-note').textContent = missing.length ? `No CarrierSettings entry for: ${missing.join(', ')}.` : '';
  $('config-note').hidden = !missing.length;
  $('banner').hidden = true;
  if (state.dirty) markDirty();
  else if (state.meta === 'missing') showBanner('No mount backend found. Install one before applying this module.');
  else if (run?.phase === 'failed') showBanner(`The boot update failed: ${run.error || 'see the boot log'}. Check the log before rebooting.`);
  else if (run?.phase === 'preparing') showBanner('The boot update did not record completion. Check the boot log.');
  else if (state.status.config_changed === true || !published) showBanner('The saved configuration has not been confirmed on this boot. Reboot to apply it.', 'Reboot', reboot);
  else if (!state.status.matches_run && Object.keys(run.files || {}).length) showBanner('Files were installed, but this viewer does not see them. Mount namespaces can differ; this does not establish what telephony reads.');
  $('diagnostic-errors').textContent = state.errors.join('\n');
  $('diagnostic-errors').hidden = !state.errors.length;
}

function syncRaw() {
  $('raw').value = JSON.stringify(state.config, null, 2);
  $('raw-error').textContent = '';
}

function markDirty() {
  state.dirty = true;
  controls();
  showBanner('Changes have not been saved.', 'Save', save);
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
      config.carriers = config.carriers.filter((c) => c.canonical_name !== name);
      config.skip.push(name);
    }
    state.config = config;
    markDirty();
    syncRaw();
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
      showBanner(`Save was not confirmed: ${error.message}`, 'Refresh', () => loadAll(true));
    }
  } finally { state.busy = false; controls(); renderSims(); }
  if (savedOk) await loadAll();
}

async function reboot() {
  if (!state.ready || state.busy || state.dirty) { toast('Save or discard changes first.'); return; }
  state.busy = true;
  controls();
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
