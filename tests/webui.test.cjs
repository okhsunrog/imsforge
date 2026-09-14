const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { spawnSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');
const code = fs.readFileSync(path.join(root, 'module/webroot/app.js'), 'utf8').replace(/loadAll\(\);\s*$/, '');

function setup() {
  const nodes = new Map();
  const ids = new Set([...fs.readFileSync(path.join(root, 'module/webroot/index.html'), 'utf8').matchAll(/id="([^"]+)"/g)].map(match => match[1]));
  const timers = new Map();
  let timerId = 0;
  function node() {
    return { children: [], textContent: '', value: '', hidden: true, disabled: false, open: false,
      append(...items) { this.children.push(...items); }, replaceChildren() { this.children = []; }, setAttribute() {} };
  }
  const context = {
    document: { body: {classList: {toggle() {}}}, getElementById(id) { assert.ok(ids.has(id), `missing HTML element ${id}`); if (!nodes.has(id)) nodes.set(id, node()); return nodes.get(id); }, createElement: node },
    setTimeout(callback) { const id = ++timerId; timers.set(id, callback); return id; },
    clearTimeout(id) { timers.delete(id); },
    btoa: s => Buffer.from(s, 'binary').toString('base64'), unescape, encodeURIComponent,
  };
  for (const match of fs.readFileSync(path.join(root, 'module/webroot/index.html'), 'utf8').matchAll(/id="([^"]+)"/g)) context.document.getElementById(match[1]);
  context.window = context;
  vm.createContext(context);
  vm.runInContext(code, context);
  return { context, nodes, timers, run: s => vm.runInContext(s, context) };
}

function response(overrides = {}) {
  const entries = { config: '{}', detect: JSON.stringify({complete: true, sims: [{slot: 1, canonical_name: '25001', spn: 'MTS', certified: false}]}),
    status: JSON.stringify({current_boot: false, run: null}), meta: 'present', radio: 'pcscf 1 yes', carrier: 'volte 1 true', ims: '',
    version: 'version=v2.2.1', log: 'log', ...overrides };
  return Object.entries(entries).map(([name, text]) => `@@${name}\n${text}\n@@${name}_ok\n0\n`).join('');
}

async function loaded(overrides = {}) {
  const env = setup();
  env.context.reply = response(overrides);
  env.run('exec = async () => ({errno:0,stdout:reply});');
  await env.run('loadAll()');
  return env;
}

test('enable works with auto off and after a certified carrier was skipped', async () => {
  for (const certified of [false, true]) {
    const env = await loaded({config:'{"auto":false,"skip":["25001"]}'});
    env.run(`state.sims[0].certified=${certified};toggleCarrier('25001',true);`);
    assert.equal(env.run("carrierPlan('25001').on"), true);
    assert.equal(env.run('state.config.carriers[0].canonical_name'), '25001');
    assert.equal(env.run('state.dirty'), true);
    assert.equal(env.nodes.get('reboot').disabled, true);
  }
});

test('minimal JSON and a collapsed editor both save the actual draft', async () => {
  for (const draft of ['{}', '{"auto":false}']) {
    const env = await loaded();
    env.nodes.get('raw').value = draft;
    env.nodes.get('raw-details').open = false;
    let saved;
    env.context.exec = async command => {
      if (command.includes('save-config')) {
        const encoded = command.match(/printf '%s' '([^']+)'/)[1];
        saved = JSON.parse(Buffer.from(encoded, 'base64').toString());
        return {errno:0,stdout:JSON.stringify(saved)};
      }
      return {errno:0,stdout:response({config:JSON.stringify(saved)})};
    };
    await env.run('save()');
    assert.equal(saved.auto, draft === '{}');
    assert.equal(env.run('state.ready'), true);
    assert.equal(env.run('state.dirty'), false);
  }
});

test('a malformed draft cannot corrupt the rendered config', async () => {
  const env = await loaded();
  env.nodes.get('raw').value = '{"skip":null}';
  await env.run('save()');
  assert.equal(env.run('Array.isArray(state.config.skip)'), true);
  assert.match(env.nodes.get('raw-error').textContent, /arrays/);
});

test('failed reads preserve configuration and disable saving', async () => {
  const env = await loaded({config:'{"auto":false}'});
  env.context.exec = async () => ({errno:1,stdout:'',stderr:'permission denied'});
  await env.run('loadAll()');
  assert.equal(env.run('state.config.auto'), false);
  assert.equal(env.run('state.ready'), false);
  assert.equal(env.nodes.get('save').disabled, true);
  assert.match(env.nodes.get('banner-text').textContent, /permission denied/);
});

test('section failures are not interpreted as an absent configuration', async () => {
  const env = await loaded({config:'{"auto":false}'});
  env.context.reply = response().replace('@@config_ok\n0', '@@config_ok\n1');
  await env.run('loadAll()');
  assert.equal(env.run('state.config.auto'), false);
  assert.equal(env.run('state.ready'), false);
});

test('VoLTE of another SIM and a previous boot do not confirm publication', async () => {
  const env = await loaded({status:JSON.stringify({current_boot:false, matches_run:true, run:{format:2,phase:'applied',files:{'others.pb':1},carriers:[]}}), carrier:'volte 0 true'});
  assert.equal(env.run('currentRun()'), null);
  assert.equal(env.nodes.get('status-rows').children[1].children[1].children[0].textContent, 'not confirmed');
  assert.equal(env.run("state.observations[1]?.volte"), undefined);
});

test('observations stay attached to sparse slot IDs and do not claim registration', async () => {
  const env = await loaded({radio:'pcscf 0 no\npcscf 1 yes',carrier:'volte 0 false\nvolte 1 true'});
  assert.equal(env.run('state.observations[1].volte'), 'true');
  assert.equal(env.run('state.observations[1].pcscf'), 'yes');
  assert.equal(env.run('imsSummary(state.observations[1]).text'), 'IMS status unavailable');
});

test('a failed current boot remains an error even if another slot enables VoLTE', async () => {
  const env = await loaded({status:JSON.stringify({current_boot:true,run:{format:2,phase:'failed',error:'label failed',files:{},carriers:[]}}),carrier:'volte 0 true'});
  assert.match(env.nodes.get('banner-text').textContent, /label failed/);
  assert.equal(env.nodes.get('banner-action').hidden, true);
});

test('refresh cannot silently discard an unsaved raw draft', async () => {
  const env = await loaded();
  env.nodes.get('raw').value = '{"auto":false}';
  env.nodes.get('raw').oninput();
  await env.run('loadAll()');
  assert.equal(env.nodes.get('raw').value, '{"auto":false}');
  assert.equal(env.run('state.dirty'), true);
});

test('root bridge failure releases its callback and timer', async () => {
  const env = setup();
  const result = await env.run('exec("true")');
  assert.equal(result.errno, 1);
  assert.equal(Object.keys(env.context).filter(k => k.startsWith('_imsforge_cb_')).length, 0);
  assert.equal(env.timers.size, 0);
});

test('the real awk files parse current data and ignore history', () => {
  const radio = 'Phone Id=0\n mServiceState={LteVopsSupportInfo: mVopsSupport = 3}\n mPreciseDataConnectionStates={}\nPhone Id=1\n mServiceState={LteVopsSupportInfo: mVopsSupport = 2}\n mPreciseDataConnectionStates={PcscfAddresses: [ /1.2.3.4 ]}\n2026 history mVopsSupport = 3 IWLAN_IKEV2_AUTH_FAILURE\nPhone Id=2\n mServiceState={NrVopsSupportInfo: mVopsSupport = 3}\n';
  const result = spawnSync('awk',['-f',path.join(root,'module/probe-radio.awk')],{input:radio,encoding:'utf8'});
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'vops 0 3\npcscf 0 no\nvops 1 2\npcscf 1 yes\n');
  const config = ' carrier_volte_available_bool = true\nPhone Id = 1\n carrier_volte_available_bool = false\n';
  const parsed = spawnSync('awk',['-f',path.join(root,'module/probe-carrier.awk')],{input:config,encoding:'utf8'});
  assert.equal(parsed.status,0,parsed.stderr);
  assert.equal(parsed.stdout,'volte 1 false\n');
});

