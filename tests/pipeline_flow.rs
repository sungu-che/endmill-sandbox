use endmill_model::ingest::{self, DocKind};
use endmill_model::pipeline::{IngestOptions, Models, Workspace};
use endmill_model::profile::{MachiningPreset, MachiningProfile};
use endmill_model::sds::{Scope, SdsStore, Track};
use endmill_model::toolimage::{self, ToolDims, ToolView};
use std::path::{Path, PathBuf};

fn fixtures() -> Option<PathBuf> {
    std::env::var("ENDMILL_FIXTURES").ok().map(PathBuf::from)
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("endmill_test_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn steel_profile() -> MachiningProfile {
    let p = MachiningPreset::default_steel_general();
    MachiningProfile::from_preset(&p, &p.name)
}

struct ToolDraw {
    px_per_mm: f64,
    d: f64,
    shank: f64,
    loc: f64,
    corner_r: f64,
    wear_um: f64,
    wear_len: f64,
}

fn inside(t: &ToolDraw, x: f64, z: f64) -> bool {
    if z < 0.0 {
        return false;
    }
    let mut r = if z <= t.loc {
        t.d / 2.0
    } else if z <= t.loc + 2.0 {
        let k = (z - t.loc) / 2.0;
        t.d / 2.0 * (1.0 - k) + t.shank / 2.0 * k
    } else {
        t.shank / 2.0
    };
    if z < t.wear_len {
        r -= t.wear_um / 1000.0 * (1.0 - 0.5 * z / t.wear_len);
    }
    let ax = x.abs();
    if ax > r {
        return false;
    }
    let cr = t.corner_r;
    if cr > 0.0 && z < cr && ax > r - cr {
        let dx = ax - (r - cr);
        let dz = cr - z;
        return dx * dx + dz * dz <= cr * cr;
    }
    true
}

fn draw_tool(t: &ToolDraw) -> Vec<u8> {
    let w_mm = 26.0;
    let h_mm = 60.0;
    let w = (w_mm * t.px_per_mm) as u32;
    let h = (h_mm * t.px_per_mm) as u32;
    let tip_y = h as f64 - 4.0 * t.px_per_mm;
    let cx = w as f64 / 2.0 + 0.37;
    let ss = 4;
    let mut img = image::RgbImage::new(w, h);
    for py in 0..h {
        for px in 0..w {
            let mut cov = 0.0;
            let mut tone = 0.0;
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (px as f64 + (sx as f64 + 0.5) / ss as f64 - cx) / t.px_per_mm;
                    let z = (tip_y - (py as f64 + (sy as f64 + 0.5) / ss as f64)) / t.px_per_mm;
                    if inside(t, x, z) {
                        cov += 1.0;
                        let stripe = ((z * 1.4 + x * 0.9).rem_euclid(3.0)) < 1.5;
                        tone += if z <= t.loc && stripe { 55.0 } else { 85.0 };
                    }
                }
            }
            let n = (ss * ss) as f64;
            let v = if cov > 0.0 { (tone / cov) * (cov / n) + 238.0 * (1.0 - cov / n) } else { 238.0 };
            let g = v.round().clamp(0.0, 255.0) as u8;
            img.put_pixel(px, py, image::Rgb([g, g, g.saturating_add(2)]));
        }
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}

