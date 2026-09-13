"""imsforge CLI: snapshot, patch, package, install and verify."""

import argparse
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import zipfile

from . import patch, protos
from .protos import ROOT

MODULE_ID = "imsforge"
MODULE_TEMPLATE = ROOT / "module"
CARRIERS_TOML = ROOT / "carriers.toml"
STOCK_DIR = ROOT / "stock"
STOCK_META = STOCK_DIR / "stock.json"
DIST_DIR = ROOT / "dist"
CARRIER_SETTINGS = "/product/etc/CarrierSettings"
PHONE_FILES = "/data/user_de/0/com.android.phone/files"
CONFIG_CACHE_GLOB = "carrierconfig-com.google.android.carrier-*.xml"


def adb(*args: str, root: bool = False, check: bool = True, mount_master: bool = False) -> str:
    if root:
        # -M puts mounts in the global namespace, otherwise system processes never see them.
        # Note: `$` inside is expanded by the device shell before su runs — escape it.
        cmd = ["adb", "shell", f'su {"-M " if mount_master else ""}-c "{args[0]}"']
    else:
        cmd = ["adb", *args]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if check and result.returncode != 0:
        raise SystemExit(f"adb {' '.join(args)} -> {result.returncode}\n{result.stderr.strip()}")
    return result.stdout


def md5(path: pathlib.Path) -> str:
    return hashlib.md5(path.read_bytes()).hexdigest()


def carriers() -> dict[str, patch.Carrier]:
    if not CARRIERS_TOML.exists():
        raise SystemExit(f"{CARRIERS_TOML} is missing")
    return patch.load_carriers(CARRIERS_TOML)


def read_stock_meta() -> dict:
    return json.loads(STOCK_META.read_text()) if STOCK_META.exists() else {}


def cmd_carriers(args: argparse.Namespace) -> None:
    """Resolve the SIMs in the phone to CarrierSettings canonical names."""
    protos.ensure_generated()
    import carrier_list_pb2 as cl

    tmp = DIST_DIR / "carrier_list.pb"
    tmp.parent.mkdir(parents=True, exist_ok=True)
    adb("pull", f"{CARRIER_SETTINGS}/carrier_list.pb", str(tmp))
    listing = cl.CarrierList()
    listing.ParseFromString(tmp.read_bytes())

    numerics = adb("shell", "getprop gsm.sim.operator.numeric").strip()
    names = adb("shell", "getprop gsm.sim.operator.alpha").strip()
    sims = [n for n in numerics.split(",") if n]
    print(f"SIMs in the phone: {numerics or '(none)'}  ({names})\n")

    for sim in sims:
        matches = [
            (entry.canonical_name, cid)
            for entry in listing.entry
            for cid in entry.carrier_id
            if cid.mcc_mnc == sim
        ]
        if not matches:
            print(f"{sim}: no entry in carrier_list.pb — carrier is unknown to CarrierSettings")
            continue
        for canonical, cid in matches:
            mvno = cid.WhichOneof("mvno_data")
            extra = f", {mvno}={getattr(cid, mvno)}" if mvno else ""
            print(f"{sim}: canonical_name = {canonical!r}{extra}")
        canonical = matches[0][0]
        print("  add to carriers.toml:\n")
        print("    [[carrier]]")
        print(f'    canonical_name = "{canonical}"')
        print(f'    ims_apn_name = "{canonical.upper()} IMS"\n')


def cmd_pull(args: argparse.Namespace) -> None:
    """Snapshot the stock CarrierSettings off the phone. Run this with the module disabled."""
    STOCK_DIR.mkdir(exist_ok=True)
    built = {p.name: md5(p) for p in (DIST_DIR / "module").rglob("*.pb")} if DIST_DIR.exists() else {}

    names = ["others.pb"]
    listing = adb("shell", f"ls {CARRIER_SETTINGS}").split()
    for canonical in carriers():
        if f"{canonical}.pb" in listing:
            names.append(f"{canonical}.pb")

    stale = False
    digests = {}
    for name in names:
        dst = STOCK_DIR / name
        adb("pull", f"{CARRIER_SETTINGS}/{name}", str(dst))
        digests[name] = md5(dst)
        patched = built.get(name) == digests[name]
        stale |= patched
        print(f"  {name}: {digests[name]}{'   <- WARNING: this is the PATCHED file!' if patched else ''}")

    meta = {
        "device": adb("shell", "getprop ro.product.device").strip(),
        "model": adb("shell", "getprop ro.product.model").strip(),
        "fingerprint": adb("shell", "getprop ro.build.fingerprint").strip(),
        "security_patch": adb("shell", "getprop ro.build.version.security_patch").strip(),
        "md5": digests,
    }
    STOCK_META.write_text(json.dumps(meta, indent=2) + "\n")
    print(f"\n{meta['model']} ({meta['device']}), {meta['fingerprint']}")
    print(f"snapshot written to {STOCK_DIR}")

    if stale:
        print(
            "\n! The module is active and shadowing /product, so this 'stock' snapshot is\n"
            "! actually the patched one. Disable the module, reboot, and pull again."
        )


