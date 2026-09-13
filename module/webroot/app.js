'use strict';

const MODDIR = '/data/adb/modules/imsforge';
const DATADIR = '/data/adb/imsforge';
const CONFIG = `${DATADIR}/carriers.json`;

let cbId = 0;

/** Run a shell command as root through the manager's bridge. */
function exec(cmd) {
  return new Promise((resolve) => {
    const key = `_imsforge_cb_${Date.now()}_${cbId++}`;
    window[key] = (errno, stdout, stderr) => {
      delete window[key];
      resolve({ errno, stdout: stdout || '', stderr: stderr || '' });
    };
    if (typeof ksu !== 'undefined' && ksu.exec) {
      try {
        ksu.exec(cmd, '{}', key);
      } catch (e) {
        delete window[key];
        resolve({ errno: 1, stdout: '', stderr: String(e && e.message) });
      }
    } else {
      resolve({ errno: 1, stdout: '', stderr: 'no root bridge: open this from the manager' });
    }
  });
}

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
};

function toast(msg) {
  if (typeof ksu !== 'undefined' && ksu.toast) {
    try { ksu.toast(msg); return; } catch (e) { /* fall through */ }
  }
  const t = $('toast');
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => { t.hidden = true; }, 2200);
}

function addRow(parent, label, valueNode) {
  const row = el('div', 'row');
  row.append(el('div', 'label', label));
  const v = el('div', 'value');
  v.append(valueNode);
  row.append(v);
  parent.append(row);
}

/* ------------------------------------------------------------------ state */

const state = {
  config: { auto: true, carriers: [], skip: [] },
  sims: [],
  patchedLastBoot: [],   // canonical names imsforge actually wrote on this boot
  ims: {},               // slot index -> { registered }
  reason: null,          // carrier-side explanation when IMS is down
};

function showBanner(text, actionLabel, onClick) {
  $('banner-text').textContent = text;
  const btn = $('banner-action');
  btn.textContent = actionLabel || '';
  btn.hidden = !actionLabel;
  btn.onclick = onClick || null;
  $('banner').hidden = false;
}

function markDirty() {
  showBanner('Changes reach the phone only after you press Save.', 'Save', save);
}

/* ------------------------------------------------------------------- load */

// One shell invocation instead of ten: each trip across the bridge costs about 60 ms of pure
// overhead, which dwarfed most of the commands themselves.
const PROBE = `
echo "@@version"; grep '^version=' ${MODDIR}/module.prop | cut -d= -f2
echo "@@config"; cat ${CONFIG} 2>/dev/null
echo "@@detect"; ${MODDIR}/bin/imsforge detect 2>/dev/null
echo "@@log"; cat ${MODDIR}/last-boot.log 2>/dev/null
echo "@@meta"; ls -d /data/adb/metamodule 2>/dev/null || echo missing
echo "@@impl"; [ -d /data/adb/ksu ] && echo ksu || echo other
echo "@@md5"; md5sum /product/etc/CarrierSettings/others.pb ${MODDIR}/product/etc/CarrierSettings/others.pb 2>/dev/null | awk '{print $1}'
echo "@@volte"; dumpsys carrier_config 2>/dev/null | grep -E '^[[:space:]]*carrier_volte_available_bool =' | sort -u
echo "@@ims"; logcat -b radio -d -t 1000 2>/dev/null | grep isImsRegistered | tail -10
`;

function sections(stdout) {
  const out = {};
  let key = null;
  for (const line of stdout.split('\n')) {
    if (line.startsWith('@@')) {
      key = line.slice(2).trim();
      out[key] = [];
    } else if (key) {
      out[key].push(line);
    }
  }
  for (const k of Object.keys(out)) out[k] = out[k].join('\n').trim();
  return out;
}

