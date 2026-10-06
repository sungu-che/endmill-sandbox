use endmill_model::curation;
use endmill_model::pipeline::{IngestOptions, Models, Workspace};
use endmill_model::profile::{MachiningPreset, MachiningProfile, ProfileStore};
use endmill_model::sds::SdsStore;
use endmill_model::store::series::series_id;
use endmill_model::store::Store;
use std::path::PathBuf;

fn tmpdir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("endmill_store_flow_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn workspace_records_into_sqlite_and_lancedb() {
    let data = tmpdir("flow");
    let mut store = Store::open(&data.join("library")).unwrap();
    let mut sds = SdsStore::open(&data.join("sds"));
    let models = Models::new(&data.join("models"));
    let preset = MachiningPreset::default_steel_general();
    let p = MachiningProfile::from_preset(&preset, "강 범용");
    store.sync_builtin_presets(&ProfileStore::new().presets).unwrap();
    store.save_profile(&p, Some(&preset.name)).unwrap();
    let mut ws = Workspace::new(&data, p.clone());
    ws.set_ids(Some("T-100".into()), None, None);
    let proc = ws.run_process(&sds).unwrap();
    let log = store.record_process(&ws, &proc).unwrap();
    assert!(log[0].contains("공정 해석"), "{:?}", log);
    assert!(log.iter().any(|l| l.contains("LanceDB cut_regime")), "{:?}", log);
    let traj = proc.analysis.trajectory.clone();
    let mut csv = String::from("time(min),VB(mm)\n");
    for i in 0..14 {
        let t = traj.life_min * 1.3 * i as f64 / 13.0;
        csv.push_str(&format!("{:.4},{:.5}\n", t, traj.vb_at(t).min(0.5)));
    }
    let out = ws.ingest(&models, &mut sds, "vb.csv", csv.as_bytes(), &IngestOptions::default()).unwrap();
    assert!(!out.calibrations.is_empty(), "{:?}", out.sds);
    let log = store.record_ingest(&ws, &out).unwrap();
    assert!(log.iter().any(|l| l.contains("LanceDB")), "{:?}", log);
    assert!(log.iter().any(|l| l.contains("실측 수명")), "{:?}", log);
    let again = store.record_ingest(&ws, &out).unwrap();
    assert!(!again.iter().any(|l| l.contains("실측 수명")), "{:?}", again);

    let mut ws2 = Workspace::new(&data, p.clone());
    ws2.set_ids(Some("T-100".into()), None, None);
    assert!(store.restore_series(&mut ws2).unwrap() >= 1);
    assert_eq!(ws2.series["vb_mm"].len(), 14);

    let em = store.index_endmill(&p.endmill_setting).unwrap();
    let wp = store.index_workpiece(&p.workpiece_setup).unwrap();
    let stats = store.rdb.pair_stats(Some(em), Some(wp), None).unwrap();
    assert_eq!(stats.len(), 1);
    assert!(stats[0].life_meas.is_some());
    assert!(stats[0].wear_ratio.is_some());
    assert!(stats[0].runs >= 3, "runs {}", stats[0].runs);
    let presets = store.rdb.list_presets().unwrap();
    let row = presets.iter().find(|x| x.name == preset.name).unwrap();
    assert!(row.runs >= 2, "preset runs {}", row.runs);

    let sid = series_id(store.project_id, "T-100", "vb_mm");
    let wins = store.ts.as_ref().unwrap().windows_of(&sid).unwrap();
    assert!(!wins.is_empty());
    let probe = wins[0].vector.clone();
    let rep = curation::curate_for_workpiece(
        &mut store,
        &p.workpiece_setup,
        Some(&p.endmill_setting),
        &p.machine,
        &p.coolant_config,
        ws.purpose,
        None,
        Some(("vb_mm", &probe, None)),
        6,
    )
    .unwrap();
    assert!(!rep.candidates.is_empty());
    let cur = rep.candidates.iter().find(|c| c.is_current).expect("current end mill ranked");
    assert!(cur.history.is_some());
    assert!(cur.evidence.iter().any(|e| e.kind == "physics"));
    assert!(!rep.curves.is_empty());
    let regimes = store.similar_regimes(&proc.analysis.regime, 3, None).unwrap();
    assert!(regimes.iter().any(|r| r.endmill_id == em && r.workpiece_id == wp && r.similarity > 0.99), "{:?}", regimes);
    let status = store.status();
    assert!(status.vectors.iter().any(|c| c.name == "endmill_attr"));
    assert!(status.vectors.iter().any(|c| c.name == "cut_regime" && c.docs >= 1));
    assert!(status.lancedb.as_ref().map(|c| c.points >= 14).unwrap_or(false));
    store.flush();
    drop(store);
    let _ = std::fs::remove_dir_all(&data);
}