def render_customize(meta: dict, stock_md5: str, patched_md5: str) -> str:
    device = meta.get("device", "")
    names = ", ".join(carriers())
    return f"""#!/system/bin/sh
SKIPUNZIP=0

# md5 of the files this module was built from. STOCK is the original from the Android build
# the snapshot was taken on, PATCHED is the result. Comparing against both tells "stock" from
# "already active" from "the OS updated and the snapshot is stale".
STOCK_MD5="{stock_md5}"
PATCHED_MD5="{patched_md5}"
BUILT_FOR_DEVICE="{device}"

DEVICE=$(getprop ro.product.device)
if [ -n "$BUILT_FOR_DEVICE" ] && [ "$DEVICE" != "$BUILT_FOR_DEVICE" ]; then
    ui_print "! Built for $BUILT_FOR_DEVICE, this device is $DEVICE."
    ui_print "! The .pb files are tied to one Android build — rebuild on this device:"
    ui_print "!   imsforge pull && imsforge build"
    abort "! Aborted"
fi

SRC={CARRIER_SETTINGS}/others.pb
[ -f "$SRC" ] || abort "! $SRC not found"

ui_print "- Checking stock CarrierSettings"
CUR_MD5=$(md5sum "$SRC" | cut -d' ' -f1)
case "$CUR_MD5" in
    "$STOCK_MD5")
        ui_print "  matches the snapshot this module was built from"
        ;;
    "$PATCHED_MD5")
        ui_print "  already showing patched data (module active) — fine"
        ;;
    *)
        ui_print "! Stock others.pb matches neither the snapshot nor the patched file."
        ui_print "! An OS update probably refreshed Google's carrier data."
        ui_print "! This module will install, but it will shadow the new data with an old"
        ui_print "! snapshot. Rebuild: imsforge pull && imsforge build && imsforge install"
        ;;
esac

ui_print "- Carriers: {names}"
set_perm_recursive "$MODPATH" 0 0 0755 0644
"""


def cmd_build(args: argparse.Namespace) -> None:
    if not (STOCK_DIR / "others.pb").exists():
        raise SystemExit(f"no snapshot in {STOCK_DIR} — run `imsforge pull` first")

    stage = DIST_DIR / "module"
    if stage.exists():
        shutil.rmtree(stage)
    # The zip carries system/product/...; KernelSU relocates it to product/ at install time.
    settings_dir = stage / "system/product/etc/CarrierSettings"
    settings_dir.mkdir(parents=True)

    print(f"Patching {STOCK_DIR} -> {settings_dir}\n")
    patch.patch_tree(STOCK_DIR, settings_dir, carriers())

    for name in ("module.prop", "post-fs-data.sh", "uninstall.sh"):
        shutil.copy2(MODULE_TEMPLATE / name, stage / name)
    (stage / "customize.sh").write_text(
        render_customize(read_stock_meta(), md5(STOCK_DIR / "others.pb"), md5(settings_dir / "others.pb"))
    )

    zip_path = DIST_DIR / f"{MODULE_ID}.zip"
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as zf:
        for path in sorted(stage.rglob("*")):
            if path.is_file():
                zf.write(path, path.relative_to(stage))
    print(f"\nBuilt {zip_path} ({zip_path.stat().st_size // 1024} KB)")


