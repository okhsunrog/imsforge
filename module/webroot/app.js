'use strict';

const MODDIR = '/data/adb/modules/imsforge';
const CONFIG = `${MODDIR}/carriers.json`;

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

function toast(msg) {
  if (typeof ksu !== 'undefined' && ksu.toast) {
    try { ksu.toast(msg); return; } catch (e) { /* fall through */ }
  }
  const el = $('toast');
  el.textContent = msg;
  el.hidden = false;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => { el.hidden = true; }, 2200);
}

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
};
const pill = (cls, text) => el('span', `pill ${cls}`, text);

function addRow(parent, label, valueNode) {
  const row = el('div', 'row');
  row.append(el('div', 'label', label));
  const v = el('div', 'value');
  v.append(valueNode);
  row.append(v);
  parent.append(row);
  return row;
}

/* ------------------------------------------------------------------ state */

const state = {
  config: { auto: true, carriers: [], skip: [] },
  sims: [],
  dirty: false,
};

function markDirty() {
  state.dirty = true;
  showBanner('Saved changes apply on the next boot.', 'Reboot', reboot);
}

function showBanner(text, actionLabel, onClick) {
  $('banner-text').textContent = text;
  const btn = $('banner-action');
  btn.textContent = actionLabel || '';
  btn.hidden = !actionLabel;
  btn.onclick = onClick || null;
  $('banner').hidden = false;
}

/* ------------------------------------------------------------------- load */

async function loadAll() {
  await Promise.all([loadStatus(), loadConfigAndSims(), loadIms()]);
}

async function loadStatus() {
  const rows = $('status-rows');
  rows.replaceChildren();

  const [ver, meta, impl, md5, log] = await Promise.all([
    exec(`grep '^version=' ${MODDIR}/module.prop | cut -d= -f2`),
    exec('ls -d /data/adb/metamodule 2>/dev/null || echo missing'),
    exec('[ -d /data/adb/ksu ] && echo ksu || echo other'),
    exec(`md5sum /product/etc/CarrierSettings/others.pb ${MODDIR}/product/etc/CarrierSettings/others.pb 2>/dev/null | awk '{print $1}' | tr '\\n' ' '`),
    exec(`cat ${MODDIR}/last-boot.log 2>/dev/null`),
  ]);

  $('version').textContent = ver.stdout.trim();

  // Magisk mounts module files itself; KernelSU needs a metamodule, and without one a module
  // that ships files does nothing at all — silently. Worth saying out loud.
  const hasMeta = !meta.stdout.includes('missing');
  const isKsu = impl.stdout.trim() === 'ksu';
  addRow(rows, 'Mount backend',
    hasMeta ? pill('ok', 'present') : (isKsu ? pill('bad', 'missing') : pill('idle', 'built in')));

  const hashes = md5.stdout.trim().split(/\s+/);
  const shadowed = hashes.length === 2 && hashes[0] === hashes[1];
  addRow(rows, 'Patch applied to /product',
    shadowed ? pill('ok', 'yes') : pill(hashes.length === 2 ? 'bad' : 'warn', 'no'));

  const text = log.stdout.trim();
  $('log').textContent = text || 'no log yet — reboot once';
  const patched = [...text.matchAll(/^\s*(\S+\.pb.*?): (\d+) keys/gm)];
  addRow(rows, 'Last boot',
    el('span', 'value mono', patched.length ? `${patched.length} file(s) patched` : 'nothing patched'));

  if (!hasMeta && isKsu) {
    showBanner('No mount backend installed — the module cannot apply anything.', '', null);
  }
}

async function loadConfigAndSims() {
  const [cfgRes, detectRes] = await Promise.all([
    exec(`cat ${CONFIG} 2>/dev/null`),
    exec(`${MODDIR}/bin/imsforge detect 2>/dev/null`),
  ]);

  if (cfgRes.stdout.trim()) {
    try {
      const parsed = JSON.parse(cfgRes.stdout);
      state.config = {
        auto: parsed.auto !== false,
        carriers: Array.isArray(parsed.carriers) ? parsed.carriers : [],
        skip: Array.isArray(parsed.skip) ? parsed.skip : [],
      };
    } catch (e) {
      toast('carriers.json is not valid JSON');
    }
  }

  try {
    state.sims = (JSON.parse(detectRes.stdout || '{}').sims) || [];
  } catch (e) {
    state.sims = [];
  }

  renderSims();
  renderCarriers();
  $('raw').value = JSON.stringify(state.config, null, 2);
}