#[test]
fn ingest_tool_text_units_and_shapes() {
    let doc = ingest::ingest(
        "spec.txt",
        "모델: XM-4F10\nSUS용 4날 엔드밀 Ø10 x LOC 25 x OAL 75, 샹크 10, 헬릭스 45°, AlTiN, 코너 R0.5, 런아웃 3um".as_bytes(),
        None,
    );
    assert_eq!(doc.kind, DocKind::ToolSpec, "{:?}", doc.issues);
    let t = doc.tool.expect("tool");
    assert!((t.diameter_mm - 10.0).abs() < 1e-9);
    assert_eq!(t.flute_count, 4);
    assert!((t.loc_mm - 25.0).abs() < 1e-9);
    assert!((t.oal_mm - 75.0).abs() < 1e-9);
    assert!((t.helix_angle_deg - 45.0).abs() < 1e-9);
    assert_eq!(t.nose.key(), "cr0.50");
    assert_eq!(t.runout_um, Some(3.0));
    assert!(doc.missing.is_empty(), "missing {:?}", doc.missing);
    let inch = ingest::ingest("spec2.txt", "2 flute end mill D=0.5 in, LOC 1.25 in".as_bytes(), Some(DocKind::ToolSpec));
    let ti = inch.tool.expect("inch tool");
    assert!((ti.diameter_mm - 12.7).abs() < 1e-9);
    assert!((ti.loc_mm - 31.75).abs() < 1e-9);
    assert!(inch.missing.contains(&"oal_mm".to_string()));
    let none = ingest::ingest("x.txt", "flute something".as_bytes(), Some(DocKind::ToolSpec));
    assert!(none.tool.is_none());
    assert!(none.issues.iter().any(|i| i.level == "error"));
}

#[test]
fn ingest_gcode_arcs_and_units() {
    let nc = "%\nO1000\nG21 G90 G17\nT1 M06\nS8000 M03\nG00 X0 Y0 Z5\nG01 Z-1 F300\nG01 X10 F1200\nG02 X20 Y0 R5\nG03 X10 Y0 I-5 J0\nG91 G01 X-5\nG90\nG00 Z5\nM30\n%\n";
    let doc = ingest::ingest("prog.nc", nc.as_bytes(), None);
    assert_eq!(doc.kind, DocKind::GCode);
    let g = doc.gcode.clone().unwrap();
    assert_eq!(g.arcs, 2);
    assert_eq!(g.spindle_rpm, Some(8000.0));
    assert_eq!(g.units, "mm");
    let segs = doc.segments.unwrap();
    let last_lin = segs.iter().rev().find(|s| s.kind() == "Linear").unwrap().end_point();
    assert!((last_lin.0 - 5.0).abs() < 1e-9, "incremental move {:?}", last_lin);
    let inch = "G20 G90\nG00 X0 Y0 Z0.2\nG01 Z-0.04 F10\nG01 X1.0 F40\n";
    let d2 = ingest::ingest("p.tap", inch.as_bytes(), None);
    let s2 = d2.segments.unwrap();
    assert!((s2.last().unwrap().end_point().0 - 25.4).abs() < 1e-9);
    assert!((s2.last().unwrap().feed().unwrap() - 1016.0).abs() < 1e-9);
}

#[test]
fn ingest_measurement_csv_units() {
    let csv = "time(min),VB(um),spindle load(%),feed_rate\n0,0,40,1000\n5,40,42,1000\n10,70,45,1000\n15,95,47,1000\n";
    let doc = ingest::ingest("log.csv", csv.as_bytes(), None);
    assert_eq!(doc.kind, DocKind::MeasurementLog, "{:?}", doc.issues);
    let vb = doc.series.iter().find(|s| s.key == "vb_mm").expect("vb");
    assert_eq!(vb.unit, "mm");
    assert_eq!(vb.t_unit, "s");
    assert!((vb.y[3] - 0.095).abs() < 1e-12);
    assert!((vb.t[2] - 600.0).abs() < 1e-9);
    assert!(doc.series.iter().any(|s| s.key == "spindle_load"));
    assert!(!doc.series.iter().any(|s| s.key == "roughness_um"));
}

