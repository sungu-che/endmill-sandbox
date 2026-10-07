use candle_core::{DType, Device, Tensor};
use endmill_model::ml::laya::LayaAdvisor;
use endmill_model::ml::ttm::{TtmConfig, TtmForecaster};
use endmill_model::physics::{self, Engagement};
use endmill_model::profile::{MachiningPreset, MachiningProfile};
use endmill_model::sds::SdsStore;
use endmill_model::wearcomp::{self, CompAction, CompOptions};
use endmill_model::wearlog;
use std::collections::HashMap;
use std::path::PathBuf;

const TTM_R3_CONFIG: &str = r#"{"adaptive_patching_levels":0,"categorical_vocab_size_list":null,"combine_quantiles_via_variance":true,"context_length":52,"d_model":128,"decoder_adaptive_patching_levels":0,"decoder_d_model":64,"decoder_mode":"common_channel","decoder_num_layers":4,"decoder_raw_residual":false,"decompose":true,"distribution_output":"student_t","dropout":0.2,"enable_base_norm_always":false,"enable_forecast_channel_mixing":false,"exogenous_channel_indices":null,"expansion_factor":3,"fcm_context_length":1,"fcm_gated_attn":true,"fcm_mix_layers":2,"fcm_prepend_past":true,"fcm_prepend_past_offset":null,"fcm_use_mixer":false,"fft_length":0,"forecast_loss_type":"joint","frequency_token_vocab_size":5,"gated_attn":true,"head_dropout":0.2,"huber_delta":1,"init_embed":"pytorch","init_linear":"pytorch","init_processing":false,"init_std":0.02,"joint_loss_weight":1,"light_mode":false,"loss":"mae","mask_value":0,"masked_context_length":null,"mode":"common_channel","model_type":"tinytimemixer","mq_cond_mode":"concat","mq_cond_path":"flatten","mq_decoder_d_model":4,"mq_eps":1e-06,"mq_hidden":8,"mq_kernel_size":3,"mq_q50_type":"mean","mq_use_decoder_pool":true,"mq_use_positional":false,"multi_quantile_head":true,"multi_scale":true,"norm_eps":1e-05,"norm_mlp":"LayerNorm","num_input_channels":1,"num_layers":10,"num_parallel_samples":100,"patch_last":true,"patch_length":4,"patch_stride":4,"penalize_large_width_ratio":0.0,"point_extra_weight":2,"positional_encoding_type":"sincos","post_init":false,"prediction_channel_indices":null,"prediction_filter_length":null,"prediction_length":16,"quantile":0.5,"quantile_levels":[0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8,0.9],"register_tokens":2,"residual_context_length":52,"residual_loss_weight":1,"resolution_prefix_tuning":false,"scaling":"std","self_attn":false,"self_attn_heads":1,"transformers_version":"4.38.0","trend_adaptive_patching_levels":null,"trend_d_model":26,"trend_decoder_d_model":26,"trend_decoder_num_layers":2,"trend_fft_length":null,"trend_head_d_model":null,"trend_loss_weight":1,"trend_multi_scale":null,"trend_num_layers":10,"trend_patch_length":13,"trend_patch_stride":13,"trend_register_tokens":1,"use_decoder":true,"use_fft_embedding":true,"use_positional_encoding":false,"width_penalty_mode":"boundary"}"#;

fn preset(name: &str) -> MachiningProfile {
    let p = MachiningPreset::builtin().into_iter().find(|p| p.name.contains(name)).expect("preset");
    MachiningProfile::from_preset(&p, &p.name)
}