def cmd_install(args: argparse.Namespace) -> None:
    zip_path = DIST_DIR / f"{MODULE_ID}.zip"
    if not zip_path.exists():
        raise SystemExit(f"{zip_path} is missing — run `imsforge build` first")

    # ksud installs over an already-staged module without clearing its directory, so files from
    # previous installs survive — and in a different layout (system/product vs product, ksud
    # relocates it). The result is a mix of versions applying on next boot. Clear it ourselves.
    adb(f"rm -rf /data/adb/modules_update/{MODULE_ID}", root=True, check=False)
    adb("push", str(zip_path), f"/data/local/tmp/{MODULE_ID}.zip")
    print(adb(f"ksud module install /data/local/tmp/{MODULE_ID}.zip", root=True))
    adb(f"rm -f /data/local/tmp/{MODULE_ID}.zip", root=True)

    built = {p.name: md5(p) for p in (DIST_DIR / "module").rglob("*.pb")}
    staged = adb(
        f"find /data/adb/modules_update/{MODULE_ID} -name '*.pb' -exec md5sum {{}} +",
        root=True,
        check=False,
    ).strip().splitlines()
    ok = len(staged) == len(built)
    for line in staged:
        digest, path = line.split()[0], line.split()[-1]
        name = path.rsplit("/", 1)[-1]
        match = built.get(name) == digest
        ok &= match
        print(f"  {name}: {'matches the build' if match else 'DOES NOT MATCH'}")
    print("Staged. Applies on next reboot." if ok else "! Staged files differ from the build")


def cmd_apply(args: argparse.Namespace) -> None:
    """Apply the patched protobufs right now, without rebooting.

    Temporary bind mount over /product plus a carrier config cache reset plus a telephony
    restart. The mounts come off as soon as the cache is rebuilt: telephony then runs off the
    new cached values, while the lasting effect comes from the module after a reboot.
    Cellular service drops for a few seconds.
    """
    stage = DIST_DIR / "module/system/product/etc/CarrierSettings"
    pbs = sorted(stage.glob("*.pb"))
    if not pbs:
        raise SystemExit("nothing built — run `imsforge build` first")

    # An interrupted run leaves its mounts in place and leftovers in /data/local/tmp with a
    # foreign SELinux label, which then makes adb push fail with Permission denied. Start clean.
    for pb in pbs:
        adb(
            f"for i in 1 2 3 4 5; do umount {CARRIER_SETTINGS}/{pb.name} 2>/dev/null || break; done",
            root=True,
            mount_master=True,
            check=False,
        )
        adb(f"rm -f /data/local/tmp/{pb.name}", root=True, check=False)

    # Take the label off a neighbouring stock file: under shell_data_file the carrier config
    # app cannot read the replacement.
    context = adb(f"ls -Z {CARRIER_SETTINGS}/carrier_list.pb", root=True).split()[0]
    print(f"SELinux context: {context}")

    for pb in pbs:
        adb("push", str(pb), f"/data/local/tmp/{pb.name}")
        adb(f"chcon {context} /data/local/tmp/{pb.name}", root=True)
        adb(f"chmod 644 /data/local/tmp/{pb.name}", root=True)
        adb(
            f"mount --bind /data/local/tmp/{pb.name} {CARRIER_SETTINGS}/{pb.name}",
            root=True,
            mount_master=True,
        )
        print(f"  mounted {pb.name}")

    print("Clearing the config cache and restarting telephony...")
    adb(f"rm -f {PHONE_FILES}/{CONFIG_CACHE_GLOB}", root=True)
    adb("kill \\$(pidof com.android.phone)", root=True, check=False)

    # Wait for the number of cache files to settle rather than for the first one: telephony
    # fetches each SIM's config separately and well apart in time. Unmounting too early leaves
    # the second SIM on stock config.
    glob = f"{PHONE_FILES}/{CONFIG_CACHE_GLOB}"
    waited = adb(
        f"prev=-1; stable=0; n=0; "
        f"while [ \\$n -lt 120 ]; do "
        f"cur=\\$(ls {glob} 2>/dev/null | wc -l); "
        f"if [ \\$cur = \\$prev ] && [ \\$cur -gt 0 ]; then stable=\\$((stable+1)); else stable=0; fi; "
        f"[ \\$stable -ge 5 ] && break; "
        f"prev=\\$cur; sleep 2; n=\\$((n+2)); "
        f"done; echo \\$cur \\$n",
        root=True,
    ).split()
    if len(waited) == 2:
        print(f"  cache rebuilt: {waited[0]} file(s) in {waited[1]}s")

    for pb in pbs:
        adb(f"umount {CARRIER_SETTINGS}/{pb.name}", root=True, mount_master=True, check=False)
        adb(f"rm -f /data/local/tmp/{pb.name}", root=True)
    print("Mounts removed, /product is stock again. Config applied from the cache.")