async function loadAll() {
  const res = await exec(PROBE);
  const s = sections(res.stdout);

  $('version').textContent = (s.version || '').trim();

  if (s.config) {
    try {
      const parsed = JSON.parse(s.config);
      state.config = {
        auto: parsed.auto !== false,
        carriers: Array.isArray(parsed.carriers) ? parsed.carriers : [],
        skip: Array.isArray(parsed.skip) ? parsed.skip : [],
      };
    } catch (e) {
      toast('carriers.json is not valid JSON');
    }
  } else {
    state.config = { auto: true, carriers: [], skip: [] };
  }

  try {
    state.sims = JSON.parse(s.detect || '{}').sims || [];
  } catch (e) {
    state.sims = [];
  }

  // Which carriers did the last boot actually write? Log lines look like
  // "others.pb [25001]: 20 keys" or "tinkoff_ru.pb: 19 keys".
  const log = s.log || '';
  state.patchedLastBoot = [
    ...[...log.matchAll(/\[([^\]]+)\]:\s*\d+ keys/g)].map((m) => m[1]),
    ...[...log.matchAll(/^(\S+)\.pb:\s*\d+ keys/gm)].map((m) => m[1]),
  ];
  $('log').textContent = log || 'no log yet — reboot once';

  // isImsRegistered is logged per phone; phone index matches SIM slot order.
  state.ims = {};
  for (const m of (s.ims || '').matchAll(/Phone-(\d)\s*: isImsRegistered =(\w+)/g)) {
    const slot = Number(m[1]);
    state.ims[slot] = state.ims[slot] || { registered: false };
    if (m[2] === 'true') state.ims[slot].registered = true;
  }
  state.reason = null;

  renderSims();
  renderStatus(s);
  syncRaw();

  // The carrier-side reason costs a 200 KB dump of telephony.registry, so it is only worth
  // fetching when something is actually wrong.
  if (needsReason()) loadReason();
}

function needsReason() {
  return state.sims.some((sim, slot) => {
    const plan = carrierPlan(sim.canonical_name);
    return plan.on && !(state.ims[slot] && state.ims[slot].registered);
  });
}

async function loadReason() {
  const res = await exec(
    "dumpsys telephony.registry 2>/dev/null | " +
      "grep -oE 'mVopsSupport = [0-9]|IWLAN_IKEV2_AUTH_FAILURE' | sort -u"
  );
  // Two carrier-side states no patch can change. Naming them keeps the module from looking
  // broken when the carrier simply does not offer the service.
  const vops = [...res.stdout.matchAll(/mVopsSupport = (\d)/g)].map((m) => m[1]);
  if (vops.includes('3')) {
    state.reason = 'the network does not offer VoLTE to this SIM';
  } else if (/IWLAN_IKEV2_AUTH_FAILURE/.test(res.stdout)) {
    state.reason = 'the carrier rejected Wi-Fi calling authentication';
  }
  if (state.reason) renderSims();
}

/* ----------------------------------------------------------------- render */

/** What imsforge does with this carrier.
 *
 * The switch already shows the state, so the line below it only explains a reason the user did
 * not choose themselves — whether the config lists the carrier explicitly or detection found it
 * is an internal detail and stays out of the interface.
 */
function carrierPlan(name) {
  if (!name) return { on: false, disabled: true, what: 'Unknown carrier — nothing to patch' };
  if (state.config.skip.includes(name)) {
    return { on: false, disabled: false, what: 'Not patched' };
  }
  const on = state.config.carriers.some((c) => c.canonical_name === name)
    || state.patchedLastBoot.includes(name);
  if (on) {
    return {
      on: true,
      disabled: false,
      what: state.patchedLastBoot.includes(name) ? 'Patched' : 'Will be patched on the next boot',
    };
  }
  return { on: false, disabled: false, what: 'Google already enables VoLTE here — no patch needed' };
}

function renderSims() {
  const box = $('sims');
  box.replaceChildren();
  if (!state.sims.length) {
    box.append(el('p', 'hint', 'No SIM detected.'));
    return;
  }

  state.sims.forEach((sim, slot) => {
    const name = sim.canonical_name;
    const plan = carrierPlan(name);
    const card = el('div', 'sim');

    const head = el('div', 'sim-head');
    const titles = el('div');
    titles.append(el('div', 'sim-name', sim.spn || sim.mccmnc));
    // Google names an unsupported carrier's entry after its MCCMNC, so the two are often the
    // same string — printing "25001 · 25001" just looks like a bug.
    const meta = !name || name === sim.mccmnc ? sim.mccmnc : `${sim.mccmnc} · ${name}`;
    titles.append(el('div', 'sim-meta', meta));
    head.append(titles);

    // One control, one meaning: patch this carrier, or do not.
    const sw = el('button', `switch${plan.on ? ' on' : ''}`);
    sw.setAttribute('role', 'switch');
    sw.setAttribute('aria-checked', String(plan.on));
    sw.disabled = plan.disabled;
    sw.onclick = () => toggleCarrier(name, !plan.on);
    sw.append(el('span', 'knob'));
    head.append(sw);
    card.append(head);

    card.append(el('div', 'sim-state', plan.what));

    // Did it actually work? The config state alone cannot answer that.
    const ims = state.ims[slot];
    const line = el('div', 'sim-ims');
    if (ims && ims.registered) {
      line.classList.add('good');
      line.textContent = 'VoLTE is working — IMS registered';
    } else if (plan.on) {
      line.classList.add('warn');
      line.textContent = state.reason
        ? `IMS not registered — ${state.reason}. That is a carrier-side setting; no patch can change it.`
        : 'IMS not registered yet';
    } else if (ims) {
      line.classList.add('muted');
      line.textContent = 'IMS not registered';
    }
    if (line.textContent) card.append(line);

    box.append(card);
  });
}