fn titanium_finishing() -> MachiningProfile {
    let mut p = preset("티타늄");
    p.conditions.cutting_speed_m_min = 66.0;
    p.conditions.feed_rate_mm_min = 321.5;
    p.conditions.feed_per_tooth_mm = 0.0383;
    p.conditions.axial_doc_mm = 3.2;
    p.conditions.radial_doc_mm = 0.3;
    p.conditions.spindle_rpm = 2101;
    p
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("endmill-wearcomp-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn zero_ttm() -> TtmForecaster {
    let cfg: TtmConfig = serde_json::from_str(TTM_R3_CONFIG).unwrap();
    let vb = candle_nn::VarBuilder::zeros(DType::F32, &Device::Cpu);
    TtmForecaster::from_varbuilder(cfg, vb, PathBuf::from("zero-ttm")).unwrap()
}

fn tiny_laya(dir: &PathBuf) -> LayaAdvisor {
    let dev = Device::Cpu;
    let (v, h, inter, layers) = (600usize, 128usize, 192usize, 4usize);
    let mut t: HashMap<String, Tensor> = HashMap::new();
    let rnd = |shape: &[usize]| Tensor::randn(0f32, 0.02, shape, &dev).unwrap();
    let ones = |n: usize| Tensor::ones(n, DType::F32, &dev).unwrap();
    let zeros = |n: usize| Tensor::zeros(n, DType::F32, &dev).unwrap();
    t.insert("encoder.embeddings.tok_embeddings.weight".into(), rnd(&[v, h]));
    t.insert("encoder.embeddings.norm.weight".into(), ones(h));
    for i in 0..layers {
        let p = format!("encoder.layers.{}", i);
        if i > 0 {
            t.insert(format!("{}.attn_norm.weight", p), ones(h));
        }
        t.insert(format!("{}.attn.Wqkv.weight", p), rnd(&[3 * h, h]));
        t.insert(format!("{}.attn.Wo.weight", p), rnd(&[h, h]));
        t.insert(format!("{}.mlp_norm.weight", p), ones(h));
        t.insert(format!("{}.mlp.Wi.weight", p), rnd(&[2 * inter, h]));
        t.insert(format!("{}.mlp.Wo.weight", p), rnd(&[h, inter]));
    }
    t.insert("encoder.final_norm.weight".into(), ones(h));
    for i in 0..2 {
        let p = format!("head.layers.{}", i);
        t.insert(format!("{}.self_attn.in_proj_weight", p), rnd(&[3 * h, h]));
        t.insert(format!("{}.self_attn.in_proj_bias", p), zeros(3 * h));
        t.insert(format!("{}.self_attn.out_proj.weight", p), rnd(&[h, h]));
        t.insert(format!("{}.self_attn.out_proj.bias", p), zeros(h));
        t.insert(format!("{}.norm1.weight", p), ones(h));
        t.insert(format!("{}.norm1.bias", p), zeros(h));
        t.insert(format!("{}.norm2.weight", p), ones(h));
        t.insert(format!("{}.norm2.bias", p), zeros(h));
        t.insert(format!("{}.linear1.weight", p), rnd(&[4 * h, h]));
        t.insert(format!("{}.linear1.bias", p), zeros(4 * h));
        t.insert(format!("{}.linear2.weight", p), rnd(&[h, 4 * h]));
        t.insert(format!("{}.linear2.bias", p), zeros(h));
    }
    t.insert("type_emb.weight".into(), rnd(&[3, h]));
    t.insert("scorer.0.weight".into(), ones(h));
    t.insert("scorer.0.bias".into(), zeros(h));
    t.insert("scorer.1.weight".into(), rnd(&[h, h]));
    t.insert("scorer.1.bias".into(), zeros(h));
    t.insert("scorer.3.weight".into(), rnd(&[1, h]));
    t.insert("scorer.3.bias".into(), zeros(1));
    t.insert("act_head.0.weight".into(), rnd(&[256, h + 4]));
    t.insert("act_head.0.bias".into(), zeros(256));
    t.insert("act_head.2.weight".into(), rnd(&[2, 256]));
    t.insert("act_head.2.bias".into(), zeros(2));
    t.insert("temperature".into(), Tensor::new(&[1.0f32, 1.0, 1.0], &dev).unwrap());
    candle_core::safetensors::save(&t, dir.join("model.safetensors")).unwrap();
    std::fs::create_dir_all(dir.join("encoder")).unwrap();
    std::fs::write(
        dir.join("encoder").join("config.json"),
        r#"{"vocab_size":600,"hidden_size":128,"intermediate_size":192,"num_hidden_layers":4,"num_attention_heads":2,"local_attention":16,"global_attn_every_n_layers":3,"global_rope_theta":160000.0,"local_rope_theta":10000.0,"norm_eps":1e-5,"norm_bias":false,"attention_bias":false,"mlp_bias":false,"max_position_embeddings":512,"pad_token_id":0,"cls_token_id":1,"sep_token_id":2}"#,
    )
    .unwrap();
    std::fs::write(dir.join("rl_agent_config.json"), r#"{"head_layers":2,"max_len":384,"head_max_len":128,"act_costs":{"escalate":0.5}}"#).unwrap();
    let mut vocab: Vec<String> = vec!["[PAD]", "[CLS]", "[SEP]", "[UNK]", "[MASK]"].into_iter().map(String::from).collect();
    for w in "choose the safest way to run optional second compensation pass that corrects wall size left by tool wear and deflection after first without damaging workpiece or end mill one light at planned offset feed reduced limit cutting force split into an final spring skip accept current replace worn then stop measure part before any further question choice um mm".split_whitespace() {
        if !vocab.iter().any(|x| x == w) {
            vocab.push(w.to_string());
        }
    }
    while vocab.len() < 600 {
        let n = vocab.len();
        vocab.push(format!("tok{}", n));
    }
    let map: Vec<String> = vocab.iter().enumerate().map(|(i, w)| format!("{:?}:{}", w, i)).collect();
    let tok_json = format!(
        r#"{{"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":{{"type":"Lowercase"}},"pre_tokenizer":{{"type":"Whitespace"}},"post_processor":null,"decoder":null,"model":{{"type":"WordLevel","vocab":{{{}}},"unk_token":"[UNK]"}}}}"#,
        map.join(",")
    );
    std::fs::create_dir_all(dir.join("tokenizer")).unwrap();
    std::fs::write(dir.join("tokenizer").join("tokenizer.json"), tok_json).unwrap();
    LayaAdvisor::load(dir, &dev).unwrap()
}

#[test]
fn titanium_pocket_plans_a_split_second_pass_without_touching_pass_one() {
    let p = preset("티타늄");
    let dir = temp_dir("ti");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let mut last = -1.0;
    let mut monotonic = true;
    let r = wearcomp::run_pipeline(&p, "pocket_zigzag", &mut sds, None, None, &CompOptions::default(), None, None, &mut |f, _| {
        if f + 1e-12 < last {
            monotonic = false;
        }
        last = f;
        true
    })
    .unwrap();
    assert!(monotonic && (last - 1.0).abs() < 1e-9);
    assert!(r.report.final_wall_count > 100 && r.report.transient_wall_count > r.report.final_wall_count);
    assert!(r.log.vb_mm.len() >= wearlog::MIN_POINTS);
    assert!(r.log.vb_mm.windows(2).all(|w| w[1] >= w[0] - 1e-12));
    assert!(r.log.histograms.iter().any(|h| h.key == "wall") && r.log.histograms.iter().any(|h| h.key == "mrr"));
    assert!(r.log.backtests.iter().any(|b| b.method == "holt") && r.log.backtests.iter().any(|b| b.method == "physics"));
    let w: f64 = r.log.backtests.iter().map(|b| b.weight).sum();
    assert!((w - 1.0).abs() < 1e-9);
    let o = &r.outcome;
    let d = &o.draft;
    assert!(d.need && d.zones.iter().any(|z| z.needs));
    assert!(d.gates.iter().any(|g| g.id == "overcut_pass1" && g.severity == 2));
    assert!(d.gates.iter().any(|g| g.id == "pass2_deflection" && g.blocks.contains(&CompAction::Execute)));
    assert_eq!(o.decision.final_action, CompAction::Split);
    assert!(o.decision.operator_confirm);
    assert!(o.after_min_um > -d.residual.tolerance_um, "overcut after pass 2: {}", o.after_min_um);
    assert!(o.after_p50_um.abs() < d.residual.p50_um.abs().max(d.residual.tolerance_um));
    let text = &o.program_text;
    assert!(text.contains("G04 P") && text.contains("M00") && text.trim_end().ends_with('%'));
    assert!(text.contains("G02") || text.contains("G03"));
    assert!(text.contains("보정 패스") && text.contains("스프링 패스"));
    assert!(o.segments.len() == o.segment_end_s.len() && o.segment_end_s.windows(2).all(|w| w[1] >= w[0]));
    assert!(!o.samples.is_empty() && o.samples.iter().any(|s| s.force_n > 0.0));
    let pass1 = endmill_model::gcode::GCodeGenerator::program_from_segments(&p, &r.segments, "P1").to_text();
    assert!(!pass1.contains("COMP2") && !pass1.contains("G04"));
    let status = sds.status(400);
    let wear = status.ledgers.iter().find(|l| l.ledger == "wear").unwrap();
    assert!(wear.run_observations > 0);
    assert!(!sds.gate_rates(&wearcomp::wear_scope(&p)).is_empty());
}
#[test]
fn aluminum_profile_within_tolerance_skips_and_saves_logs() {
    let p = preset("알루미늄 황삭");
    let dir = temp_dir("al");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let r = wearcomp::run_pipeline(&p, "rect_profile", &mut sds, None, None, &CompOptions::default(), None, None, &mut |_, _| true).unwrap();
    assert!(!r.outcome.draft.need);
    assert_eq!(r.outcome.decision.final_action, CompAction::Skip);
    assert!(r.outcome.program_text.is_empty());
    assert!(r.report.max_workpiece_defl_um < 1000.0, "thin wall artifact {}", r.report.max_workpiece_defl_um);
    let csv = wearlog::bins_csv(&r.report.wear_log);
    assert_eq!(csv.lines().count(), r.report.wear_log.len() + 1);
    let summary = serde_json::json!({ "line": r.outcome.report_line, "log": r.log });
    let files = wearcomp::save_job_files(&dir.join("wear_logs"), &wearcomp::utc_stamp(wearcomp::now_ms()), &p.name, &csv, &summary);
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|f| std::path::Path::new(f).exists()));
}