#[test]
fn sds_hierarchy_and_shrinkage() {
    let dir = tmpdir("sds");
    let p = steel_profile();
    let scope = endmill_model::pipeline::process_scope(&p);
    {
        let mut s = SdsStore::open(&dir);
        assert!(s.calibration_factor(&scope, "wear").is_none());
        for _ in 0..3 {
            s.record_calibration(&scope, "wear", 2.0, 1.0);
        }
        let (f, n, _, key) = s.calibration_factor(&scope, "wear").unwrap();
        assert_eq!(n, 3);
        assert!(key.ends_with(&p.endmill_setting.signature().to_lowercase()), "{}", key);
        assert!((f - (1.0 + 3.0 / 8.0)).abs() < 1e-9, "{}", f);
        s.close_runs();
    }
    let s2 = SdsStore::open(&dir);
    let (f2, n2, _, _) = s2.calibration_factor(&scope, "wear").unwrap();
    assert_eq!(n2, 3);
    assert!((f2 - 1.375).abs() < 1e-9);
    let other = Scope::new(Track::Process, scope.primary.as_str(), "another-tool");
    let (f3, n3, _, k3) = s2.calibration_factor(&other, "wear").unwrap();
    assert_eq!(n3, 3);
    assert!(k3.ends_with('|'), "primary level key {}", k3);
    assert!((f3 - 1.375).abs() < 1e-9);
    let st = s2.status(20);
    assert!(st.ledgers.iter().any(|l| l.ledger == "process" && l.observations >= 3));
}

#[test]
fn tap_test_frf_is_stored_with_stickout() {
    let dir = tmpdir("frf");
    let mut ws = Workspace::new(&dir, steel_profile());
    let before = ws.summary().frf;
    assert!(before.fn_hz.is_none() && before.estimated_fn_hz > 500.0 && before.estimated_k_n_per_um > 0.5);
    assert!(ws.set_frf(Some(2400.0), None, None, false).is_err());
    assert!(ws.set_frf(None, None, Some(0.5), false).is_err());
    ws.set_frf(Some(2400.0), Some(7.5), Some(0.04), false).unwrap();
    let f = ws.summary().frf;
    assert_eq!(f.fn_hz, Some(2400.0));
    assert_eq!(f.k_n_per_um, Some(7.5));
    assert!((f.zeta - 0.04).abs() < 1e-12);
    assert_eq!(f.measured_stickout_mm, Some(f.stickout_mm));
    assert!(f.applies);
    let sds = SdsStore::open(&dir.join("sds"));
    let a = ws.ensure_analysis(&sds);
    assert!(a.stability.measured && (a.stability.fn_hz - 2400.0).abs() < 1e-6);
    let mut other = ws.profile.clone();
    other.endmill_setting.diameter_mm = 8.0;
    other.endmill_setting.shank_diameter_mm = 8.0;
    ws.set_profile(other);
    let f2 = ws.summary().frf;
    assert!(f2.fn_hz.is_some() && !f2.applies);
    assert!(!ws.ensure_analysis(&sds).stability.measured);
    ws.set_frf(None, None, None, true).unwrap();
    assert!(ws.summary().frf.fn_hz.is_none());
    assert!(!ws.ensure_analysis(&sds).stability.measured);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tool_silhouette_measures_radial_loss() {
    let px = 20.0;
    let base = ToolDraw { px_per_mm: px, d: 10.0, shank: 10.0, loc: 22.0, corner_r: 0.2, wear_um: 0.0, wear_len: 0.0 };
    let worn = ToolDraw { wear_um: 60.0, wear_len: 6.0, corner_r: 0.6, ..base };
    let dims = ToolDims { diameter_mm: 10.0, shank_mm: 10.0, loc_mm: 22.0, corner_r_mm: 0.2 };
    let a = toolimage::prepare(&draw_tool(&base), ToolView::Side, &dims, 64, None, None).unwrap();
    let b = toolimage::prepare(&draw_tool(&worn), ToolView::Side, &dims, 64, None, None).unwrap();
    let sa = a.side.unwrap();
    let sb = b.side.unwrap();
    println!("px/mm {:.3} {} / corner {:.3} -> {:.3}", sa.px_per_mm, sa.calibration, sa.corner_r_left_mm, sb.corner_r_left_mm);
    assert!((sa.px_per_mm - px).abs() / px < 0.02, "scale {}", sa.px_per_mm);
    assert!(sb.corner_r_left_mm > sa.corner_r_left_mm + 0.15);
    let probe = |s: &toolimage::SideProfile, z: f64| -> f64 {
        let i = s.z_mm.iter().enumerate().min_by(|x, y| (x.1 - z).abs().partial_cmp(&(y.1 - z).abs()).unwrap()).unwrap().0;
        (s.left_mm[i] + s.right_mm[i]) / 2.0
    };
    let loss_2mm = (probe(&sa, 2.0) - probe(&sb, 2.0)) * 1000.0;
    let loss_10mm = (probe(&sa, 10.0) - probe(&sb, 10.0)) * 1000.0;
    println!("radial loss @2mm {:.1} um, @10mm {:.1} um", loss_2mm, loss_10mm);
    assert!((loss_2mm - 50.0).abs() < 15.0, "loss at 2mm {}", loss_2mm);
    assert!(loss_10mm.abs() < 8.0, "loss at 10mm {}", loss_10mm);
    assert_eq!(a.zone_of_patch.len(), 16);
}

fn link_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let to = dst.join(e.file_name());
        if p.is_dir() {
            link_dir(&p, &to);
        } else {
            std::fs::copy(&p, &to).unwrap();
        }
    }
}