function renderStatus(s) {
  const rows = $('status-rows');
  rows.replaceChildren();

  const hasMeta = !(s.meta || '').includes('missing');
  const isKsu = (s.impl || '').trim() === 'ksu';
  const pill = (cls, text) => el('span', `pill ${cls}`, text);

  addRow(rows, 'Mount backend',
    hasMeta ? pill('ok', 'present') : (isKsu ? pill('bad', 'missing') : pill('idle', 'built in')));

  const hashes = (s.md5 || '').trim().split(/\s+/);
  const applied = hashes.length === 2 && hashes[0] === hashes[1];
  addRow(rows, 'Patch active on this boot', applied ? pill('ok', 'yes') : pill('bad', 'no'));

  const volte = /carrier_volte_available_bool = true/.test(s.volte || '');
  addRow(rows, 'Telephony sees VoLTE enabled', volte ? pill('ok', 'yes') : pill('bad', 'no'));

  if (!hasMeta && isKsu) {
    showBanner('No mount backend installed — nothing this module writes can reach the system.', '', null);
  } else if (!applied) {
    showBanner('The patch is not active. Reboot to apply it.', 'Reboot', reboot);
  }
}

function syncRaw() {
  $('raw').value = JSON.stringify(state.config, null, 2);
  $('raw-error').textContent = '';
}

/* ---------------------------------------------------------------- actions */

function toggleCarrier(name, on) {
  state.config.skip = state.config.skip.filter((s) => s !== name);
  const pinned = state.config.carriers.some((c) => c.canonical_name === name);

  if (on) {
    if (!pinned) state.config.carriers.push({ canonical_name: name });
  } else {
    state.config.carriers = state.config.carriers.filter((c) => c.canonical_name !== name);
    state.config.skip.push(name);
  }
  renderSims();
  syncRaw();
  markDirty();
}

async function save() {
  const raw = $('raw').value.trim();
  if ($('raw-details').open && raw) {
    try {
      state.config = JSON.parse(raw);
      renderSims();
    } catch (e) {
      $('raw-error').textContent = String(e.message);
      toast('Invalid JSON');
      return;
    }
  }

  // base64 keeps quotes, newlines and non-ASCII intact through the shell.
  const text = JSON.stringify(state.config, null, 2) + '\n';
  const b64 = btoa(unescape(encodeURIComponent(text)));
  const res = await exec(
    `mkdir -p ${DATADIR} && echo '${b64}' | base64 -d > ${CONFIG} && chmod 644 ${CONFIG} && echo saved`
  );
  if (res.errno !== 0 || !res.stdout.includes('saved')) {
    toast('Could not write the configuration');
    return;
  }
  toast('Saved');
  showBanner('Saved. The new configuration is applied on the next boot.', 'Reboot', reboot);
}

async function reboot() {
  showBanner('Rebooting…', '', null);
  await exec('svc power reboot || reboot');
}

/* ------------------------------------------------------------------- init */

$('refresh').onclick = async () => {
  $('banner').hidden = true;
  await loadAll();
  toast('Refreshed');
};
$('save').onclick = save;
$('reboot').onclick = reboot;
$('raw-format').onclick = () => {
  try {
    $('raw').value = JSON.stringify(JSON.parse($('raw').value), null, 2);
    $('raw-error').textContent = '';
  } catch (e) {
    $('raw-error').textContent = String(e.message);
  }
};

loadAll();