#[test]
fn measured_radial_wear_raises_offsets_and_calibrates() {
    let p = preset("스테인리스");
    let dir = temp_dir("ss");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let (ctx, _) = endmill_model::pipeline::calibrated_context(&p, Some(&sds));
    let (_, rep, segs) = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |_| true).unwrap();
    let log = wearcomp::analyze_job(&p, &ctx, &rep, None, &sds);
    let base = wearcomp::evaluate(&p, &ctx, &rep, &log, &CompOptions::default(), None);
    let mut opts = CompOptions::default();
    opts.measured_radial_um = Some(rep.radial_loss_end_um + 15.0);
    let measured = wearcomp::evaluate(&p, &ctx, &rep, &log, &opts, None);
    assert_eq!(measured.radial_source, "measured");
    let off = |d: &wearcomp::CompDraft| d.zones.iter().filter(|z| z.needs).map(|z| z.offset_um).fold(0.0, f64::max);
    assert!(off(&measured) > off(&base) + 5.0, "{} vs {}", off(&measured), off(&base));
    let decision = wearcomp::consult(&measured, None, None, true);
    assert_eq!(decision.decided_by, "rule");
    assert!(decision.laya_error.is_some());
    let out = wearcomp::finalize(&p, &ctx, &rep, &segs, &log, &measured, &decision, &opts);
    let lines = wearcomp::record(&mut sds, &p, &log, &out, Some("skip"));
    assert!(lines.iter().any(|l| l.contains("보정 계수")));
    let ps = endmill_model::pipeline::process_scope(&p);
    for _ in 0..3 {
        wearcomp::record(&mut sds, &p, &log, &out, None);
    }
    assert!(sds.calibration_factor(&ps, "wear").is_some());
}