fn write_word_tokenizer(path: &Path) {
    let mut vocab = serde_json::Map::new();
    for (i, t) in ["[UNK]", "[CLS]", "[SEP]", "[PAD]", "[MASK]"].iter().enumerate() {
        vocab.insert(t.to_string(), serde_json::json!(i));
    }
    let words = "end mill machining state workpiece tool coolant operation finishing roughing pass spindle power torque deflection temperature limit ok watch over critical keep reduce feed rate speed change inspect replace stop question choice which action the a of and to";
    for (i, w) in words.split_whitespace().enumerate() {
        vocab.insert(w.to_string(), serde_json::json!(5 + i));
    }
    let tok = serde_json::json!({
        "version": "1.0",
        "truncation": null,
        "padding": null,
        "added_tokens": [],
        "normalizer": {"type": "Lowercase"},
        "pre_tokenizer": {"type": "Whitespace"},
        "post_processor": null,
        "decoder": null,
        "model": {"type": "WordLevel", "vocab": vocab, "unk_token": "[UNK]"}
    });
    std::fs::write(path, serde_json::to_string(&tok).unwrap()).unwrap();
}

#[test]
fn workspace_end_to_end() {
    let data = tmpdir("ws");
    let mut sds = SdsStore::open(&data.join("sds"));
    let models_root = tmpdir("models");
    let mut models = Models::new(&models_root);
    if let Some(fx) = fixtures() {
        link_dir(&fx.join("siglip_small"), &models_root.join("siglip2-test"));
        link_dir(&fx.join("ttm_r3_52_16"), &models_root.join("granite-timeseries-ttm-r3").join("52-16-dec-52-r3"));
        link_dir(&fx.join("laya_small"), &models_root.join("laya-typed-decisions"));
        write_word_tokenizer(&models_root.join("laya-typed-decisions").join("tokenizer.json"));
        println!("siglip: {:?}", models.load_siglip());
        println!("ttm: {:?}", models.load_ttm());
        println!("laya: {:?}", models.load_laya());
        assert!(models.siglip.is_some());
        assert!(models.ttm.is_some());
        assert!(models.laya.is_some());
    }
    let mut ws = Workspace::new(&data, steel_profile());
    ws.set_pattern("pocket_zigzag").unwrap();
    let proc1 = ws.run_process(&sds).unwrap();
    println!(
        "process: P={:.2}kW F={:.0}N T={:.0}C life={:.1}min sim {:.2}min episodes {}",
        proc1.analysis.spindle_power_kw,
        proc1.analysis.forces.f_res_peak,
        proc1.analysis.thermal.interface_c,
        proc1.analysis.wear.tool_life_min,
        proc1.sim.total_time_min,
        proc1.sim.episodes.len()
    );
    assert!(proc1.sim.samples.len() <= 1001);
    assert_eq!(proc1.coolant_matrix.len(), 5);
    assert!(proc1.coolant_matrix.iter().filter(|r| r.recommended).count() <= 1);
    assert!(proc1.nominal_mold.as_ref().unwrap().descriptor.max_depth_mm > 0.0);
    let rate = proc1.analysis.wear.vb_rate_mm_per_min;
    let traj = proc1.analysis.trajectory.clone();
    assert!(traj.life_min < traj.linear_life_min, "life {} linear {}", traj.life_min, traj.linear_life_min);
    let mut csv = String::from("time(min),VB(mm),spindle power(kW)\n");
    for i in 0..12 {
        let t = i as f64 * 4.0;
        let vb = 0.01 + 1.6 * traj.growth_from(0.01, t);
        csv.push_str(&format!("{},{:.5},{:.3}\n", t, vb, proc1.analysis.spindle_power_kw * 1.25));
    }
    let opt = IngestOptions::default();
    let out = ws.ingest(&models, &mut sds, "vb_log.csv", csv.as_bytes(), &opt).unwrap();
    println!("log applied {:?} sds {:?}", out.doc.applied, out.sds);
    assert!(out.sds.iter().any(|l| l.contains("'wear'")), "{:?}", out.sds);
    assert!(out.sds.iter().any(|l| l.contains("'power'")), "{:?}", out.sds);
    let again = ws.ingest(&models, &mut sds, "vb_log.csv", csv.as_bytes(), &opt).unwrap();
    assert!(again.doc.duplicate);
    assert!(!again.sds.iter().any(|l| l.contains("'wear'")));
    for k in 0..2 {
        let mut csv2 = String::from("time(min),VB(mm)\n");
        for i in 0..12 {
            let t = i as f64 * 5.0 + k as f64 * 0.1;
            csv2.push_str(&format!("{},{:.5}\n", t, 0.01 + 1.6 * traj.growth_from(0.01, t)));
        }
        ws.ingest(&models, &mut sds, &format!("vb_log_{}.csv", k), csv2.as_bytes(), &opt).unwrap();
    }
    let proc2 = ws.run_process(&sds).unwrap();
    println!("calibration applied: {:?}", proc2.calibration);
    let wear_cal = proc2.calibration.iter().find(|c| c.axis == "wear").expect("wear calibration applied");
    assert!(wear_cal.factor > 1.15 && wear_cal.factor < 1.6, "factor {}", wear_cal.factor);
    assert!(proc2.analysis.wear.vb_rate_mm_per_min > rate * 1.1);
    let f = ws.forecast(&models, &sds, "vb_mm", 16).unwrap();
    println!("forecast method {} threshold {:?} notes {:?}", f.method, f.threshold, f.notes);
    assert_eq!(f.point.len(), if models.ttm.is_some() { 16 } else { 16 });
    if models.ttm.is_some() {
        assert_eq!(f.method, "ttm-r3");
    }
    assert!(f.threshold.is_some());
    if models.siglip.is_some() {
        let base = ToolDraw { px_per_mm: 20.0, d: 10.0, shank: 10.0, loc: 25.0, corner_r: 0.2, wear_um: 0.0, wear_len: 0.0 };
        let worn = ToolDraw { wear_um: 60.0, wear_len: 6.0, corner_r: 0.6, ..base };
        let mut topt = IngestOptions::default();
        topt.role = Some("reference".into());
        topt.tool_id = Some("T1-001".into());
        let r = ws.ingest(&models, &mut sds, "ref.png", &draw_tool(&base), &topt).unwrap();
        assert!(r.tool_image.is_some(), "{:?}", r.doc.issues);
        topt.role = Some("current".into());
        topt.cut_minutes = Some(30.0);
        let c = ws.ingest(&models, &mut sds, "cur.png", &draw_tool(&worn), &topt).unwrap();
        let ti = c.tool_image.expect("current image");
        let cmp = ti.comparison.unwrap();
        println!(
            "wear state {:?} reasons {:?} measured {:?} predicted {:?} identity {:?} sds {:?}",
            cmp.state, cmp.state_reasons, ti.measured_radial_um, ti.predicted_radial_um, ti.identity, ti.sds
        );
        assert_eq!(cmp.patch_distance.len(), 16);
        let g = cmp.geometry.unwrap();
        println!("flank from {:.2} mm, max flank loss {:.1} um, corner {:.3} -> {:.3}", g.flank_from_mm, g.max_loss_um, g.corner_r_ref_mm, g.corner_r_cur_mm);
        assert!(g.max_loss_um > 40.0 && g.max_loss_um < 70.0, "flank max {}", g.max_loss_um);
        assert!(g.corner_r_cur_mm > g.corner_r_ref_mm + 0.25);
        assert!(ti.measured_radial_um.unwrap() > 20.0);
        assert!(ws.series.contains_key("radial_loss_um"));
        ws.run_process(&sds).unwrap();
    }
    let nominal = ws.nominal_mold.clone().unwrap();
    let mut measured = nominal.clone();
    for v in measured.z.iter_mut() {
        if v.is_finite() {
            *v += 0.012;
        }
    }
    let mut mopt = IngestOptions::default();
    mopt.mold_id = Some("CAV-01".into());
    let mut grid_csv = String::new();
    for j in (0..measured.ny).rev() {
        let row: Vec<String> = (0..measured.nx)
            .map(|i| {
                let v = measured.z[j * measured.nx + i];
                if v.is_finite() { format!("{:.4}", v) } else { "0".into() }
            })
            .collect();
        grid_csv.push_str(&row.join(","));
        grid_csv.push('\n');
    }
    mopt.hint = Some("mold_csv".into());
    mopt.cell_mm = Some(measured.cell_mm);
    mopt.origin_x = Some(measured.x0);
    mopt.origin_y = Some(measured.y0);
    let m_out = ws.ingest(&models, &mut sds, "measured.csv", grid_csv.as_bytes(), &mopt).unwrap();
    let mg = m_out.mold_geometry.expect("mold geometry");
    println!("mold issues {:?} fit_error {:?}", m_out.doc.issues, mg.fit_error);
    let fit = mg.fit.expect("deviation fit");
    let axial = fit.coefficients.iter().find(|c| c.name == "axial_offset_um").unwrap();
    println!("axial offset {:.2} um (r2 {:.3}, pts {})", axial.value, fit.r2, fit.points);
    assert!((axial.value - 12.0).abs() < 2.0, "axial {}", axial.value);
    assert!(mg.comparison.is_some());
    let rep = ws.decide(&models, &mut sds, true).unwrap();
    println!("decision: rule {:?} final {:?} advisor {:?} err {:?}", rep.rule_action, rep.final_action, rep.advisor_action, rep.advisor_error);
    for g in rep.gates.iter() {
        println!("  gate {} sev {} {:.3}/{:.3} {}", g.id, g.severity, g.value, g.limit, g.message);
    }
    assert!(rep.gates.iter().any(|g| g.id == "forecast"));
    assert!(rep.gates.iter().any(|g| g.id == "mold_residual"));
    if models.laya.is_some() {
        assert!(rep.advisor.is_some(), "{:?}", rep.advisor_error);
    }
    let changes = ws.apply_decision().unwrap();
    println!("apply: {:?}", changes);
    let proc3 = ws.run_process(&sds).unwrap();
    let ad = ws.adaptive_program().unwrap();
    println!("adaptive: scaled {} min {:.2} time {:.2}->{:.2}", ad.scaled_segments, ad.min_scale, ad.time_base_min, ad.time_adapted_min);
    assert!(ad.text.contains("M30"));
    assert!(proc3.sim.total_time_min > 0.0);
    sds.close_runs();
    let st = sds.status(40);
    println!("sds ledgers {:?}", st.ledgers);
    assert!(data.join("sds").join("process.json").exists());
    let summary = ws.summary();
    assert!(summary.series.iter().any(|s| s.key == "vb_mm"));
}