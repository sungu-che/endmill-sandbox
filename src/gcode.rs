use serde::{Deserialize, Serialize};
use crate::profile::MachiningProfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCodeLine {
    pub line_number: u32,
    pub code: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCodeProgram {
    pub program_name: String,
    pub header: Vec<GCodeLine>,
    pub body: Vec<GCodeLine>,
    pub footer: Vec<GCodeLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolPathPattern {
    /// 사각형 외곽 프로파일
    RectangularProfile,
    /// 포켓 가공 (내부 지그재그)
    PocketZigZag,
    /// 원형 프로파일 (보간)
    CircularProfile { center_x: f64, center_y: f64, radius: f64 },
    /// 헬리컬 진입 + 원형 포켓
    HelicalPocket { center_x: f64, center_y: f64, radius: f64, depth_per_rev: f64 },
    /// 등고선 (Contour) 다중 패스
    ContourMultiPass { offset_count: u8, step_over_mm: f64 },
    /// 슬롯 (직선 홈)
    Slot { start_x: f64, start_y: f64, end_x: f64, end_y: f64, width: f64 },
}

impl GCodeProgram {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("%\n");
        out.push_str(&format!("O{} ({})\n", self.program_name, self.program_name));
        for line in &self.header {
            out.push_str(&Self::format_line(line));
        }
        for line in &self.body {
            out.push_str(&Self::format_line(line));
        }
        for line in &self.footer {
            out.push_str(&Self::format_line(line));
        }
        out.push_str("%\n");
        out
    }

    fn format_line(line: &GCodeLine) -> String {
        let base = format!("N{:04} {}", line.line_number, line.code);
        match &line.comment {
            Some(c) => format!("{} ({})\n", base, c),
            None => format!("{}\n", base),
        }
    }

    pub fn line_count(&self) -> usize {
        self.header.len() + self.body.len() + self.footer.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolPathSegment {
    Linear { x: f64, y: f64, z: f64, feed: f64 },
    ArcCW { x: f64, y: f64, z: f64, i: f64, j: f64, feed: f64 },
    ArcCCW { x: f64, y: f64, z: f64, i: f64, j: f64, feed: f64 },
    Rapid { x: f64, y: f64, z: f64 },
    Plunge { x: f64, y: f64, z_start: f64, z_end: f64, feed: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GCodeMetadata {
    pub generated_at: String,
    pub profile_name: String,
    pub workpiece_material: String,
    pub tool_model: String,
    pub coolant_method: String,
    pub estimated_time_min: f64,
    pub total_distance_mm: f64,
    pub warnings: Vec<String>,
}

pub struct GCodeGenerator;

impl GCodeGenerator {
    pub fn generate_synthetic_segments_with_pattern(
        profile: &MachiningProfile,
        pattern: &ToolPathPattern,
    ) -> Vec<ToolPathSegment> {
        let feed = profile.conditions.feed_rate_mm_min;
        let cut_z = -profile.conditions.axial_doc_mm;
        let safe_z = cut_z + profile.endmill_setting.loc_mm + 10.0;
        let w = profile.workpiece_setup.width_mm;
        let h = profile.workpiece_setup.height_mm;
        let dia = profile.endmill_setting.diameter_mm;
        let plunge_feed = feed * 0.4;

        let mut segments = Vec::new();

        segments.push(ToolPathSegment::Rapid { x: 0.0, y: 0.0, z: safe_z });

        match pattern {
            ToolPathPattern::RectangularProfile => {
                segments.push(ToolPathSegment::Rapid { x: -dia / 2.0, y: -dia / 2.0, z: safe_z });
                segments.push(ToolPathSegment::Plunge {
                    x: -dia / 2.0, y: -dia / 2.0,
                    z_start: safe_z, z_end: cut_z, feed: plunge_feed,
                });
                segments.push(ToolPathSegment::Linear { x: w + dia / 2.0, y: -dia / 2.0, z: cut_z, feed });
                segments.push(ToolPathSegment::Linear { x: w + dia / 2.0, y: h + dia / 2.0, z: cut_z, feed });
                segments.push(ToolPathSegment::Linear { x: -dia / 2.0, y: h + dia / 2.0, z: cut_z, feed });
                segments.push(ToolPathSegment::Linear { x: -dia / 2.0, y: -dia / 2.0, z: cut_z, feed });
            }

            ToolPathPattern::PocketZigZag => {
                let step = dia * 0.6;
                let margin = dia / 2.0 + 1.0;
                let inner_w = w - margin * 2.0;
                let inner_h = h - margin * 2.0;

                segments.push(ToolPathSegment::Rapid { x: margin, y: margin, z: safe_z });
                segments.push(ToolPathSegment::Plunge {
                    x: margin, y: margin,
                    z_start: safe_z, z_end: cut_z, feed: plunge_feed,
                });

                let mut y_pos = margin;
                let mut direction = 1.0_f64;
                while y_pos < margin + inner_h {
                    let x_end = if direction > 0.0 { margin + inner_w } else { margin };
                    segments.push(ToolPathSegment::Linear { x: x_end, y: y_pos, z: cut_z, feed });
                    y_pos += step;
                    if y_pos < margin + inner_h {
                        segments.push(ToolPathSegment::Linear { x: x_end, y: y_pos, z: cut_z, feed: feed * 0.7 });
                    }
                    direction *= -1.0;
                }
            }

            ToolPathPattern::CircularProfile { center_x, center_y, radius } => {
                let start_x = center_x + radius;
                let start_y = *center_y;
                segments.push(ToolPathSegment::Rapid { x: start_x, y: start_y, z: safe_z });
                segments.push(ToolPathSegment::Plunge {
                    x: start_x, y: start_y,
                    z_start: safe_z, z_end: cut_z, feed: plunge_feed,
                });
                let steps = 36;
                for i in 1..=steps {
                    let angle = (i as f64 / steps as f64) * 2.0 * std::f64::consts::PI;
                    let px = center_x + radius * angle.cos();
                    let py = center_y + radius * angle.sin();
                    segments.push(ToolPathSegment::ArcCW {
                        x: px, y: py, z: cut_z,
                        i: -radius * ((i as f64 - 1.0) / steps as f64 * 2.0 * std::f64::consts::PI).cos(),
                        j: -radius * ((i as f64 - 1.0) / steps as f64 * 2.0 * std::f64::consts::PI).sin(),
                        feed,
                    });
                }
            }

            ToolPathPattern::HelicalPocket { center_x, center_y, radius, depth_per_rev } => {
                let total_depth = profile.conditions.axial_doc_mm;
                let revolutions = (total_depth / depth_per_rev).ceil() as usize;
                let start_x = center_x + radius;
                let start_y = *center_y;

                segments.push(ToolPathSegment::Rapid { x: start_x, y: start_y, z: safe_z });

                for rev in 0..revolutions {
                    let steps_per_rev = 24;
                    for i in 0..=steps_per_rev {
                        let angle = (i as f64 / steps_per_rev as f64) * 2.0 * std::f64::consts::PI;
                        let px = center_x + radius * angle.cos();
                        let py = center_y + radius * angle.sin();
                        let pz = -(rev as f64 * depth_per_rev) - (i as f64 / steps_per_rev as f64) * depth_per_rev;
                        let pz = pz.max(cut_z);
                        if rev == 0 && i == 0 {
                            segments.push(ToolPathSegment::Plunge {
                                x: px, y: py, z_start: safe_z, z_end: pz, feed: plunge_feed,
                            });
                        } else {
                            segments.push(ToolPathSegment::Linear { x: px, y: py, z: pz, feed });
                        }
                    }
                }

                let finish_steps = 36;
                for i in 1..=finish_steps {
                    let angle = (i as f64 / finish_steps as f64) * 2.0 * std::f64::consts::PI;
                    let px = center_x + radius * angle.cos();
                    let py = center_y + radius * angle.sin();
                    segments.push(ToolPathSegment::Linear { x: px, y: py, z: cut_z, feed });
                }
            }

            ToolPathPattern::ContourMultiPass { offset_count, step_over_mm } => {
                for pass in 0..*offset_count {
                    let offset = (pass as f64 + 1.0) * step_over_mm;
                    let pw = w - offset * 2.0;
                    let ph = h - offset * 2.0;
                    if pw <= 0.0 || ph <= 0.0 { break; }

                    let sx = offset;
                    let sy = offset;

                    if pass == 0 {
                        segments.push(ToolPathSegment::Rapid { x: sx, y: sy, z: safe_z });
                        segments.push(ToolPathSegment::Plunge {
                            x: sx, y: sy, z_start: safe_z, z_end: cut_z, feed: plunge_feed,
                        });
                    } else {
                        segments.push(ToolPathSegment::Rapid { x: sx, y: sy, z: safe_z });
                        segments.push(ToolPathSegment::Plunge {
                            x: sx, y: sy, z_start: safe_z, z_end: cut_z, feed: plunge_feed,
                        });
                    }

                    segments.push(ToolPathSegment::Linear { x: sx + pw, y: sy, z: cut_z, feed });
                    segments.push(ToolPathSegment::Linear { x: sx + pw, y: sy + ph, z: cut_z, feed });
                    segments.push(ToolPathSegment::Linear { x: sx, y: sy + ph, z: cut_z, feed });
                    segments.push(ToolPathSegment::Linear { x: sx, y: sy, z: cut_z, feed });
                }
            }

            ToolPathPattern::Slot { start_x, start_y, end_x, end_y, width } => {
                let half_w = width / 2.0;
                let dx = end_x - start_x;
                let dy = end_y - start_y;
                let len = (dx * dx + dy * dy).sqrt();
                if len < 0.001 {
                    segments.push(ToolPathSegment::Rapid { x: 0.0, y: 0.0, z: safe_z });
                    return segments;
                }
                let nx = -dy / len * half_w;
                let ny = dx / len * half_w;

                segments.push(ToolPathSegment::Rapid { x: start_x + nx, y: start_y + ny, z: safe_z });
                segments.push(ToolPathSegment::Plunge {
                    x: start_x + nx, y: start_y + ny,
                    z_start: safe_z, z_end: cut_z, feed: plunge_feed,
                });
                segments.push(ToolPathSegment::Linear { x: end_x + nx, y: end_y + ny, z: cut_z, feed });
                segments.push(ToolPathSegment::Linear { x: end_x - nx, y: end_y - ny, z: cut_z, feed });
                segments.push(ToolPathSegment::Linear { x: start_x - nx, y: start_y - ny, z: cut_z, feed });
                segments.push(ToolPathSegment::Linear { x: start_x + nx, y: start_y + ny, z: cut_z, feed });
            }
        }

        segments.push(ToolPathSegment::Rapid { x: 0.0, y: 0.0, z: safe_z });
        segments
    }

    pub fn generate_gcode_with_pattern(
        profile: &MachiningProfile,
        pattern: &ToolPathPattern,
    ) -> GCodeProgram {
        let segments = Self::generate_synthetic_segments_with_pattern(profile, pattern);
        let mut line_num: u32 = 10;
        let mut header = Vec::new();
        let mut body = Vec::new();
        let mut footer = Vec::new();

        header.push(GCodeLine {
            line_number: line_num,
            code: "G90 G94 G17 G21".into(),
            comment: Some("절대좌표, mm, XY평면".into()),
        });
        line_num += 10;

        header.push(GCodeLine {
            line_number: line_num,
            code: "G54".into(),
            comment: Some("워크 좌표계".into()),
        });
        line_num += 10;

        header.push(GCodeLine {
            line_number: line_num,
            code: format!("T1 M06"),
            comment: Some(format!("공구: {} (Ø{}mm, {}날)",
                profile.endmill_setting.model,
                profile.endmill_setting.diameter_mm,
                profile.endmill_setting.flute_count)),
        });
        line_num += 10;

        header.push(GCodeLine {
            line_number: line_num,
            code: format!("S{} M03", profile.conditions.spindle_rpm),
            comment: Some(format!("스핀들 {} RPM", profile.conditions.spindle_rpm)),
        });
        line_num += 10;

        header.push(GCodeLine {
            line_number: line_num,
            code: "G43 H1".into(),
            comment: Some("공구 길이 보정".into()),
        });
        line_num += 10;

        let coolant_code = profile.coolant_config.method.gcode_m_code();
        header.push(GCodeLine {
            line_number: line_num,
            code: coolant_code.to_string(),
            comment: Some(format!("냉각: {} ({}bar, {}L/min)",
                profile.coolant_config.method.label(),
                profile.coolant_config.pressure_bar,
                profile.coolant_config.flow_rate_l_min)),
        });
        line_num += 10;

        for seg in &segments {
            match seg {
                ToolPathSegment::Rapid { x, y, z } => {
                    body.push(GCodeLine {
                        line_number: line_num,
                        code: format!("G00 X{:.3} Y{:.3} Z{:.3}", x, y, z),
                        comment: None,
                    });
                }
                ToolPathSegment::Linear { x, y, z, feed } => {
                    body.push(GCodeLine {
                        line_number: line_num,
                        code: format!("G01 X{:.3} Y{:.3} Z{:.3} F{:.0}", x, y, z, feed),
                        comment: None,
                    });
                }
                ToolPathSegment::Plunge { x, y, z_start: _, z_end, feed } => {
                    body.push(GCodeLine {
                        line_number: line_num,
                        code: format!("G01 X{:.3} Y{:.3} Z{:.3} F{:.0}", x, y, z_end, feed),
                        comment: Some("절입".into()),
                    });
                }
                ToolPathSegment::ArcCW { x, y, z, i, j, feed } => {
                    body.push(GCodeLine {
                        line_number: line_num,
                        code: format!("G02 X{:.3} Y{:.3} Z{:.3} I{:.3} J{:.3} F{:.0}", x, y, z, i, j, feed),
                        comment: None,
                    });
                }
                ToolPathSegment::ArcCCW { x, y, z, i, j, feed } => {
                    body.push(GCodeLine {
                        line_number: line_num,
                        code: format!("G03 X{:.3} Y{:.3} Z{:.3} I{:.3} J{:.3} F{:.0}", x, y, z, i, j, feed),
                        comment: None,
                    });
                }
            }
            line_num += 10;
        }

        footer.push(GCodeLine { line_number: line_num, code: "M09".into(), comment: Some("냉각 정지".into()) });
        line_num += 10;
        footer.push(GCodeLine { line_number: line_num, code: "M05".into(), comment: Some("스핀들 정지".into()) });
        line_num += 10;
        footer.push(GCodeLine { line_number: line_num, code: "G91 G28 Z0.".into(), comment: Some("Z 원점".into()) });
        line_num += 10;
        footer.push(GCodeLine { line_number: line_num, code: "G91 G28 X0. Y0.".into(), comment: Some("XY 원점".into()) });
        line_num += 10;
        footer.push(GCodeLine { line_number: line_num, code: "M30".into(), comment: Some("종료".into()) });

        GCodeProgram {
            program_name: format!("{}_{}", profile.name, format!("{:?}", pattern).split('{').next().unwrap_or("").trim()),
            header,
            body,
            footer,
        }
    }

    pub fn generate_metadata_with_pattern(
        profile: &MachiningProfile,
        pattern: &ToolPathPattern,
    ) -> GCodeMetadata {
        let segments = Self::generate_synthetic_segments_with_pattern(profile, pattern);
        let mut total_dist = 0.0;
        let mut cutting_dist = 0.0;
        let mut prev: Option<(f64, f64, f64)> = None;
        let mut warnings = Vec::new();

        let safe_z_threshold = 0.0; // 소재 상단을 Z=0.0으로 가정
        let loc_limit = profile.endmill_setting.loc_mm;

        for seg in &segments {
            let (x, y, z) = match seg {
                ToolPathSegment::Linear { x, y, z, .. } => (*x, *y, *z),
                ToolPathSegment::Rapid { x, y, z } => (*x, *y, *z),
                ToolPathSegment::Plunge { x, y, z_end, .. } => (*x, *y, *z_end),
                ToolPathSegment::ArcCW { x, y, z, .. } => (*x, *y, *z),
                ToolPathSegment::ArcCCW { x, y, z, .. } => (*x, *y, *z),
            };
            if let Some((px, py, pz)) = prev {
                let d = ((x - px).powi(2) + (y - py).powi(2) + (z - pz).powi(2)).sqrt();
                total_dist += d;
                if !matches!(seg, ToolPathSegment::Rapid { .. }) {
                    cutting_dist += d;
                }
            }
            prev = Some((x, y, z));

            // 안전성 충돌 검사 로직
            match seg {
                ToolPathSegment::Rapid { z, .. } => {
                    if *z < safe_z_threshold {
                        let msg = format!("충돌 위험: 안전 높이 미만에서 G00 급이속 이동 감지 (Z={:.3})", z);
                        if !warnings.contains(&msg) {
                            warnings.push(msg);
                        }
                    }
                }
                ToolPathSegment::Plunge { z_end, .. } => {
                    if z_end.abs() > loc_limit {
                        let msg = format!("샹크 간섭: 플런지 깊이가 공구 유효 길이 초과 (Z={:.3}, LOC={:.1})", z_end, loc_limit);
                        if !warnings.contains(&msg) {
                            warnings.push(msg);
                        }
                    }
                }
                ToolPathSegment::Linear { z, .. } | ToolPathSegment::ArcCW { z, .. } | ToolPathSegment::ArcCCW { z, .. } => {
                    if z.abs() > loc_limit {
                        let msg = format!("샹크 간섭: 가공 깊이가 공구 유효 길이 초과 (Z={:.3}, LOC={:.1})", z, loc_limit);
                        if !warnings.contains(&msg) {
                            warnings.push(msg);
                        }
                    }
                }
            }
        }

        let est_time = if profile.conditions.feed_rate_mm_min > 0.0 {
            cutting_dist / profile.conditions.feed_rate_mm_min
        } else {
            0.0
        };

        GCodeMetadata {
            generated_at: format!("{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()),
            profile_name: profile.name.clone(),
            workpiece_material: profile.workpiece_setup.material_label(),
            tool_model: profile.endmill_setting.model.clone(),
            coolant_method: profile.coolant_config.method.label(),
            estimated_time_min: est_time,
            total_distance_mm: total_dist,
            warnings,
        }
    }

    pub fn generate_synthetic_segments(profile: &MachiningProfile) -> Vec<ToolPathSegment> {
        Self::generate_synthetic_segments_with_pattern(profile, &ToolPathPattern::PocketZigZag)
    }

    pub fn generate_from_profile(profile: &MachiningProfile) -> GCodeProgram {
        Self::generate_gcode_with_pattern(profile, &ToolPathPattern::PocketZigZag)
    }

    pub fn generate_metadata(profile: &MachiningProfile) -> GCodeMetadata {
        Self::generate_metadata_with_pattern(profile, &ToolPathPattern::PocketZigZag)
    }
}