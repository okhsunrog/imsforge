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
  certified: [],         // carriers the patcher deliberately left to Google
  booted: false,         // is there a record of a patch run to reason from at all
  live: false,           // is the file the system reads right now our output
  nothingToPatch: false, // the last run decided every carrier was fine as Google shipped it
  ims: {},               // slot index -> { registered }
  reasons: {},           // slot index -> carrier-side explanation when IMS is down
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
echo "@@status"; ${MODDIR}/bin/imsforge status 2>/dev/null
echo "@@volte"; dumpsys carrier_config 2>/dev/null | grep -E '^[[:space:]]*carrier_volte_available_bool =' | sort -u
echo "@@ims"; logcat -b radio -d 2>/dev/null | grep isImsRegistered | tail -10
echo "@@radio"; dumpsys telephony.registry 2>/dev/null | awk '
  /^[[:space:]]*Phone Id=/ { split($0, a, "="); phone = a[2]; seen[phone] = 0 }
  /mPreciseDataConnectionStates/ {
    if ($0 ~ /PcscfAddresses: \[ \//) print "pdn", phone, "yes"; else print "pdn", phone, "no"
  }
  /mVopsSupport/ && seen[phone] == 0 {
    n = split($0, b, "mVopsSupport = ")
    if (n > 1) { print "vops", phone, substr(b[2], 1, 1); seen[phone] = 1 }
  }
  /IWLAN_IKEV2_AUTH_FAILURE/ { print "iwlan", phone }
'
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

  // What the last patch decided, and whether it is what the system reads right now. The patcher
  // writes this record for us: reconstructing it from the log would make the wording of a log
  // line an interface, and a reworded line would silently make this screen lie.
  let status = {};
  try {
    status = JSON.parse(s.status || '{}');
  } catch (e) { /* no patcher, or one too old to answer */ }
  const run = status.run && status.run.format === 1 ? status.run : null;
  // "ours" is the only answer that means the mount backend delivered our files.
  state.live = status.product === 'ours';
  const carriers = run && Array.isArray(run.carriers) ? run.carriers : [];
  const named = (outcome) =>
    carriers.filter((c) => c.outcome === outcome).map((c) => c.canonical_name);

  state.patchedLastBoot = named('patched');
  // Only the patcher knows a carrier was left alone because Google already supports it; the
  // interface must not guess that from the absence of a patch.
  state.certified = named('certified');
  state.booted = run !== null;
  state.nothingToPatch = run !== null && state.patchedLastBoot.length === 0;

  // A name in carriers.json that CarrierSettings has nothing for: valid JSON, accepted on save,
  // and still nothing will ever come of it. Only the patcher can tell, and only after a boot.
  const missing = named('missing');
  const note = $('config-note');
  note.textContent = missing.length
    ? `carriers.json names ${missing.join(', ')} — no carrier by that name exists in this phone's `
      + 'CarrierSettings, so nothing is patched for it.'
    : '';
  note.hidden = missing.length === 0;

  $('log').textContent = s.log || 'no log yet — reboot once';

  // Is IMS actually up, per slot?
  //
  // The IMS bearer is the thing to look at: an APN of type IMS that is connected and carries the
  // P-CSCF address the network handed out. That is live state, read out of a dump.
  //
  // isImsRegistered in the radio log is only corroboration, and only where the dump said nothing
  // about a slot at all: the line is a debug print from a getter, so it appears when something
  // happens to call it — three times in an hour on a working phone — and then ages out of the
  // ring buffer. Read the other way round, its absence would report a working SIM as broken.
  //
  // A slot missing from both is left unknown rather than called unregistered.
  state.ims = {};
  for (const m of (s.radio || '').matchAll(/^pdn (\d+) (yes|no)$/gm)) {
    state.ims[Number(m[1])] = { registered: m[2] === 'yes' };
  }
  const logged = {};
  // The last line per phone wins: or-ing them would keep reporting "registered" after IMS had
  // dropped, simply because an older line in the buffer said so.
  for (const m of (s.ims || '').matchAll(/Phone-(\d)\s*: isImsRegistered =(\w+)/g)) {
    logged[Number(m[1])] = m[2] === 'true';
  }
  for (const slot of Object.keys(logged)) {
    if (!state.ims[slot]) state.ims[slot] = { registered: logged[slot] };
  }

  // Two carrier-side states no patch can change. Naming them keeps the module from looking
  // broken when the carrier simply does not offer the service.
  //
  // Per phone, and only the first value each one reports: the dump carries a whole history of
  // service states, so a carrier whose VoLTE works right now still has older entries saying it
  // did not. Reading them all at once and asking "is a 3 in there" blames the wrong SIM, and
  // blames it for something that is no longer true.
  state.reasons = {};
  for (const m of (s.radio || '').matchAll(/^vops (\d+) (\d)$/gm)) {
    if (m[2] === '3') {
      state.reasons[Number(m[1])] = 'the network does not offer VoLTE to this SIM';
    }
  }
  for (const m of (s.radio || '').matchAll(/^iwlan (\d+)$/gm)) {
    const slot = Number(m[1]);
    if (!state.reasons[slot]) {
      state.reasons[slot] = 'the carrier rejected Wi-Fi calling authentication';
    }
  }

  renderSims();
  renderStatus(s);
  syncRaw();
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
  if (state.certified.includes(name)) {
    return { on: false, disabled: false, what: 'Google already enables VoLTE here — no patch needed' };
  }
  if (!state.booted) {
    return { on: false, disabled: false, what: 'Not patched yet — reboot to apply' };
  }
  return { on: false, disabled: false, what: 'Not patched' };
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
    } else if (!ims) {
      // Nothing to go on. Calling that "not registered" would be a guess, and on a phone that
      // has been up a while it would be the wrong one.
      if (plan.on) {
        line.classList.add('muted');
        line.textContent = 'IMS state unknown';
      }
    } else if (plan.on) {
      line.classList.add('warn');
      const why = state.reasons[slot];
      line.textContent = why
        ? `IMS not registered — ${why}. That is a carrier-side setting; no patch can change it.`
        : 'IMS not registered yet';
    } else {
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

  const volte = /carrier_volte_available_bool = true/.test(s.volte || '');
  // Did this boot write a patch at all? state.live answers a narrower question — whether the
  // file THIS process reads at /product is ours — and a root shell or an app confined to its own
  // mount namespace can be told our file is not there while the patch was applied all the same.
  const applied = state.booted && state.patchedLastBoot.length > 0;

  addRow(rows, 'Patch active on this boot',
    state.live ? pill('ok', 'yes')
      : state.nothingToPatch ? pill('idle', 'nothing to patch')
        // Written this boot, but not the file we are reading here: normal when the mount backend
        // hands the module's files to some apps and not to this viewer. Only a fault if telephony
        // — the one process that must see it — did not either, and that is the row below.
        : applied ? pill('idle', 'applied on boot')
          : pill('bad', 'no'));

  addRow(rows, 'Telephony sees VoLTE enabled', volte ? pill('ok', 'yes') : pill('bad', 'no'));

  if (!hasMeta && isKsu) {
    showBanner('No mount backend installed — nothing this module writes can reach the system.', '', null);
  } else if (!state.booted) {
    // No record of a run: a fresh install before its first reboot.
    showBanner('Not applied yet. Reboot to apply the patch.', 'Reboot', reboot);
  } else if (applied && !state.live && !volte) {
    // The patch was written, this viewer does not see it, and neither does telephony — so it is
    // not just a namespace the viewer is outside of; the backend is not delivering the files.
    showBanner('The patch was applied on boot but the system is not reading it. Reboot to reapply.', 'Reboot', reboot);
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
    // Turning a carrier on normally just lifts the skip and lets detection do its job. An
    // explicit entry is only added when detection would pass the carrier over, because such an
    // entry permanently bypasses the "Google already supports this" safety check.
    if (!pinned && state.certified.includes(name)) {
      state.config.carriers.push({ canonical_name: name });
    }
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

  // Written to one side and checked by the patcher before it replaces anything: the patcher
  // rejects a key it does not know, and a configuration it rejects stops the next boot from
  // patching at all. Finding that out here beats finding it out from a log after a reboot.
  //
  // base64 keeps quotes, newlines and non-ASCII intact through the shell.
  const text = JSON.stringify(state.config, null, 2) + '\n';
  const b64 = btoa(unescape(encodeURIComponent(text)));
  const res = await exec(`
mkdir -p ${DATADIR} || exit 1
echo '${b64}' | base64 -d > ${CONFIG}.new || exit 1
${MODDIR}/bin/imsforge check --config ${CONFIG}.new || { rm -f ${CONFIG}.new; exit 1; }
chmod 644 ${CONFIG}.new && mv ${CONFIG}.new ${CONFIG} && echo saved
`);
  if (res.errno !== 0 || !res.stdout.includes('saved')) {
    const why = (res.stderr || '').replace(/^imsforge: \S*:\s*/m, '').trim();
    $('raw-error').textContent = why;
    toast(why ? 'Rejected: ' + why.split('\n')[0] : 'Could not write the configuration');
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
