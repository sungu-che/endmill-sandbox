use endmill_model::*;

fn main() {
    println!("══════════════════════════════════════════");
    println!("  앤드밀(End Mill) 데이터 모델 데모");
    println!("══════════════════════════════════════════\n");

    println!("■ 주요 브랜드 & 가격 등급");
    println!("{:-<60}", "");
    for brand in known_brands() {
        println!(
            "  {:<22} {:<12} {:?} | 전문: {}",
            brand.name,
            format!("{:?}", brand.country),
            brand.primary_tier,
            brand.specialties.join(", ")
        );
    }

    println!("\n■ 절삭 조건 계산기 (Speeds & Feeds)");
    println!("{:-<60}", "");

    let workpiece = WorkpieceMaterial::Titanium;
    let conditions = CuttingCalculator::recommend_conditions(
        &workpiece,
        10.0,
        4,
        true,
    )
    .unwrap();

    println!("  피삭재: {:?}", workpiece);
    println!("  공구: Ø10mm 4날 (하이엔드)");
    println!("  ─────────────────────────────");
    println!("  절삭속도: {:.1} m/min", conditions.cutting_speed_m_min);
    println!("  스핀들:   {} RPM", conditions.spindle_rpm);
    println!("  이송속도: {:.0} mm/min", conditions.feed_rate_mm_min);
    println!("  칩/날:    {:.3} mm/tooth", conditions.feed_per_tooth_mm);
    println!("  절입깊이: 축 {:.1} / 반경 {:.1} mm", conditions.axial_doc_mm, conditions.radial_doc_mm);
    println!("  냉각:     {:?}", conditions.coolant);

    let derated = CuttingCalculator::apply_low_end_derating(&conditions, 0.4);
    println!("\n  [저가 공구 흉내내기 (40% 감속)]");
    println!("  스핀들:   {} RPM (원본 {})", derated.spindle_rpm, conditions.spindle_rpm);
    println!("  이송속도: {:.0} mm/min (원본 {:.0})", derated.feed_rate_mm_min, conditions.feed_rate_mm_min);

    println!("\n■ 샘플 하이엔드 공구 사양");
    println!("{:-<60}", "");
    let spec = sample_high_end_endmill();
    println!("  모델: {}", spec.model);
    println!("  등급: {:?}", spec.tier);
    println!("  직경: {}mm, 날: {:?}", spec.diameter_mm, spec.flute_geometry);
    println!("  코팅: {:?}", spec.coating.as_ref().map(|c| c.name.as_str()));
    println!("  난삭재 가공: {}", spec.can_machine_hard_materials());
    println!("  무인 가공 적합: {}", spec.suitable_for_unmanned_op());
    println!("  저가 대비 수명: {}배", spec.estimated_life_multiplier_vs_low_end());

    println!("\n■ 용도별 권장 등급 매칭");
    println!("{:-<60}", "");
    for profile in build_application_table() {
        println!(
            "  {:<12} | {:<14} | {:?} | {:?} | {}",
            format!("{:?}", profile.industry),
            format!("{:?}", profile.purpose),
            profile.recommended_tier,
            profile.workpiece,
            profile.note
        );
    }

    println!("\n■ JSON 직렬화 (일부)");
    println!("{:-<60}", "");
    let json = serde_json::to_string_pretty(&spec.coating).unwrap();
    let preview: String = json.chars().take(500).collect();
    println!("  {}...", preview);

    println!("\n══════════════════════════════════════════");
}