#[test]
fn ttm_and_laya_models_are_actually_used_in_the_job() {
    let p = preset("티타늄");
    let dir = temp_dir("models");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let ttm = zero_ttm();
    let laya_dir = dir.join("laya-tiny");
    std::fs::create_dir_all(&laya_dir).unwrap();
    let laya = tiny_laya(&laya_dir);
    let r = wearcomp::run_pipeline(&p, "pocket_zigzag", &mut sds, Some(&ttm), Some(&laya), &CompOptions::default(), None, None, &mut |_, _| true).unwrap();
    assert!(r.log.ttm_used);
    assert!(r.log.backtests.iter().any(|b| b.method == "ttm"));
    assert!(r.log.forecast.as_ref().map(|f| f.method.contains("ttm")).unwrap_or(false));
    let dec = &r.outcome.decision;
    let ans = dec.laya.as_ref().expect("laya answer");
    assert_eq!(ans.labels.len(), dec.allowed.len());
    assert!((ans.probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    assert!(dec.allowed.contains(&dec.final_action));
    assert!(dec.final_action.rank() >= dec.rule_action.rank());
    assert!(dec.laya_shape.is_some());
}

#[test]
fn titanium_finishing_leaves_sub_edge_radius_zones_and_runs_one_pass() {
    let p = titanium_finishing();
    let dir = temp_dir("ti-fin");
    let mut sds = SdsStore::open(&dir.join("sds"));
    let r = wearcomp::run_pipeline(&p, "pocket_zigzag", &mut sds, None, None, &CompOptions::default(), None, None, &mut |_, _| true).unwrap();
    let o = &r.outcome;
    let d = &o.draft;
    let thin: Vec<&wearcomp::CompZone> = d.zones.iter().filter(|z| z.thin).collect();
    assert!(!thin.is_empty() && thin.iter().all(|z| !z.needs && z.execute.h_max_um.min(z.fresh.h_max_um) < d.h_min_um));
    assert!(d.gates.iter().any(|g| g.id == "min_engagement" && g.severity == 1 && g.blocks.is_empty()));
    assert!(d.zones.iter().filter(|z| z.needs).all(|z| z.execute.h_max_um.min(z.fresh.h_max_um) >= d.h_min_um - 1e-9));
    let chip = d.gates.iter().find(|g| g.id == "min_chip").unwrap();
    assert!(chip.blocks.contains(&CompAction::ExecuteReduced) && !chip.blocks.contains(&CompAction::Execute));
    assert_eq!(o.decision.final_action, CompAction::Execute);
    let tol = d.residual.tolerance_um;
    assert!(d.zones.iter().filter(|z| z.needs).all(|z| {
        let (_, mn, mx) = z.after_for(CompAction::Execute);
        mn > -tol && mx < tol
    }));
    assert!(o.after_max_um >= thin.iter().map(|z| z.residual_max_um).fold(f64::NEG_INFINITY, f64::max) - 1e-9);
    let rpm = p.conditions.spindle_rpm as f64;
    let flutes = p.endmill_setting.flute_count as f64;
    let feeds: Vec<String> = d.zones.iter().filter(|z| z.needs).map(|z| format!("F{:.0}", (z.execute.fz_mm * rpm * flutes).round())).collect();
    assert!(o.program_text.lines().any(|l| l.split_whitespace().any(|t| feeds.iter().any(|f| f == t))));
    let (ctx, _) = endmill_model::pipeline::calibrated_context(&p, None);
    let mut opts = CompOptions::default();
    opts.measured_radial_um = Some(40.0);
    let worn = wearcomp::evaluate(&p, &ctx, &r.report, &r.log, &opts, None);
    assert!(worn.gates.iter().any(|g| g.id == "vb_now" && g.severity == 3));
    assert!(worn.allowed.contains(&CompAction::ToolChange), "{:?}", worn.allowed);
    assert_eq!(worn.rule_action, CompAction::ToolChange);
}

#[test]
fn runout_is_averaged_over_a_revolution_in_down_and_up_milling() {
    let p = preset("티타늄");
    let (ctx, _) = endmill_model::pipeline::calibrated_context(&p, None);
    let rpm = p.conditions.spindle_rpm as f64;
    let mut true_tool = ctx.tool.clone();
    true_tool.runout_um = 0.0;
    for ae in [0.02, 0.1, 2.0] {
        let down = Engagement::side(3.2, ae, 10.0, true);
        let up = Engagement::side(3.2, ae, 10.0, false);
        let co = physics::cutting_coeffs_at(&ctx, 100.0, 0.01);
        let fd = physics::mechanistic_forces(&ctx.tool, &co, 0.05, rpm, &down, 96, 10);
        let fu = physics::mechanistic_forces(&ctx.tool, &co, 0.05, rpm, &up, 96, 10);
        let f0 = physics::mechanistic_forces(&true_tool, &co, 0.05, rpm, &down, 96, 10);
        assert!((fd.h_max_mm - fu.h_max_mm).abs() < 0.1 * fd.h_max_mm, "ae {} h {} vs {}", ae, fd.h_max_mm, fu.h_max_mm);
        assert!(fd.h_max_mm > f0.h_max_mm);
        assert!(fd.f_res_mean > 0.0 && (fd.f_res_mean - fu.f_res_mean).abs() < 0.15 * fd.f_res_mean.max(fu.f_res_mean), "ae {} F {} vs {}", ae, fd.f_res_mean, fu.f_res_mean);
        assert!((fd.f_res_mean - f0.f_res_mean).abs() < 0.35 * f0.f_res_mean, "ae {} F {} vs no runout {}", ae, fd.f_res_mean, f0.f_res_mean);
    }
}

#[test]
fn cancelling_the_simulation_stops_early() {
    let p = preset("탄소강");
    let (ctx, _) = endmill_model::pipeline::calibrated_context(&p, None);
    let r = endmill_model::viewport::build_report_prog(&p, "pocket_zigzag", &ctx, &mut |f| f < 0.3);
    assert!(r.is_err());
}

#[test]
fn utc_stamp_is_human_readable() {
    assert_eq!(wearcomp::utc_stamp(0), "19700101-000000Z");
    assert_eq!(wearcomp::utc_stamp(1_791_374_706_000), "20261007-120506Z");
    assert_eq!(wearcomp::utc_stamp(951_782_400_000), "20000229-000000Z");
}

#[test]
fn wear_log_resampling_conserves_volume_and_orders_time() {
    let mut bins = Vec::new();
    let mut t = 0.0;
    let mut vb = 0.0;
    for i in 0..500 {
        let dt = 0.05 + 0.01 * ((i % 7) as f64);
        t += dt;
        vb += 0.0001 * dt * (1.0 + (i as f64 / 500.0));
        bins.push(endmill_model::loadsim::WearBin {
            t_cut_s: t,
            t_s: t * 1.1,
            dt_s: dt,
            removed_mm3: 3.0 * dt,
            force_n: 100.0,
            force_peak_n: 150.0,
            temp_c: 300.0,
            temp_max_c: 320.0,
            power_kw: 1.0,
            h_um: 20.0,
            engage_deg: 60.0,
            chatter_min: 1.5,
            wall_um: 5.0,
            preheat_c: 0.0,
            vb_mm: vb,
            radial_loss_um: vb * 176.0,
            wp_c: 25.0,
            seg0: i,
            seg1: i,
        });
    }
    let log = wearlog::analyze(&bins, &[1.0, 2.0, 3.0], None, None, &[], 0.3);
    let total: f64 = bins.iter().map(|b| b.removed_mm3).sum();
    assert!((log.removed_mm3.last().unwrap() - total).abs() < 1e-6 * total.max(1.0));
    assert!(log.t_cut_s.windows(2).all(|w| w[1] > w[0]));
    assert!((log.vb_end_mm - vb).abs() < 1e-12);
    assert!(log.backtests.iter().all(|b| b.method == "holt"));
    let f = log.forecast.unwrap();
    assert!(f.quantiles[2].iter().zip(f.quantiles[0].iter()).all(|(hi, lo)| hi >= lo));
}