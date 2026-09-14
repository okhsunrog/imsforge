use crate::testing::tempdir;
use crate::tests::{args_in, write_stock};
use crate::*;

#[test]
fn invalid_names_and_ambiguous_overrides_are_rejected() {
    for name in [
        "../outside",
        "/tmp/outside",
        "..",
        "",
        "a/b",
        "others",
        "carrier_list",
    ] {
        let text = serde_json::json!({"carriers":[{"canonical_name":name}]}).to_string();
        assert!(Config::parse(&text).is_err(), "accepted {name}");
    }
    assert!(
        Config::parse(r#"{"carriers":[{"canonical_name":"a"},{"canonical_name":"a"}]}"#).is_err()
    );
    assert!(
        Config::parse(
            r#"{"carriers":[{"canonical_name":"a","bools":{"x":true},"int_arrays":{"x":[1]}}]}"#
        )
        .is_err()
    );
    assert!(Config::parse(r#"{"skip":["../outside"]}"#).is_err());
    assert!(Config::parse("{}").is_ok());
}

#[test]
fn failed_preparation_keeps_output_and_records_failure_on_this_boot() {
    let dir = tempdir("failed-apply");
    let args = args_in(&dir);
    write_stock(&args.sources.src, &[("25001", false)]);
    fs::write(&args.sims, "25001\n").unwrap();
    publish::run(&args, true, |_| Ok(())).unwrap();
    let old = fs::read(args.out.join("others.pb")).unwrap();
    let failure = publish::run(&args, true, |_| Err("labelling failed".into()));
    assert!(failure.is_err());
    assert_eq!(old, fs::read(args.out.join("others.pb")).unwrap());
    let status: Status =
        serde_json::from_slice(&fs::read(args.status.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(status.phase, "failed");
    assert_eq!(status.boot_id, status::boot_id());
    assert_eq!(status.error.as_deref(), Some("labelling failed"));
}

#[test]
fn invalid_config_cannot_leave_a_successful_boot_record() {
    let dir = tempdir("bad-config");
    let args = args_in(&dir);
    write_stock(&args.sources.src, &[("25001", false)]);
    fs::write(&args.sims, "25001\n").unwrap();
    publish::run(&args, true, |_| Ok(())).unwrap();
    fs::write(&args.config, "{bad").unwrap();
    assert!(publish::run(&args, true, |_| Ok(())).is_err());
    let status: Status =
        serde_json::from_slice(&fs::read(args.status.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(status.phase, "failed");
    assert!(args.out.join("others.pb").is_file());
}

#[test]
fn manual_generation_has_a_local_report_and_replaces_removed_targets() {
    let dir = tempdir("local-report");
    let mut args = args_in(&dir);
    args.status = None;
    write_stock(&args.sources.src, &[("25001", false)]);
    fs::write(&args.sims, "25001\n").unwrap();
    cmd_patch(&args).unwrap();
    let report: Status =
        serde_json::from_slice(&fs::read(args.out.join(".imsforge.json")).unwrap()).unwrap();
    assert_eq!(report.phase, "generated");
    assert!(!dir.join("status.json").exists());
    fs::write(&args.config, r#"{"auto":false}"#).unwrap();
    cmd_patch(&args).unwrap();
    assert!(!args.out.join("others.pb").exists());
}

#[test]
fn a_new_stock_generation_drops_removed_standalone_files() {
    let dir = tempdir("ota-cache");
    let args = args_in(&dir);
    write_stock(&args.sources.src, &[("25001", false)]);
    let standalone = args.sources.src.join("carrier_b.pb");
    fs::write(
        &standalone,
        protobuf::Message::write_to_bytes(&patch::stock_settings("carrier_b", true)).unwrap(),
    )
    .unwrap();
    fs::write(&args.sims, "25001\ncarrier_b\n").unwrap();
    cmd_patch(&args).unwrap();
    fs::remove_file(standalone).unwrap();
    write_stock(&args.sources.src, &[("25001", false), ("carrier_b", false)]);
    cmd_patch(&args).unwrap();
    let expected = fs::read(args.out.join("others.pb")).unwrap();
    assert!(!args.sources.stock_cache.join("carrier_b.pb").exists());
    fs::write(args.sources.src.join("others.pb"), &expected).unwrap();
    cmd_patch(&args).unwrap();
    assert_eq!(expected, fs::read(args.out.join("others.pb")).unwrap());
}

#[test]
fn cache_outputs_keep_both_previous_and_new_results_recognizable() {
    let dir = tempdir("output-registry");
    let args = args_in(&dir);
    write_stock(&args.sources.src, &[("25001", false), ("25002", false)]);
    fs::write(&args.sims, "25001\n").unwrap();
    cmd_patch(&args).unwrap();
    let previous = fs::read(args.out.join("others.pb")).unwrap();
    fs::write(args.sources.src.join("others.pb"), &previous).unwrap();
    fs::write(&args.config, r#"{"carriers":[{"canonical_name":"25002"}]}"#).unwrap();
    cmd_patch(&args).unwrap();
    let new = fs::read(args.out.join("others.pb")).unwrap();
    assert_ne!(new, previous);
    assert!(cache::is_ours(&args.sources.src, &args.sources.stock_cache));
    fs::write(args.sources.src.join("others.pb"), &new).unwrap();
    assert!(cache::is_ours(&args.sources.src, &args.sources.stock_cache));
}

#[test]
fn a_failed_exchange_does_not_remove_either_input() {
    let dir = tempdir("exchange");
    let stage = dir.join("stage");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("new"), b"new").unwrap();
    let dest = dir.join("old");
    fs::write(&dest, b"old").unwrap();
    assert!(atomic::replace_dir(&stage, &dest).is_err());
    assert_eq!(fs::read(dest).unwrap(), b"old");
    assert_eq!(fs::read(stage.join("new")).unwrap(), b"new");
}

#[test]
fn writers_cannot_share_a_generation_lock() {
    let dir = tempdir("lock");
    let path = dir.join("lock");
    let first = atomic::lock(&path).unwrap();
    assert!(atomic::lock(&path).is_err());
    drop(first);
    assert!(atomic::lock(&path).is_ok());
}

#[test]
fn sparse_sim_slots_are_preserved() {
    let sims = detect::pair_sims(",25001", ",MTS");
    assert_eq!(sims.len(), 1);
    assert_eq!(sims[0].slot, 1);
}

#[test]
fn fingerprints_compare_normalized_configurations() {
    assert_eq!(
        config_fingerprint(&Config::parse("{}").unwrap()).unwrap(),
        config_fingerprint(&Config::parse(r#"{"auto":true,"skip":[],"carriers":[]}"#).unwrap())
            .unwrap()
    );
    assert_ne!(
        config_fingerprint(&Config::default()).unwrap(),
        config_fingerprint(&Config::parse(r#"{"auto":false}"#).unwrap()).unwrap()
    );
}

#[test]
fn failed_ota_publication_keeps_the_old_outputs_own_stock_available() {
    let dir = tempdir("old-generation");
    let args = args_in(&dir);
    let original = write_stock(&args.sources.src, &[("25001", false)]);
    fs::write(&args.sims, "25001\n").unwrap();
    publish::run(&args, true, |_| Ok(())).unwrap();
    let old = fs::read(args.out.join("others.pb")).unwrap();
    write_stock(&args.sources.src, &[("25001", false), ("25002", false)]);
    assert!(publish::run(&args, true, |_| Err("label failed".into())).is_err());
    fs::write(args.sources.src.join("others.pb"), &old).unwrap();
    let effective = cache::effective_src(&args.sources.src, &args.sources.stock_cache);
    assert_ne!(effective, args.sources.src);
    assert_ne!(effective, args.sources.stock_cache);
    assert_eq!(original, fs::read(effective.join("others.pb")).unwrap());
}

#[test]
fn matching_others_does_not_hide_a_mismatched_standalone_output() {
    let dir = tempdir("full-manifest");
    let args = args_in(&dir);
    write_stock(&args.sources.src, &[]);
    let standalone = args.sources.src.join("carrier_b.pb");
    fs::write(
        &standalone,
        protobuf::Message::write_to_bytes(&patch::stock_settings("carrier_b", false)).unwrap(),
    )
    .unwrap();
    fs::write(&args.sims, "carrier_b\n").unwrap();
    cmd_patch(&args).unwrap();
    fs::write(
        args.sources.src.join("others.pb"),
        fs::read(args.out.join("others.pb")).unwrap(),
    )
    .unwrap();
    assert!(!cache::is_ours(
        &args.sources.src,
        &args.sources.stock_cache
    ));
    fs::write(
        &standalone,
        fs::read(args.out.join("carrier_b.pb")).unwrap(),
    )
    .unwrap();
    assert!(cache::is_ours(&args.sources.src, &args.sources.stock_cache));
}

#[test]
fn interrupted_archive_is_not_a_valid_stock_generation() {
    let dir = tempdir("incomplete-archive");
    let src = dir.join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("others.pb"), b"patched").unwrap();
    let cache = dir.join("cache");
    let archive = cache.with_extension("generations").join("interrupted");
    fs::create_dir_all(&archive).unwrap();
    fs::write(archive.join("others.pb"), b"stock").unwrap();
    fs::write(
        archive.join("output.fingerprint"),
        cache::fingerprint(b"patched").to_string(),
    )
    .unwrap();
    assert_eq!(cache::effective_src(&src, &cache), src);
}