async function loadIms() {
  const rows = $('ims-rows');
  rows.replaceChildren();

  const [cc, reg] = await Promise.all([
    exec("dumpsys carrier_config 2>/dev/null | grep -E '^[[:space:]]*(carrier_config_version_string|carrier_volte_available_bool) =' | sort -u"),
    exec("dumpsys telephony.registry 2>/dev/null"),
  ]);

  const versions = [...cc.stdout.matchAll(/carrier_config_version_string = (\S+)/g)].map((m) => m[1]);
  addRow(rows, 'Config in use',
    el('span', 'value mono', versions.length ? versions.join(', ') : 'unknown'));

  const volte = /carrier_volte_available_bool = true/.test(cc.stdout);
  addRow(rows, 'VoLTE available', volte ? pill('ok', 'yes') : pill('bad', 'no'));

  const pcscf = [...reg.stdout.matchAll(/PcscfAddresses: \[\s*([^\]\s][^\]]*)\]/g)].map((m) => m[1].trim());
  addRow(rows, 'IMS PDN',
    pcscf.length ? pill('ok', 'connected') : pill('warn', 'not up'));
  if (pcscf.length) {
    addRow(rows, 'P-CSCF', el('span', 'value mono', pcscf[0]));
  }

  // Two states no patch can fix — worth naming precisely instead of looking like our bug.
  const vops = [...reg.stdout.matchAll(/mVopsSupport = (\d)/g)].map((m) => m[1]);
  const hint = $('ims-hint');
  if (vops.length && !vops.includes('2')) {
    hint.textContent =
      'The network reports mVopsSupport = 3: it is not offering voice over LTE to this SIM. ' +
      'That is a carrier-side setting — no config can override it.';
    hint.hidden = false;
  } else if (/IWLAN_IKEV2_AUTH_FAILURE/.test(reg.stdout)) {
    hint.textContent =
      'The carrier ePDG rejected authentication for Wi-Fi calling, so the subscription is not ' +
      'provisioned for it. Turn Wi-Fi calling off for that SIM to stop the retries.';
    hint.hidden = false;
  } else {
    hint.hidden = true;
  }
}

/* ----------------------------------------------------------------- render */

function renderSims() {
  const box = $('sims');
  box.replaceChildren();
  if (!state.sims.length) {
    box.append(el('p', 'hint', 'No SIM detected.'));
    return;
  }

  for (const sim of state.sims) {
    const card = el('div', 'sim');
    const head = el('div', 'sim-head');
    head.append(el('div', 'sim-name', sim.spn || sim.mccmnc));

    const name = sim.canonical_name;
    const forced = state.config.carriers.some((c) => c.canonical_name === name);
    const skipped = state.config.skip.includes(name);

    if (!name) head.append(pill('warn', 'unknown carrier'));
    else if (skipped) head.append(pill('idle', 'skipped'));
    else if (forced) head.append(pill('ok', 'override'));
    else head.append(pill('ok', 'auto'));
    card.append(head);

    card.append(el('div', 'sim-meta', `${sim.mccmnc}${name ? ` · ${name}` : ''}`));

    if (name) {
      const actions = el('div', 'sim-actions');
      if (!forced) {
        const b = el('button', 'small', 'Add override');
        b.onclick = () => {
          state.config.carriers.push({ canonical_name: name, ims_apn_name: `${name} IMS` });
          state.config.skip = state.config.skip.filter((s) => s !== name);
          renderSims(); renderCarriers(); syncRaw(); markDirty();
        };
        actions.append(b);
      }
      const s = el('button', 'small', skipped ? 'Unskip' : 'Never patch');
      s.onclick = () => {
        state.config.skip = skipped
          ? state.config.skip.filter((x) => x !== name)
          : [...state.config.skip, name];
        renderSims(); syncRaw(); markDirty();
      };
      actions.append(s);
      card.append(actions);
    }
    box.append(card);
  }
}

function renderCarriers() {
  const box = $('carriers');
  box.replaceChildren();
  if (!state.config.carriers.length) {
    box.append(el('p', 'hint', 'None — everything is handled automatically.'));
    return;
  }

  state.config.carriers.forEach((carrier, i) => {
    const row = el('div', 'carrier');
    const grow = el('div', 'grow');
    grow.append(el('div', 'cname', carrier.canonical_name));

    const input = el('input');
    input.value = carrier.ims_apn_name || '';
    input.placeholder = 'IMS APN name';
    input.oninput = () => { carrier.ims_apn_name = input.value; syncRaw(); markDirty(); };
    grow.append(input);

    const extras = [];
    if (carrier.int_arrays) extras.push(...Object.keys(carrier.int_arrays));
    if (carrier.bools) extras.push(...Object.keys(carrier.bools));
    if (extras.length) grow.append(el('div', 'extra', extras.join(', ')));
    row.append(grow);

    const del = el('button', 'small danger', 'Remove');
    del.onclick = () => {
      state.config.carriers.splice(i, 1);
      renderCarriers(); renderSims(); syncRaw(); markDirty();
    };
    row.append(del);
    box.append(row);
  });
}

function syncRaw() {
  $('raw').value = JSON.stringify(state.config, null, 2);
  $('raw-error').textContent = '';
}

/* ---------------------------------------------------------------- actions */

async function save() {
  const raw = $('raw').value.trim();
  if ($('raw-details').open && raw) {
    try {
      const parsed = JSON.parse(raw);
      state.config = parsed;
      renderCarriers();
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
    `echo '${b64}' | base64 -d > ${CONFIG} && chmod 644 ${CONFIG} && echo saved`
  );
  if (res.errno !== 0 || !res.stdout.includes('saved')) {
    toast('Could not write carriers.json');
    return;
  }
  state.dirty = false;
  toast('Saved');
  showBanner('Config saved. It takes effect on the next boot.', 'Reboot', reboot);
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
