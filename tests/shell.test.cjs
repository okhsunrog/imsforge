const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const root = path.resolve(__dirname, '..');

test('post-fs-data delegates publication and propagates its failure without removing output', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'imsforge-boot-'));
  try {
    const module = path.join(dir,'module');
    const output = path.join(module,'product/etc/CarrierSettings');
    fs.mkdirSync(output,{recursive:true});
    fs.mkdirSync(path.join(module,'bin'));
    fs.writeFileSync(path.join(output,'.keep'),'');
    fs.writeFileSync(path.join(output,'others.pb'),'previous output');
    fs.writeFileSync(path.join(module,'bin/imsforge'), '#!/bin/sh\n[ "$1" = apply ] || exit 20\nexit 1\n', {mode:0o755});
    const script = fs.readFileSync(path.join(root,'module/post-fs-data.sh'),'utf8')
      .replace('DATADIR=/data/adb/imsforge',`DATADIR=${dir}/data`)
      .replace('PHONE_FILES=/data/user_de/0/com.android.phone/files',`PHONE_FILES=${dir}/phone`);
    fs.writeFileSync(path.join(module,'post-fs-data.sh'),script);
    const result = spawnSync('sh',[path.join(module,'post-fs-data.sh')],{encoding:'utf8'});
    assert.equal(result.status,1,result.stderr);
    assert.equal(fs.readFileSync(path.join(output,'others.pb'),'utf8'),'previous output');
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
});

test('the shipped probe preserves errors from dumpsys instead of a successful empty awk', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'imsforge-probe-'));
  try {
    fs.mkdirSync(path.join(dir,'bin'));
    for (const file of ['probe.sh','probe-radio.awk','probe-carrier.awk']) fs.copyFileSync(path.join(root,'module',file),path.join(dir,file));
    fs.writeFileSync(path.join(dir,'module.prop'),'version=v-test\n');
    fs.writeFileSync(path.join(dir,'bin/imsforge'),'#!/bin/sh\nprintf "{}\\n"\n',{mode:0o755});
    fs.writeFileSync(path.join(dir,'bin/dumpsys'),'#!/bin/sh\necho "permission denied" >&2\nexit 1\n',{mode:0o755});
    const result = spawnSync('sh',[path.join(dir,'probe.sh')],{encoding:'utf8',env:{...process.env,TMPDIR:dir,PATH:`${dir}/bin:${process.env.PATH}`}});
    assert.equal(result.status,0,result.stderr);
    assert.match(result.stdout, /@@radio_ok\n1\n@@radio_error\npermission denied/);
    assert.match(result.stdout, /@@carrier_ok\n1\n@@carrier_error\npermission denied/);
    assert.equal(fs.readdirSync(dir).some(name=>name.startsWith('imsforge-probe.')),false);
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
});