test('semantic rejection preserves an editable draft without overwriting saved config', async () => {
  const env = await loaded();
  const draft = '{"auto":false,"unknown":true}';
  env.nodes.get('raw').value = draft;
  env.nodes.get('raw').oninput();
  env.context.exec = async () => ({errno:1,stdout:'',stderr:'imsforge: invalid configuration: unknown field unknown'});
  await env.run('save()');
  assert.equal(env.nodes.get('raw').value,draft);
  assert.equal(env.run('state.ready'),true);
  assert.equal(env.run('state.saved.auto'),true);
  assert.equal(env.nodes.get('raw').disabled,false);
});

const appliedStatus = {current_boot:true,config_changed:false,matches_run:true,run:{format:2,phase:'applied',files:{'others.pb':1},carriers:[{canonical_name:'25001',outcome:'patched'}]}};

test('footer only appears for drafts or a pending restart', async () => {
  const env = await loaded({status:JSON.stringify(appliedStatus)});
  assert.equal(env.nodes.get('action-bar').hidden,true);
  assert.equal(env.nodes.get('save').disabled,true);
  env.run("toggleCarrier('25001', false)");
  assert.equal(env.nodes.get('action-bar').hidden,false);
  assert.equal(env.nodes.get('reboot').hidden,true);
  await env.nodes.get('discard').onclick();
  assert.equal(env.nodes.get('action-bar').hidden,true);
  env.run('state.status.config_changed=true;renderStatus()');
  assert.equal(env.nodes.get('action-bar').hidden,false);
  assert.equal(env.nodes.get('save').hidden,true);
  assert.equal(env.nodes.get('reboot').hidden,false);
});

