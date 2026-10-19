//! 문헌 벤치마크 칸 하나의 물리 내역 — 어긋난 칸의 원인(힘·온도·마모 기구·궤적)을 찾을 때 쓴다.
//!
//!   cargo run --example bench_probe -- <공구 key> <조건 key> [Vc 배수 ...]
//!   cargo run --example bench_probe -- list
//!
//! 공구·조건 key 는 `src/benchmark.rs` (`paper_tools` · `sandbox_tools` · `paper_cuts`). Vc 배수를 여러 개 주면 같은 칸을 속도만 바꿔 나란히 찍는다.
use endmill_model::benchmark::{self, Param};
use endmill_model::physics::{self, CutContext, ThermalBody};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut tools = benchmark::paper_tools();
    tools.extend(benchmark::sandbox_tools());
    let cuts = benchmark::paper_cuts();
    if args.is_empty() || args[0] == "list" {
        println!("공구: {}", tools.iter().map(|t| t.key).collect::<Vec<_>>().join(" "));
        println!("조건: {}", cuts.iter().map(|c| c.key).collect::<Vec<_>>().join(" "));
        return;
    }
    let tool = tools.iter().find(|t| t.key == args[0]).expect("공구 key");
    let cut = cuts.iter().find(|c| c.key == args.get(1).map(|s| s.as_str()).unwrap_or(tool.key)).expect("조건 key");
    let scales: Vec<f64> = if args.len() > 2 { args[2..].iter().filter_map(|s| s.parse().ok()).collect() } else { vec![1.0] };
    println!("공구 {} — {}\n  {}", tool.key, tool.label, tool.source);
    println!("조건 {} — {}\n  {}", cut.key, cut.label, cut.source);
    for k in scales {
        let p = benchmark::cell_profile(tool, cut, &[(Param::Vc, k)]);
        let mut ctx = CutContext::from_profile(&p);
        ctx.wp.vb_criterion_mm = cut.vb_criterion_mm;
        let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
        let a = physics::analyze_cut(&ctx, &p.conditions, cut.down, body);
        let pr = benchmark::predict(tool, cut, &[(Param::Vc, k)]);
        let t = &ctx.tool;
        println!("\n== Vc ×{:.2}: Vc {:.1} (실효 {:.1}) · n {:.0} · fz {:.4} · ap {:.2} · ae {:.2} · {:?} · 냉각 {:?}", k, a.vc_m_min, a.vc_effective_m_min, a.rpm, a.fz_mm, a.ap_mm, a.ae_mm, a.mode, ctx.coolant.method);
        println!(
            "  공구: Ø{:.2} Z{} 헬릭스 {:.0}° 경사 {:.1}° 여유 {:.1}° 날끝 {:.1} µm 코팅 {} ({}) 모재 {} 런아웃 {:.1} µm · 적합 마모 ×{:.2} · 기하 ×{:.2}",
            t.diameter_mm,
            t.flutes,
            t.helix_deg,
            t.rake_deg,
            t.clearance_deg,
            t.edge_radius_um,
            t.coating.name,
            t.coating.family,
            t.substrate,
            t.runout_um,
            t.fit_wear,
            physics::flank_geometry_factor(&ctx)
        );
        let co = &a.coeffs;
        println!(
            "  계수: μ {:.3} (β {:.1}°) φ {:.1}° kc1.1 {:.0} (마찰 배수 {:.3} · 연화 {:.3}) mc {:.2} kr {:.3} kte {:.1} kre {:.1} · h_min {:.1} µm · 변형률 속도 {:.2e} (×{:.3}) · 접근 {:.2} · 점착 {:.2}",
            co.mu, co.beta_deg, co.shear_angle_deg, co.kc11, co.friction_factor, co.softening, co.mc, co.kr, co.kte, co.kre, co.h_min_mm * 1000.0, co.strain_rate, co.rate_factor, co.lubricant_access, co.sticking
        );
        let f = &a.forces;
        println!(
            "  힘: 평균 합력 {:.1} N · 피크 {:.1} N · 평균 벡터 {:.1} N · 축 {:.1} N · 토크 {:.3} N·m · 동력 {:.3} kW · h 평균 {:.2} µm · h 최대 {:.2} µm · 물린 날 {:.2} · 밀림 {:.0}% · 비에너지 {:.2} J/mm³ · MRR {:.2} cm³/min",
            f.f_res_mean,
            f.f_res_peak,
            pr.f_vec_mean_n,
            f.fa_mean,
            f.torque_mean_nm,
            f.cutting_power_kw,
            f.h_mean_mm * 1000.0,
            f.h_max_mm * 1000.0,
            f.teeth_in_cut,
            a.ploughing_share * 100.0,
            pr.specific_energy_j_mm3,
            a.mrr_cm3_min
        );
        let th = &a.thermal;
        let (shear_rise, beta) = physics::shear_zone(&ctx, co, a.vc_effective_m_min, f.h_mean_mm);
        println!(
            "  열: 계면 건식 {:.0} °C · 냉각 {:.0} °C (저감 {:.0} %, 효과 {:.2}, {}) · 전단면 +{:.0} K (공작물 몫 {:.2}) · 접촉 {:.3} mm · 과도 {:.2} · 열균열 {:.2} · BUE {:.2}",
            th.interface_dry_c,
            th.interface_c,
            th.coolant_reduction_pct,
            th.cooling.effectiveness,
            th.cooling.boiling_state,
            shear_rise,
            beta,
            th.cooling.contact_length_mm,
            th.transient_factor,
            th.thermal_crack_risk,
            th.bue_risk
        );
        println!(
            "        물림 시간 {:.3} ms · 칩 쪽 전도 {:.3e} W/m²K · 공구 k {:.0} W/mK · 공구 확산 {:.2e} m²/s · 물림각 {:.1}°",
            th.engagement_time_ms,
            th.cooling.conduction,
            ctx.tool.tool_conductivity,
            ctx.tool.tool_diffusivity(),
            a.engage_angle_deg
        );
        {
            let d = ctx.tool.diameter_mm;
            let eng = physics::Engagement::side(a.ap_mm, a.ae_mm, d, cut.down);
            let eref = physics::Engagement::side((ctx.wp.ap_ratio * d).min(ctx.tool.loc_mm), ctx.wp.ae_ratio * d, d, true);
            let lc = th.cooling.contact_length_mm;
            let gc = th.cooling.conduction;
            let period = 60.0 / a.rpm;
            let kap = ctx.tool.tool_diffusivity();
            let hc = endmill_model::tribology::interrupted_heating(lc, physics::engaged_edge_length_mm(&ctx.tool, &eng), gc, ctx.tool.tool_conductivity, kap, eng.span() / std::f64::consts::TAU * period, period);
            let hr = endmill_model::tribology::interrupted_heating(lc, physics::engaged_edge_length_mm(&ctx.tool, &eref), gc, ctx.tool.tool_conductivity, kap, eref.span() / std::f64::consts::TAU * period, period);
            println!(
                "        단속 열: 날 접촉 {:.2} mm (기준 {:.2}) · 물림 {:.3} ms (기준 {:.3}) · 열저항 {:.3e} / {:.3e} = {:.3} · 배수 {:.3}",
                physics::engaged_edge_length_mm(&ctx.tool, &eng),
                physics::engaged_edge_length_mm(&ctx.tool, &eref),
                eng.span() / std::f64::consts::TAU * period * 1000.0,
                eref.span() / std::f64::consts::TAU * period * 1000.0,
                hc,
                hr,
                hc / hr,
                physics::interrupted_factor(&ctx, &eng, a.rpm, lc, gc)
            );
        }
        let r = physics::reference_state(&ctx);
        println!(
            "  기준 상태 ({}): 계면 {:.0} °C · 반경 하중 {:.1} N/mm · Vc {:.0} · 물림 {:.3} · 기준 수명 {:.0} min (VB {:.2})",
            ctx.wp.label,
            r.t_c,
            r.stress,
            r.vc,
            r.duty,
            ctx.wp.ref_life_min,
            ctx.wp.vb_criterion_mm
        );
        let w = &a.wear;
        let mech: Vec<String> = w.mechanisms.iter().map(|m| format!("{} {:.0}% (×{:.2})", m.key, m.share * 100.0, m.ratio)).collect();
        println!(
            "  마모: 응력 ×{:.2} · 미끄럼 ×{:.2} · 온도 ×{:.2} · 런아웃 ×{:.2} · 배출 ×{:.2} · 기구 합 ×{:.2} · 코팅 노출 {:.2} · 채터 ×{:.2} · 지배 {}",
            w.stress_factor, w.speed_factor, w.temperature_factor, w.runout_factor, w.evacuation_factor, w.mechanism_factor, w.coating_exposure, w.chatter_factor, w.dominant
        );
        println!("        {}", mech.join(" · "));
        let tr = &a.trajectory;
        println!(
            "  궤적: κ {:.3} · 길들이기 {:.1} µm / τ {:.2} min · 선형 수명 {:.2} · 수명 {:.2} min (판정 VB {:.2}) · 3차 진입 {} · 가속 ×{:.2}",
            tr.kappa,
            tr.break_in_vb_mm * 1000.0,
            tr.break_in_tau_min,
            tr.linear_life_min,
            tr.life_min,
            tr.vb_limit_mm,
            tr.phase3_min.map(|v| format!("{:.2} min", v)).unwrap_or_else(|| "-".into()),
            tr.acceleration
        );
        let pts: Vec<String> = [0.25, 0.5, 0.75, 1.0].iter().map(|q| format!("{:.0}% {:.3} mm", q * 100.0, tr.vb_at(q * tr.life_min))).collect();
        println!("        VB(t): {}", pts.join(" · "));
        let growth: Vec<String> = [0.1, 0.2, 0.3].iter().map(|vb| format!("VB {:.1} ×{:.2}", vb, benchmark::force_growth(tr, *vb))).collect();
        println!("        합력 증가: {}", growth.join(" · "));
        println!(
            "  예측: 수명 {:.3} min (채터 제외 {:.3}) · 수명 체적 {:.3} cm³ · Ra {:.3} µm · 채터 여유 {:.2} · 궁합 {} · 적합도 {}{}",
            pr.life_min,
            pr.life_no_chatter_min,
            pr.life_volume_cm3,
            pr.ra_um,
            pr.chatter_margin,
            pr.compat_severity,
            pr.fit_verdict,
            if cut.ortho_h_mm.is_some() { format!(" · 직교 Fc′ {:.1} · Ff′ {:.1} N/mm", pr.ortho_fc_n_mm, pr.ortho_ff_n_mm) } else { String::new() }
        );
    }
}