def cmd_verify(args: argparse.Namespace) -> None:
    """Post-reboot check: is /product shadowed, what does telephony see, did IMS come up."""
    ok = True

    print("=== module ===")
    listing = adb(f"ls /data/adb/modules/{MODULE_ID}/", root=True, check=False)
    print(f"  installed: {'yes' if listing.strip() else 'NO'}")
    ok &= bool(listing.strip())
    log = adb(f"cat /data/adb/modules/{MODULE_ID}/last-boot.log", root=True, check=False)
    for line in log.strip().splitlines():
        print(f"  {line}")

    print("\n=== mount backend ===")
    meta = adb("ls -l /data/adb/metamodule", root=True, check=False).strip()
    if meta:
        print(f"  metamodule: {meta.split('->')[-1].strip()}")
    else:
        print("  NO metamodule installed — modules that ship files are silently ignored.")
        print("  Install NoMount, Mountify or meta-overlayfs. See the README.")
        ok = False

    print("\n=== /product shadowed ===")
    expected = md5(DIST_DIR / "module/system/product/etc/CarrierSettings/others.pb")
    actual = adb(f"md5sum {CARRIER_SETTINGS}/others.pb", root=True).split()[0]
    shadowed = actual == expected
    print(f"  others.pb: {actual} {'(patched)' if shadowed else '(STOCK — not being shadowed)'}")
    ok &= shadowed

    print("\n=== carrier config ===")
    dump = adb("shell", "dumpsys carrier_config")
    for key in (
        "carrier_config_version_string",
        "carrier_volte_available_bool",
        "carrier_wfc_ims_available_bool",
        "carrier_nr_availabilities_int_array",
    ):
        values = sorted(set(re.findall(rf"^\s*{key} = (.*)$", dump, re.M)))
        print(f"  {key}: {', '.join(v.strip() or '<empty>' for v in values)}")
    ok &= "true" in re.findall(r"^\s*carrier_volte_available_bool = (.*)$", dump, re.M)

    print("\n=== IMS APN ===")
    for canonical in carriers():
        if not canonical.isdigit():
            continue
        rows = adb(
            "content query --uri content://telephony/carriers "
            f"--projection name:apn:type --where \\\"numeric=\\\\\\\"{canonical}\\\\\\\"\\\"",
            root=True,
            check=False,
        )
        for row in rows.strip().splitlines():
            if "type=ims" in row:
                print(f"  {canonical}: {row.strip()}")

    print("\n=== IMS PDN ===")
    # The sturdiest sign of working IMS: an APN of type ims in state CONNECTED with a P-CSCF
    # (the SIP proxy) handed out by the network. No P-CSCF, no registration.
    registry = adb("shell", "dumpsys telephony.registry")
    ims_up = False
    for chunk in registry.split("Pair{"):
        # every record ends at "network validation status:" — trim, or the tail of the next
        # record produces false positives
        chunk = chunk.split("network validation status:")[0]
        if not re.search(r",\s*ims,", chunk) or "state: CONNECTED" not in chunk:
            continue
        name = re.search(r"\[ApnSetting\] ([^,]+), \d+, (\d+)", chunk)
        transport = re.search(r"transport: (\w+)", chunk)
        pcscf = re.search(r"PcscfAddresses: \[([^\]]*)\]", chunk)
        label = f"{name.group(1)} (MCCMNC {name.group(2)})" if name else "ims APN"
        addrs = pcscf.group(1).strip() if pcscf else ""
        print(f"  {label}: CONNECTED over {transport.group(1) if transport else '?'}, "
              f"P-CSCF: {addrs or 'NONE'}")
        ims_up |= bool(addrs)
    if not ims_up:
        print("  no IMS PDN is up")
    ok &= ims_up

    print("\n=== IMS registration (radio log, if it has not rotated) ===")
    radio = adb("logcat -b radio -d -t 4000", root=True, check=False)
    states = re.findall(r"(Phone-\d)\s*: isImsRegistered =(\w+)", radio)
    if states:
        for phone, state in sorted(set(states)):
            print(f"  {phone}: isImsRegistered={state}")
    else:
        print("  nothing logged (these lines appear only when something queries the state)")

    print(f"\nResult: {'all good' if ok else 'problems above'}")


def main() -> None:
    parser = argparse.ArgumentParser(prog="imsforge", description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("carriers", help="resolve the SIMs in the phone to CarrierSettings names")
    sub.add_parser("pull", help="snapshot stock CarrierSettings (module must be disabled)")
    sub.add_parser("build", help="patch the protobufs and build the module zip")
    sub.add_parser("install", help="push the zip and install it through ksud")
    sub.add_parser("apply", help="apply the config now, without rebooting")
    sub.add_parser("verify", help="check the state after a reboot")
    args = parser.parse_args()
    {
        "carriers": cmd_carriers,
        "pull": cmd_pull,
        "build": cmd_build,
        "install": cmd_install,
        "apply": cmd_apply,
        "verify": cmd_verify,
    }[args.cmd](args)