test('formatting or restoring the saved raw draft is not a pending change', async () => {
  const env = await loaded({status:JSON.stringify(appliedStatus)});
  env.nodes.get('raw').value='{"skip":[],"carriers":[],"auto":true}';
  env.nodes.get('raw').oninput();
  assert.equal(env.run('state.dirty'),false);
  assert.equal(env.nodes.get('action-bar').hidden,true);
});

test('IMS registration requires current per-slot data and is independent of patch state', async () => {
  const env = await loaded({status:JSON.stringify(appliedStatus),ims:'ims 0 0\nvoice 0 false\nims 1 2\nvoice 1 true\nlast_transport 1 wifi'});
  assert.equal(env.run('imsSummary(state.observations[1]).tone'),'good');
  assert.equal(env.run('imsSummary(state.observations[0]).text'),'IMS not registered');
  env.run("toggleCarrier('25001',false)");
  assert.equal(env.run('imsSummary(state.observations[1]).tone'),'good');
});

test('diagnostic failures do not hide the boot failure explanation', async () => {
  const env = await loaded({status:JSON.stringify({...appliedStatus,run:{...appliedStatus.run,phase:'failed',error:'output denied'}})});
  env.run("state.errors=['ims: unavailable'];renderStatus()");
  assert.match(env.nodes.get('banner-text').textContent,/output denied/);
});

test('IMS parser scopes current fields and treats transport as history only', () => {
  const input = `GsmCdmaPhone extends:
 mPhoneId=0
ImsPhone extends:
 mPhoneId=0
++++++++++++++++
ImsPhoneCallTracker extends:
 mMmTelCapabilities=MmTel Capabilities - [Voice: false SMS: false]
++++++++++++++++
ImsPhone:
 mImsMmTelRegistrationState = 0
 Registration Log:
  2026-01-01 handleImsRegistered: onImsMmTelConnected imsTransportType=WLAN
++++++++++++++++
GsmCdmaPhone extends:
 mPhoneId=2
ImsPhone extends:
 mPhoneId=2
++++++++++++++++
ImsPhoneCallTracker extends:
 mMmTelCapabilities=MmTel Capabilities - [Voice: true SMS: true]
++++++++++++++++
ImsPhone:
 mImsMmTelRegistrationState = 2
 mImsMmTelEmergencyRegistrationState = 0
 Registration Log:
  2026-01-01 handleImsRegistered: onImsMmTelConnected imsTransportType=WLAN
  2026-01-02 handleImsUnregistered: onImsMmTelDisconnected
  2026-01-03 handleImsRegistered: onImsMmTelConnected imsTransportType=WWAN
++++++++++++++++
Unrelated:
 mImsMmTelRegistrationState = 0
 mMmTelCapabilities=MmTel Capabilities - [Voice: false]
 2026-01-04 handleImsRegistered: onImsMmTelConnected imsTransportType=WLAN
`;
  const result=spawnSync('awk',['-f',path.join(root,'module/probe-ims.awk')],{input,encoding:'utf8'});
  assert.equal(result.status,0,result.stderr);
  assert.equal(result.stdout,'ims 0 0\nvoice 0 false\nims 2 2\nvoice 2 true\nlast_transport 2 cellular\n');
  const unsupported=spawnSync('awk',['-f',path.join(root,'module/probe-ims.awk')],{input:'No services match\n mImsRegistered=true\n',encoding:'utf8'});
  assert.equal(unsupported.stdout,'');
});

test('disabling and re-enabling preserves custom carrier overrides', async () => {
  const env=await loaded({config:JSON.stringify({auto:true,carriers:[{canonical_name:'25001',int_arrays:{carrier_nr_availabilities_int_array:[1,2]}}],skip:[]}),status:JSON.stringify(appliedStatus)});
  env.run("toggleCarrier('25001',false);toggleCarrier('25001',true)");
  assert.equal(env.run('state.config.carriers[0].int_arrays.carrier_nr_availabilities_int_array.join()'),'1,2');
  assert.equal(env.run('state.dirty'),false);
  assert.equal(env.nodes.get('action-bar').hidden,true);
});
