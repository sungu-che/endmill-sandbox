use super::now_ms;
use super::rdb::Rdb;
use crate::loadsim::{CutRegime, LoadSimReport};
use crate::physics::{CutContext, ToolGeometry};
use crate::profile::{EndMillMockupSetting, MachiningProfile};
use crate::toolwear::{self, AxialProfile, RefCut, RefSummary, ToolStart, WearField};
use crate::wearcomp::{CompAction, CompOutcome};
use crate::wearlog::WearLog;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

fn err(e: rusqlite::Error) -> String {
    format!("SQLite: {}", e)
}

fn fin(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

fn fin_opt(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

pub const HISTORY_CAP: usize = 600;
pub const HISTORY_PER_PASS: usize = 16;
pub const REF_MIN_SIMILARITY: f64 = 0.02;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolInstanceRow {
    pub id: i64,
    pub slot_key: String,
    pub geom_key: String,
    pub endmill_id: Option<i64>,
    pub label: String,
    pub generation: i64,
    pub vb_band_mm: f64,
    pub vb_max_mm: f64,
    pub cut_min: f64,
    pub removed_cm3: f64,
    pub jobs: i64,
    pub field: WearField,
    pub history: Vec<(f64, f64)>,
    pub anchor_um: Option<f64>,
    pub pred_base_um: f64,
    pub last_measured_um: Option<f64>,
    pub last_ap_mm: f64,
    pub thermal_damage: f64,
    pub life_used_max: f64,
    pub pending_job_id: Option<i64>,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub retired_at: Option<i64>,
    pub retire_reason: String,
}

impl ToolInstanceRow {
    pub fn field_for(&self, tool: &ToolGeometry) -> WearField {
        let flutes = tool.flutes.max(1) as usize;
        if self.field.compatible(tool.loc_mm, flutes) {
            return self.field.clone();
        }
        self.field.resampled(tool.loc_mm, flutes).unwrap_or_else(|| WearField::for_tool(tool))
    }

    pub fn start(&self, tool: &ToolGeometry, ap_mm: f64) -> ToolStart {
        let flutes = tool.flutes.max(1) as usize;
        let mut field = self.field_for(tool);
        if field.is_empty() && self.vb_band_mm > 0.0 {
            field.deposit(ap_mm.max(self.last_ap_mm).min(tool.loc_mm), self.vb_band_mm, &AxialProfile::default(), &vec![1.0; flutes]);
        }
        ToolStart {
            instance_id: if self.id > 0 { Some(self.id) } else { None },
            generation: self.generation,
            label: self.label.clone(),
            vb_band_mm: field.band_vb(ap_mm),
            vb_max_mm: field.max_vb(),
            cut_min: self.cut_min,
            removed_cm3: self.removed_cm3,
            jobs: self.jobs,
            field,
            anchor_um: self.anchor_um,
            pred_base_since_anchor_um: self.pred_base_um,
            history: self.history.clone(),
            last_measured_um: self.last_measured_um,
            thermal_damage: self.thermal_damage,
            life_used_max: self.life_used_max,
        }
    }

    fn conform(&mut self, tool: &ToolGeometry) -> bool {
        if self.field.compatible(tool.loc_mm, tool.flutes.max(1) as usize) {
            return false;
        }
        self.field = self.field_for(tool);
        true
    }

    fn refresh(&mut self, tool: &ToolGeometry, ap_mm: f64, vb_limit_mm: f64) {
        if ap_mm > 0.0 {
            self.last_ap_mm = ap_mm;
        }
        let ap = if self.last_ap_mm > 0.0 { self.last_ap_mm } else { self.field.loc_mm };
        self.vb_band_mm = self.field.band_vb(ap);
        self.vb_max_mm = self.field.max_vb();
        let s = toolwear::summarize(&self.field, tool, ap, vb_limit_mm);
        self.life_used_max = fin(self.life_used_max).max(s.life_used).max(self.thermal_damage);
        if self.history.len() > HISTORY_CAP {
            let cut = self.history.len() - HISTORY_CAP;
            self.history.drain(0..cut);
        }
        self.updated_at = now_ms();
    }

    fn push_history(&mut self, t_min: f64, vb: f64) {
        if !t_min.is_finite() || !vb.is_finite() {
            return;
        }
        if let Some(last) = self.history.last() {
            if t_min <= last.0 + 1e-9 {
                if (t_min - last.0).abs() <= 1e-9 {
                    let n = self.history.len();
                    self.history[n - 1].1 = vb;
                }
                return;
            }
        }
        self.history.push((t_min, vb));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WearEventRow {
    pub id: i64,
    pub tool_instance_id: i64,
    pub job_id: Option<i64>,
    pub kind: String,
    pub cut_min_after: f64,
    pub vb_before_mm: f64,
    pub vb_after_mm: f64,
    pub radial_um: Option<f64>,
    pub source: String,
    pub note: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobPassRow {
    pub job_id: i64,
    pub pass: i64,
    pub status: String,
    pub action: String,
    pub cut_min: f64,
    pub removed_cm3: f64,
    pub vb_start_mm: f64,
    pub vb_end_mm: f64,
    pub radial_end_um: f64,
    pub residual_p50_um: Option<f64>,
    pub residual_p90_um: Option<f64>,
    pub max_temp_c: Option<f64>,
    pub max_force_n: Option<f64>,
    pub chatter_fraction: Option<f64>,
    pub chatter_wear: Option<f64>,
    pub committed: bool,
    #[serde(default)]
    pub field: Option<WearField>,
    #[serde(default)]
    pub metrics: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobRow {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub profile_name: String,
    pub pattern: String,
    pub stamp: String,
    pub parent_job_id: Option<i64>,
    pub root_job_id: Option<i64>,
    pub tool_instance_id: Option<i64>,
    pub endmill_id: Option<i64>,
    pub workpiece_id: Option<i64>,
    pub material_key: String,
    pub coating_family: String,
    pub coolant: String,
    pub final_action: String,
    pub decided_by: String,
    pub vb_start_mm: f64,
    pub vb_end_mm: f64,
    pub report_line: String,
    pub created_at: i64,
    #[serde(default)]
    pub passes: Vec<JobPassRow>,
    #[serde(default)]
    pub tool_label: String,
    #[serde(default)]
    pub tool_generation: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CutRecordRow {
    pub id: i64,
    pub job_id: i64,
    pub pass: i64,
    pub tool_instance_id: Option<i64>,
    pub endmill_id: Option<i64>,
    pub tool_label: String,
    pub material_key: String,
    pub coating_family: String,
    pub coolant: String,
    pub diameter_mm: f64,
    pub flutes: i64,
    pub helix_deg: f64,
    pub nose: String,
    pub mode: String,
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub phi_st_deg: f64,
    pub phi_ex_deg: f64,
    pub engage_deg: f64,
    pub vc_m_min: f64,
    pub fz_mm: f64,
    pub h_mean_um: f64,
    pub t_cut_s: f64,
    pub removed_mm3: f64,
    pub temp_c: f64,
    pub force_n: f64,
    pub chatter_min: f64,
    pub chatter_wear: f64,
    pub vb_start_mm: f64,
    pub dvb_mm: f64,
    pub vb_rate_um_min: f64,
    pub flute_shares: Vec<f64>,
    pub measured_ratio: Option<f64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobDetail {
    pub job: JobRow,
    pub summary: serde_json::Value,
    pub records: Vec<CutRecordRow>,
    pub events: Vec<WearEventRow>,
    pub tool: Option<ToolInstanceRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolIdentity {
    pub slot: String,
    pub geom: String,
    pub label: String,
}

const TOOL_COLS: &str = "id, slot_key, geom_key, endmill_id, label, generation, vb_band_mm, vb_max_mm, cut_min, removed_cm3, jobs, field_json, history_json, anchor_um, pred_base_um, last_measured_um, last_ap_mm, thermal_damage, life_used_max, pending_job_id, version, created_at, updated_at, retired_at, retire_reason";
const JOB_COLS: &str = "j.id, j.project_id, j.name, j.profile_name, j.pattern, j.stamp, j.parent_job_id, j.root_job_id, j.tool_instance_id, j.endmill_id, j.workpiece_id, j.material_key, j.coating_family, j.coolant, j.final_action, j.decided_by, j.vb_start_mm, j.vb_end_mm, j.report_line, j.created_at, COALESCE(t.label, ''), COALESCE(t.generation, 0)";
const PASS_COLS: &str = "job_id, pass, status, action, cut_min, removed_cm3, vb_start_mm, vb_end_mm, radial_end_um, residual_p50_um, residual_p90_um, max_temp_c, max_force_n, chatter_fraction, chatter_wear, committed, field_json, metrics_json";
const REC_COLS: &str = "id, job_id, pass, tool_instance_id, endmill_id, tool_label, material_key, coating_family, coolant, diameter_mm, flutes, helix_deg, nose, mode, ap_mm, ae_mm, phi_st_deg, phi_ex_deg, engage_deg, vc_m_min, fz_mm, h_mean_um, t_cut_s, removed_mm3, temp_c, force_n, chatter_min, chatter_wear, vb_start_mm, dvb_mm, vb_rate_um_min, flute_shares, measured_ratio, created_at";
const EVENT_COLS: &str = "id, tool_instance_id, job_id, kind, cut_min_after, vb_before_mm, vb_after_mm, radial_um, source, note, created_at";

fn tool_of(r: &Row) -> rusqlite::Result<ToolInstanceRow> {
    let field_json: String = r.get(11)?;
    let history_json: String = r.get(12)?;
    Ok(ToolInstanceRow {
        id: r.get(0)?,
        slot_key: r.get(1)?,
        geom_key: r.get(2)?,
        endmill_id: r.get(3)?,
        label: r.get(4)?,
        generation: r.get(5)?,
        vb_band_mm: r.get(6)?,
        vb_max_mm: r.get(7)?,
        cut_min: r.get(8)?,
        removed_cm3: r.get(9)?,
        jobs: r.get(10)?,
        field: serde_json::from_str(&field_json).unwrap_or_default(),
        history: serde_json::from_str(&history_json).unwrap_or_default(),
        anchor_um: r.get(13)?,
        pred_base_um: r.get(14)?,
        last_measured_um: r.get(15)?,
        last_ap_mm: r.get(16)?,
        thermal_damage: r.get(17)?,
        life_used_max: r.get(18)?,
        pending_job_id: r.get(19)?,
        version: r.get(20)?,
        created_at: r.get(21)?,
        updated_at: r.get(22)?,
        retired_at: r.get(23)?,
        retire_reason: r.get(24)?,
    })
}

fn job_of(r: &Row) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        profile_name: r.get(3)?,
        pattern: r.get(4)?,
        stamp: r.get(5)?,
        parent_job_id: r.get(6)?,
        root_job_id: r.get(7)?,
        tool_instance_id: r.get(8)?,
        endmill_id: r.get(9)?,
        workpiece_id: r.get(10)?,
        material_key: r.get(11)?,
        coating_family: r.get(12)?,
        coolant: r.get(13)?,
        final_action: r.get(14)?,
        decided_by: r.get(15)?,
        vb_start_mm: r.get(16)?,
        vb_end_mm: r.get(17)?,
        report_line: r.get(18)?,
        created_at: r.get(19)?,
        passes: Vec::new(),
        tool_label: r.get(20)?,
        tool_generation: r.get(21)?,
    })
}

fn pass_of(r: &Row) -> rusqlite::Result<JobPassRow> {
    let field_json: String = r.get(16)?;
    let metrics_json: String = r.get(17)?;
    Ok(JobPassRow {
        job_id: r.get(0)?,
        pass: r.get(1)?,
        status: r.get(2)?,
        action: r.get(3)?,
        cut_min: r.get(4)?,
        removed_cm3: r.get(5)?,
        vb_start_mm: r.get(6)?,
        vb_end_mm: r.get(7)?,
        radial_end_um: r.get(8)?,
        residual_p50_um: r.get(9)?,
        residual_p90_um: r.get(10)?,
        max_temp_c: r.get(11)?,
        max_force_n: r.get(12)?,
        chatter_fraction: r.get(13)?,
        chatter_wear: r.get(14)?,
        committed: r.get::<_, i64>(15)? != 0,
        field: if field_json.is_empty() { None } else { serde_json::from_str(&field_json).ok() },
        metrics: serde_json::from_str(&metrics_json).unwrap_or(serde_json::Value::Null),
    })
}

fn record_of(r: &Row) -> rusqlite::Result<CutRecordRow> {
    let shares: String = r.get(31)?;
    Ok(CutRecordRow {
        id: r.get(0)?,
        job_id: r.get(1)?,
        pass: r.get(2)?,
        tool_instance_id: r.get(3)?,
        endmill_id: r.get(4)?,
        tool_label: r.get(5)?,
        material_key: r.get(6)?,
        coating_family: r.get(7)?,
        coolant: r.get(8)?,
        diameter_mm: r.get(9)?,
        flutes: r.get(10)?,
        helix_deg: r.get(11)?,
        nose: r.get(12)?,
        mode: r.get(13)?,
        ap_mm: r.get(14)?,
        ae_mm: r.get(15)?,
        phi_st_deg: r.get(16)?,
        phi_ex_deg: r.get(17)?,
        engage_deg: r.get(18)?,
        vc_m_min: r.get(19)?,
        fz_mm: r.get(20)?,
        h_mean_um: r.get(21)?,
        t_cut_s: r.get(22)?,
        removed_mm3: r.get(23)?,
        temp_c: r.get(24)?,
        force_n: r.get(25)?,
        chatter_min: r.get(26)?,
        chatter_wear: r.get(27)?,
        vb_start_mm: r.get(28)?,
        dvb_mm: r.get(29)?,
        vb_rate_um_min: r.get(30)?,
        flute_shares: serde_json::from_str(&shares).unwrap_or_default(),
        measured_ratio: r.get(32)?,
        created_at: r.get(33)?,
    })
}

fn event_of(r: &Row) -> rusqlite::Result<WearEventRow> {
    Ok(WearEventRow {
        id: r.get(0)?,
        tool_instance_id: r.get(1)?,
        job_id: r.get(2)?,
        kind: r.get(3)?,
        cut_min_after: r.get(4)?,
        vb_before_mm: r.get(5)?,
        vb_after_mm: r.get(6)?,
        radial_um: r.get(7)?,
        source: r.get(8)?,
        note: r.get(9)?,
        created_at: r.get(10)?,
    })
}

fn json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn metric(m: &serde_json::Value, key: &str) -> f64 {
    m.get(key).and_then(|v| v.as_f64()).filter(|v| v.is_finite()).unwrap_or(0.0)
}

impl Rdb {
    pub fn active_tool(&self, slot_key: &str) -> Result<Option<ToolInstanceRow>, String> {
        self.conn()
            .query_row(
                &format!("SELECT {} FROM tool_instances WHERE slot_key = ?1 AND retired_at IS NULL", TOOL_COLS),
                params![slot_key],
                tool_of,
            )
            .optional()
            .map_err(err)
    }

    pub fn tool_by_id(&self, id: i64) -> Result<Option<ToolInstanceRow>, String> {
        self.conn()
            .query_row(&format!("SELECT {} FROM tool_instances WHERE id = ?1", TOOL_COLS), params![id], tool_of)
            .optional()
            .map_err(err)
    }

    pub fn endmill_id_of(&self, s: &EndMillMockupSetting) -> Result<Option<i64>, String> {
        let key = crate::store::attr::EndMillAttr::from_setting(s).key();
        self.conn()
            .query_row("SELECT id FROM endmills WHERE attr_key = ?1", params![key], |r| r.get(0))
            .optional()
            .map_err(err)
    }

    fn insert_tool(&self, id: &ToolIdentity, endmill_id: Option<i64>, tool: &ToolGeometry) -> Result<ToolInstanceRow, String> {
        let gen: i64 = self
            .conn()
            .query_row("SELECT COALESCE(MAX(generation), 0) FROM tool_instances WHERE slot_key = ?1", params![id.slot], |r| r.get(0))
            .map_err(err)?;
        let now = now_ms();
        let field = WearField::for_tool(tool);
        self.conn()
            .execute(
                "INSERT INTO tool_instances(slot_key, geom_key, endmill_id, label, generation, field_json, history_json, created_at, updated_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, '[]', ?7, ?7)",
                params![id.slot, id.geom, endmill_id, id.label, gen + 1, json(&field), now],
            )
            .map_err(err)?;
        let rid = self.conn().last_insert_rowid();
        self.tool_by_id(rid)?.ok_or_else(|| "공구 개체를 만들지 못했습니다".to_string())
    }

    fn retire(&self, t: &ToolInstanceRow, kind: &str, reason: &str, source: &str, job_id: Option<i64>) -> Result<(), String> {
        let now = now_ms();
        self.conn()
            .execute(
                "UPDATE tool_instances SET retired_at = ?2, retire_reason = ?3, updated_at = ?2, version = version + 1 WHERE id = ?1 AND retired_at IS NULL",
                params![t.id, now, reason],
            )
            .map_err(err)?;
        self.add_wear_event(&WearEventRow {
            tool_instance_id: t.id,
            job_id,
            kind: kind.into(),
            cut_min_after: t.cut_min,
            vb_before_mm: t.vb_band_mm,
            vb_after_mm: 0.0,
            source: source.into(),
            note: reason.to_string(),
            created_at: now,
            ..Default::default()
        })?;
        Ok(())
    }

    pub fn ensure_tool(&self, id: &ToolIdentity, endmill_id: Option<i64>, tool: &ToolGeometry) -> Result<(ToolInstanceRow, Option<String>), String> {
        if let Some(t) = self.active_tool(&id.slot)? {
            if id.geom.is_empty() || t.geom_key.is_empty() || t.geom_key == id.geom {
                return Ok((t, None));
            }
            let note = format!(
                "프리셋의 공구 사양 변경 ({} → {}) — 이 작업을 기록할 때 이전 개체 #{} 을 은퇴시키고 새 공구로 시작 (기록 전에 취소하거나 사양을 되돌리면 그대로 유지)",
                t.geom_key, id.geom, t.id
            );
            let next = ToolInstanceRow {
                slot_key: id.slot.clone(),
                geom_key: id.geom.clone(),
                endmill_id,
                label: id.label.clone(),
                generation: t.generation + 1,
                field: WearField::for_tool(tool),
                ..Default::default()
            };
            return Ok((next, Some(note)));
        }
        Ok((self.insert_tool(id, endmill_id, tool)?, None))
    }

    fn charge_pending(&self, n: &mut ToolInstanceRow, job: i64, tool: &ToolGeometry, vb_limit_mm: f64, lines: &mut Vec<String>) -> Result<(), String> {
        let Some(p2) = self.job_passes(job)?.into_iter().find(|p| p.pass == 2 && !p.committed && p.action == CompAction::ToolChange.key()) else {
            return Ok(());
        };
        let before = n.vb_band_mm;
        let added = match p2.field.as_ref() {
            Some(f) => n.field.add_any(f),
            None => false,
        };
        if !added {
            self.set_pass_status(job, 2, "replaced", false)?;
            lines.push(format!("작업 #{} 의 공구 교체 2차 계획은 새 공구의 날 수·마모 지도와 맞지 않아 기록하지 않음 — 새 공구로 다시 계획하세요", job));
            return Ok(());
        }
        n.cut_min += fin(p2.cut_min);
        n.removed_cm3 += fin(p2.removed_cm3);
        n.jobs += 1;
        n.pred_base_um += metric(&p2.metrics, "pred_base_um");
        n.thermal_damage += metric(&p2.metrics, "thermal_damage");
        let ap = metric(&p2.metrics, "ap_mm");
        n.refresh(tool, ap, vb_limit_mm);
        let (cut, band) = (n.cut_min, n.vb_band_mm);
        n.push_history(cut, band);
        self.add_wear_event(&WearEventRow {
            tool_instance_id: n.id,
            job_id: Some(job),
            kind: "pass2".into(),
            cut_min_after: n.cut_min,
            vb_before_mm: before,
            vb_after_mm: n.vb_band_mm,
            source: "sim".into(),
            note: format!("작업 #{} 의 공구 교체 후 2차 보정을 새 공구로 가공한 것으로 기록", job),
            ..Default::default()
        })?;
        self.set_pass_status(job, 2, "done_new_tool", true)?;
        n.version = self.save_tool(n)?;
        lines.push(format!("작업 #{} 의 2차(공구 교체 후 보정) {:.2}분을 새 공구 #{} 에 기록 — VB {:.3} mm", job, p2.cut_min, n.id, n.vb_band_mm));
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn replace_active(&self, id: &ToolIdentity, endmill_id: Option<i64>, tool: &ToolGeometry, vb_limit_mm: f64, kind: &str, reason: &str, source: &str) -> Result<(ToolInstanceRow, Vec<String>), String> {
        let mut lines = Vec::new();
        let old = self.active_tool(&id.slot)?;
        let pending = old.as_ref().and_then(|o| o.pending_job_id);
        if let Some(o) = old.as_ref() {
            self.retire(o, kind, reason, source, None)?;
            lines.push(format!("공구 #{} ({}세대) 은퇴: {}", o.id, o.generation, reason));
        }
        let mut n = self.insert_tool(id, endmill_id, tool)?;
        if let Some(j) = pending {
            self.charge_pending(&mut n, j, tool, vb_limit_mm, &mut lines)?;
        }
        Ok((n, lines))
    }

    fn set_pass_metric(&self, job_id: i64, pass: i64, key: &str, value: serde_json::Value) -> Result<(), String> {
        let raw: Option<String> = self
            .conn()
            .query_row("SELECT metrics_json FROM job_passes WHERE job_id = ?1 AND pass = ?2", params![job_id, pass], |r| r.get(0))
            .optional()
            .map_err(err)?;
        let Some(raw) = raw else {
            return Ok(());
        };
        let mut m: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(o) = m.as_object_mut() {
            o.insert(key.to_string(), value);
        }
        self.conn()
            .execute("UPDATE job_passes SET metrics_json = ?3 WHERE job_id = ?1 AND pass = ?2", params![job_id, pass, json(&m)])
            .map(|_| ())
            .map_err(err)
    }

    pub fn save_tool(&self, t: &ToolInstanceRow) -> Result<i64, String> {
        let n = self
            .conn()
            .execute(
                "UPDATE tool_instances SET endmill_id = ?2, label = ?3, vb_band_mm = ?4, vb_max_mm = ?5, cut_min = ?6, removed_cm3 = ?7, jobs = ?8, field_json = ?9, history_json = ?10, anchor_um = ?11, pred_base_um = ?12, last_measured_um = ?13, last_ap_mm = ?14, thermal_damage = ?15, life_used_max = ?16, pending_job_id = ?17, updated_at = ?18, version = version + 1 WHERE id = ?1 AND version = ?19 AND retired_at IS NULL",
                params![
                    t.id,
                    t.endmill_id,
                    t.label,
                    fin(t.vb_band_mm),
                    fin(t.vb_max_mm),
                    fin(t.cut_min),
                    fin(t.removed_cm3),
                    t.jobs,
                    json(&t.field),
                    json(&t.history),
                    fin_opt(t.anchor_um),
                    fin(t.pred_base_um),
                    fin_opt(t.last_measured_um),
                    fin(t.last_ap_mm),
                    fin(t.thermal_damage),
                    fin(t.life_used_max),
                    t.pending_job_id,
                    t.updated_at.max(now_ms()),
                    t.version
                ],
            )
            .map_err(err)?;
        if n == 0 {
            return Err(format!("공구 개체 #{} 가 그 사이 리셋되었거나 다른 작업이 먼저 기록해 저장하지 않았습니다", t.id));
        }
        Ok(t.version + 1)
    }

    pub fn reset_tool(&self, id: &ToolIdentity, endmill_id: Option<i64>, tool: &ToolGeometry, vb_limit_mm: f64, reason: &str) -> Result<(ToolInstanceRow, Vec<String>), String> {
        let tx = self.conn().unchecked_transaction().map_err(err)?;
        let out = self.replace_active(id, endmill_id, tool, vb_limit_mm, "reset", reason, "operator")?;
        tx.commit().map_err(err)?;
        Ok(out)
    }

    pub fn list_tools(&self, include_retired: bool, limit: i64) -> Result<Vec<ToolInstanceRow>, String> {
        let sql = format!(
            "SELECT {} FROM tool_instances {} ORDER BY (retired_at IS NOT NULL), updated_at DESC LIMIT ?1",
            TOOL_COLS,
            if include_retired { "" } else { "WHERE retired_at IS NULL" }
        );
        let mut st = self.conn().prepare(&sql).map_err(err)?;
        let rows = st.query_map(params![limit.max(1)], tool_of).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn add_wear_event(&self, e: &WearEventRow) -> Result<i64, String> {
        self.conn()
            .execute(
                "INSERT INTO tool_wear_events(tool_instance_id, job_id, kind, cut_min_after, vb_before_mm, vb_after_mm, radial_um, source, note, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    e.tool_instance_id,
                    e.job_id,
                    e.kind,
                    fin(e.cut_min_after),
                    fin(e.vb_before_mm),
                    fin(e.vb_after_mm),
                    fin_opt(e.radial_um),
                    e.source,
                    e.note,
                    if e.created_at > 0 { e.created_at } else { now_ms() }
                ],
            )
            .map_err(err)?;
        Ok(self.conn().last_insert_rowid())
    }

    pub fn wear_events(&self, tool_instance_id: i64, limit: i64) -> Result<Vec<WearEventRow>, String> {
        let mut st = self
            .conn()
            .prepare(&format!("SELECT {} FROM tool_wear_events WHERE tool_instance_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2", EVENT_COLS))
            .map_err(err)?;
        let rows = st.query_map(params![tool_instance_id, limit.max(1)], event_of).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn job_events(&self, job_id: i64) -> Result<Vec<WearEventRow>, String> {
        let mut st = self
            .conn()
            .prepare(&format!("SELECT {} FROM tool_wear_events WHERE job_id = ?1 ORDER BY created_at, id", EVENT_COLS))
            .map_err(err)?;
        let rows = st.query_map(params![job_id], event_of).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn pass2_tool(&self, job_id: i64) -> Result<Option<i64>, String> {
        self.conn()
            .query_row(
                "SELECT tool_instance_id FROM tool_wear_events WHERE job_id = ?1 AND kind = 'pass2' ORDER BY id DESC LIMIT 1",
                params![job_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)
    }

    pub fn root_of(&self, job_id: i64) -> Result<i64, String> {
        Ok(self
            .conn()
            .query_row("SELECT COALESCE(root_job_id, id) FROM jobs WHERE id = ?1", params![job_id], |r| r.get(0))
            .optional()
            .map_err(err)?
            .unwrap_or(job_id))
    }

    pub fn insert_job(&self, j: &JobRow, summary: &serde_json::Value) -> Result<i64, String> {
        self.conn()
            .execute(
                "INSERT INTO jobs(project_id, name, profile_name, pattern, stamp, parent_job_id, root_job_id, tool_instance_id, endmill_id, workpiece_id, material_key, coating_family, coolant, final_action, decided_by, vb_start_mm, vb_end_mm, report_line, summary_json, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    j.project_id,
                    j.name,
                    j.profile_name,
                    j.pattern,
                    j.stamp,
                    j.parent_job_id,
                    j.root_job_id,
                    j.tool_instance_id,
                    j.endmill_id,
                    j.workpiece_id,
                    j.material_key,
                    j.coating_family,
                    j.coolant,
                    j.final_action,
                    j.decided_by,
                    fin(j.vb_start_mm),
                    fin(j.vb_end_mm),
                    j.report_line,
                    json(summary),
                    if j.created_at > 0 { j.created_at } else { now_ms() }
                ],
            )
            .map_err(err)?;
        Ok(self.conn().last_insert_rowid())
    }

    pub fn upsert_pass(&self, p: &JobPassRow) -> Result<(), String> {
        self.conn()
            .execute(
                "INSERT INTO job_passes(job_id, pass, status, action, cut_min, removed_cm3, vb_start_mm, vb_end_mm, radial_end_um, residual_p50_um, residual_p90_um, max_temp_c, max_force_n, chatter_fraction, chatter_wear, committed, field_json, metrics_json)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                 ON CONFLICT(job_id, pass) DO UPDATE SET status = excluded.status, action = excluded.action, cut_min = excluded.cut_min, removed_cm3 = excluded.removed_cm3, vb_start_mm = excluded.vb_start_mm, vb_end_mm = excluded.vb_end_mm, radial_end_um = excluded.radial_end_um, residual_p50_um = excluded.residual_p50_um, residual_p90_um = excluded.residual_p90_um, max_temp_c = excluded.max_temp_c, max_force_n = excluded.max_force_n, chatter_fraction = excluded.chatter_fraction, chatter_wear = excluded.chatter_wear, committed = excluded.committed, field_json = excluded.field_json, metrics_json = excluded.metrics_json",
                params![
                    p.job_id,
                    p.pass,
                    p.status,
                    p.action,
                    fin(p.cut_min),
                    fin(p.removed_cm3),
                    fin(p.vb_start_mm),
                    fin(p.vb_end_mm),
                    fin(p.radial_end_um),
                    fin_opt(p.residual_p50_um),
                    fin_opt(p.residual_p90_um),
                    fin_opt(p.max_temp_c),
                    fin_opt(p.max_force_n),
                    fin_opt(p.chatter_fraction),
                    fin_opt(p.chatter_wear),
                    i64::from(p.committed),
                    p.field.as_ref().map(json).unwrap_or_default(),
                    json(&p.metrics)
                ],
            )
            .map(|_| ())
            .map_err(err)
    }

    pub fn set_pass_status(&self, job_id: i64, pass: i64, status: &str, committed: bool) -> Result<(), String> {
        self.conn()
            .execute(
                "UPDATE job_passes SET status = ?3, committed = ?4 WHERE job_id = ?1 AND pass = ?2",
                params![job_id, pass, status, i64::from(committed)],
            )
            .map(|_| ())
            .map_err(err)
    }

    pub fn job_passes(&self, job_id: i64) -> Result<Vec<JobPassRow>, String> {
        let mut st = self.conn().prepare(&format!("SELECT {} FROM job_passes WHERE job_id = ?1 ORDER BY pass", PASS_COLS)).map_err(err)?;
        let rows = st.query_map(params![job_id], pass_of).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn jobs(&self, project_id: Option<i64>, tool_instance_id: Option<i64>, limit: i64) -> Result<Vec<JobRow>, String> {
        let sql = format!(
            "SELECT {} FROM jobs j LEFT JOIN tool_instances t ON t.id = j.tool_instance_id WHERE (?1 IS NULL OR j.project_id = ?1) AND (?2 IS NULL OR j.tool_instance_id = ?2) ORDER BY j.created_at DESC, j.id DESC LIMIT ?3",
            JOB_COLS
        );
        let mut st = self.conn().prepare(&sql).map_err(err)?;
        let rows = st.query_map(params![project_id, tool_instance_id, limit.max(1)], job_of).map_err(err)?;
        let mut out = rows.collect::<Result<Vec<_>, _>>().map_err(err)?;
        for j in out.iter_mut() {
            j.passes = self.job_passes(j.id)?;
            for p in j.passes.iter_mut() {
                p.field = None;
            }
        }
        Ok(out)
    }

    pub fn job_detail(&self, id: i64) -> Result<Option<JobDetail>, String> {
        let row = self
            .conn()
            .query_row(
                &format!("SELECT {}, j.summary_json FROM jobs j LEFT JOIN tool_instances t ON t.id = j.tool_instance_id WHERE j.id = ?1", JOB_COLS),
                params![id],
                |r| {
                    let j = job_of(r)?;
                    let s: String = r.get(22)?;
                    Ok((j, s))
                },
            )
            .optional()
            .map_err(err)?;
        let Some((mut job, summary)) = row else {
            return Ok(None);
        };
        job.passes = self.job_passes(id)?;
        let tool = match job.tool_instance_id {
            Some(t) => self.tool_by_id(t)?,
            None => None,
        };
        Ok(Some(JobDetail {
            summary: serde_json::from_str(&summary).unwrap_or(serde_json::Value::Null),
            records: self.cut_records(id)?,
            events: self.job_events(id)?,
            job,
            tool,
        }))
    }

    pub fn insert_cut_records(&self, rows: &[CutRecordRow]) -> Result<usize, String> {
        let mut st = self
            .conn()
            .prepare(&format!(
                "INSERT INTO cut_records({}) VALUES(NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33)",
                REC_COLS
            ))
            .map_err(err)?;
        let now = now_ms();
        let mut n = 0;
        for c in rows.iter() {
            st.execute(params![
                c.job_id,
                c.pass,
                c.tool_instance_id,
                c.endmill_id,
                c.tool_label,
                c.material_key,
                c.coating_family,
                c.coolant,
                fin(c.diameter_mm),
                c.flutes,
                fin(c.helix_deg),
                c.nose,
                c.mode,
                fin(c.ap_mm),
                fin(c.ae_mm),
                fin(c.phi_st_deg),
                fin(c.phi_ex_deg),
                fin(c.engage_deg),
                fin(c.vc_m_min),
                fin(c.fz_mm),
                fin(c.h_mean_um),
                fin(c.t_cut_s),
                fin(c.removed_mm3),
                fin(c.temp_c),
                fin(c.force_n),
                fin(c.chatter_min),
                fin(c.chatter_wear),
                fin(c.vb_start_mm),
                fin(c.dvb_mm),
                fin(c.vb_rate_um_min),
                json(&c.flute_shares),
                fin_opt(c.measured_ratio),
                if c.created_at > 0 { c.created_at } else { now }
            ])
            .map_err(err)?;
            n += 1;
        }
        Ok(n)
    }

    pub fn cut_records(&self, job_id: i64) -> Result<Vec<CutRecordRow>, String> {
        let mut st = self
            .conn()
            .prepare(&format!("SELECT {} FROM cut_records WHERE job_id = ?1 ORDER BY pass, t_cut_s DESC", REC_COLS))
            .map_err(err)?;
        let rows = st.query_map(params![job_id], record_of).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    pub fn set_measured_ratio(&self, job_id: i64, pass: i64, ratio: f64) -> Result<usize, String> {
        self.conn()
            .execute(
                "UPDATE cut_records SET measured_ratio = ?3 WHERE job_id = ?1 AND pass = ?2",
                params![job_id, pass, fin(ratio)],
            )
            .map_err(err)
    }

    pub fn candidate_records(&self, material_key: &str, flutes: i64, exclude_root: Option<i64>, limit: i64) -> Result<Vec<CutRecordRow>, String> {
        let cols: Vec<String> = REC_COLS.split(", ").map(|c| format!("c.{}", c)).collect();
        let mut st = self
            .conn()
            .prepare(&format!(
                "SELECT {} FROM cut_records c JOIN jobs j ON j.id = c.job_id LEFT JOIN job_passes jp ON jp.job_id = c.job_id AND jp.pass = c.pass WHERE c.material_key = ?1 AND c.flutes = ?2 AND (?3 IS NULL OR (c.job_id <> ?3 AND COALESCE(j.root_job_id, -1) <> ?3)) AND COALESCE(jp.status, '') NOT IN ('replaced', 'awaiting_tool') ORDER BY c.created_at DESC LIMIT ?4",
                cols.join(", ")
            ))
            .map_err(err)?;
        let rows = st.query_map(params![material_key, flutes, exclude_root, limit.max(1)], record_of).map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }
}

pub fn tool_identity(p: &MachiningProfile) -> ToolIdentity {
    let s = &p.endmill_setting;
    let attr = crate::store::attr::EndMillAttr::from_setting(s);
    let nose = match attr.nose.as_str() {
        "corner" => format!("cr{:.2}", attr.corner_r_mm),
        other => other.to_string(),
    };
    ToolIdentity {
        slot: format!("profile:{}", p.name),
        geom: format!(
            "d{:.2}:z{}:loc{:.1}:hx{:.0}:{}:c-{}:{}",
            attr.diameter_mm,
            attr.flutes,
            attr.loc_mm,
            attr.helix_deg,
            nose,
            attr.coating_family.to_lowercase(),
            attr.substrate
        ),
        label: s.name.clone(),
    }
}

pub fn material_key(p: &MachiningProfile) -> String {
    p.workpiece_setup.effective_material().family_key().to_string()
}

fn downsample(t: &[f64], y: &[f64], n: usize) -> Vec<(f64, f64)> {
    let m = t.len().min(y.len());
    if m == 0 {
        return Vec::new();
    }
    if m <= n {
        return (0..m).map(|i| (t[i], y[i])).collect();
    }
    (0..n).map(|k| k * (m - 1) / (n - 1).max(1)).map(|i| (t[i], y[i])).collect()
}

fn base_growth_um(growth_calib_mm: f64, growth_fixed_mm: f64, calib: f64, tan_clear: f64) -> f64 {
    (growth_calib_mm / calib.max(1e-6) + growth_fixed_mm).max(0.0) * tan_clear * 1000.0
}

#[allow(clippy::too_many_arguments)]
fn regime_record(p: &MachiningProfile, ctx: &CutContext, job_id: i64, pass: i64, tool_id: Option<i64>, endmill_id: Option<i64>, g: &CutRegime, vb_start: f64, shares: &[f64]) -> CutRecordRow {
    CutRecordRow {
        id: 0,
        job_id,
        pass,
        tool_instance_id: tool_id,
        endmill_id,
        tool_label: p.endmill_setting.name.clone(),
        material_key: material_key(p),
        coating_family: ctx.tool.coating.family.clone(),
        coolant: p.coolant_config.method.key().to_string(),
        diameter_mm: ctx.tool.diameter_mm,
        flutes: ctx.tool.flutes as i64,
        helix_deg: ctx.tool.helix_deg,
        nose: ctx.tool.nose.key(),
        mode: g.mode.clone(),
        ap_mm: g.ap_mm,
        ae_mm: g.ae_mm,
        phi_st_deg: g.phi_st_deg,
        phi_ex_deg: g.phi_ex_deg,
        engage_deg: g.engage_deg,
        vc_m_min: g.vc_m_min,
        fz_mm: g.fz_mm,
        h_mean_um: g.h_mean_um,
        t_cut_s: g.t_cut_s,
        removed_mm3: g.removed_mm3,
        temp_c: g.temp_c,
        force_n: g.force_n,
        chatter_min: g.chatter_min,
        chatter_wear: g.chatter_wear,
        vb_start_mm: vb_start,
        dvb_mm: g.dvb_mm,
        vb_rate_um_min: g.vb_rate_um_min,
        flute_shares: shares.to_vec(),
        measured_ratio: None,
        created_at: 0,
    }
}

pub fn pass2_regimes(p: &MachiningProfile, ctx: &CutContext, comp: &CompOutcome) -> Vec<CutRegime> {
    let action = comp.decision.final_action;
    if !action.cuts() {
        return Vec::new();
    }
    let rpm = comp.draft.rpm.max(1.0);
    let z = p.endmill_setting.flute_count.max(1) as f64;
    let vc = std::f64::consts::PI * p.endmill_setting.diameter_mm * rpm / 1000.0;
    let measured = ctx.tool.measured_fn_hz.is_some() && ctx.tool.measured_k_n_per_um.is_some();
    let mut out: Vec<CutRegime> = Vec::new();
    for zone in comp.draft.zones.iter().filter(|z| z.needs) {
        let passes = if action == CompAction::Split { vec![0usize, 1] } else { vec![1usize] };
        for ps in passes {
            let e = zone.eval_for(action, ps);
            let feed = (e.fz_mm * rpm * z).max(1e-6);
            let t = zone.len_mm / feed * 60.0;
            let eng = e.engage_deg;
            let (st, ex) = if zone.down { (180.0 - eng, 180.0) } else { (0.0, eng) };
            out.push(CutRegime {
                mode: if zone.down { "down".into() } else { "up".into() },
                ap_mm: zone.ap_mm,
                ae_mm: e.ae_um / 1000.0,
                phi_st_deg: st,
                phi_ex_deg: ex,
                engage_deg: eng,
                vc_m_min: vc,
                fz_mm: e.fz_mm,
                h_mean_um: 0.64 * e.h_max_um,
                t_cut_s: t,
                removed_mm3: zone.len_mm * zone.ap_mm * e.ae_um / 1000.0,
                temp_c: e.temp_c,
                force_n: e.force_n,
                chatter_min: e.chatter,
                chatter_wear: toolwear::chatter_wear_factor(e.chatter, measured),
                dvb_mm: e.vb_rate_mm_min * t / 60.0,
                vb_rate_um_min: e.vb_rate_mm_min * 1000.0,
                samples: 1,
            });
        }
    }
    let mut merged: Vec<CutRegime> = Vec::new();
    for r in out.into_iter() {
        let key = |g: &CutRegime| (g.mode.clone(), (g.ap_mm / 0.5).round() as i64, (g.ae_mm * 100.0).round() as i64);
        if let Some(m) = merged.iter_mut().find(|m| key(m) == key(&r)) {
            let (a, b) = (m.t_cut_s, r.t_cut_s);
            let w = |x: f64, y: f64| (x * a + y * b) / (a + b).max(1e-12);
            m.temp_c = w(m.temp_c, r.temp_c);
            m.force_n = w(m.force_n, r.force_n);
            m.h_mean_um = w(m.h_mean_um, r.h_mean_um);
            m.chatter_wear = w(m.chatter_wear, r.chatter_wear);
            m.chatter_min = m.chatter_min.min(r.chatter_min);
            m.t_cut_s += r.t_cut_s;
            m.removed_mm3 += r.removed_mm3;
            m.dvb_mm += r.dvb_mm;
            m.vb_rate_um_min = m.dvb_mm / m.t_cut_s.max(1e-12) * 60.0 * 1000.0;
            m.samples += 1;
        } else {
            merged.push(r);
        }
    }
    merged.sort_by(|a, b| b.t_cut_s.partial_cmp(&a.t_cut_s).unwrap_or(std::cmp::Ordering::Equal));
    merged.truncate(crate::loadsim::MAX_REGIMES);
    merged
}

pub struct JobInput<'a> {
    pub project_id: i64,
    pub profile: &'a MachiningProfile,
    pub ctx: &'a CutContext,
    pub pattern: &'a str,
    pub stamp: &'a str,
    pub parent_job_id: Option<i64>,
    pub endmill_id: Option<i64>,
    pub workpiece_id: Option<i64>,
    pub report: &'a LoadSimReport,
    pub log: &'a WearLog,
    pub comp: &'a CompOutcome,
    pub replan: bool,
    pub summary: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobRecordOutcome {
    pub job_id: i64,
    pub root_job_id: i64,
    pub tool: Option<ToolInstanceRow>,
    pub records: usize,
    pub lines: Vec<String>,
    #[serde(default)]
    pub handoff: Option<ToolInstanceRow>,
    #[serde(default)]
    pub handoff_vb_before_mm: f64,
}

fn revert_pass2(rdb: &Rdb, t: &mut ToolInstanceRow, parent: i64, lines: &mut Vec<String>) -> Result<(), String> {
    let passes = rdb.job_passes(parent)?;
    if let Some(p2) = passes.iter().find(|p| p.pass == 2 && p.status == "awaiting_tool") {
        rdb.set_pass_status(parent, 2, "replaced", false)?;
        if t.pending_job_id == Some(parent) {
            t.pending_job_id = None;
        }
        lines.push(format!("이전 작업 #{} 의 공구 교체 2차 계획({:.2}분)은 실행 전 재계획으로 대체", parent, p2.cut_min));
        return Ok(());
    }
    let Some(p2) = passes.iter().find(|p| p.pass == 2 && p.committed) else {
        return Ok(());
    };
    if let Some(f) = p2.field.as_ref() {
        t.field.subtract(f);
    }
    t.cut_min = (t.cut_min - p2.cut_min).max(0.0);
    t.removed_cm3 = (t.removed_cm3 - p2.removed_cm3).max(0.0);
    t.thermal_damage = (t.thermal_damage - metric(&p2.metrics, "thermal_damage")).max(0.0);
    let cut = t.cut_min;
    t.history.retain(|(tm, _)| *tm <= cut + 1e-9);
    t.pred_base_um = (t.pred_base_um - metric(&p2.metrics, "pred_base_um")).max(0.0);
    if let Some(v) = p2.metrics.get("life_used_max_before").and_then(|v| v.as_f64()).filter(|v| v.is_finite()) {
        t.life_used_max = v.max(0.0);
    }
    rdb.set_pass_status(parent, 2, "replaced", false)?;
    lines.push(format!("이전 작업 #{} 의 2차 계획은 실행되지 않은 것으로 보고 공구 누적 마모에서 되돌림 (재계획)", parent));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn release_parent_pass2(rdb: &Rdb, a: &mut ToolInstanceRow, parent: i64, tool: &ToolGeometry, ap: f64, vb_limit: f64, lines: &mut Vec<String>) -> Result<(), String> {
    let Some(p2) = rdb.job_passes(parent)?.into_iter().find(|p| p.pass == 2) else {
        return Ok(());
    };
    if !p2.committed {
        if p2.status == "awaiting_tool" {
            rdb.set_pass_status(parent, 2, "replaced", false)?;
        }
        return Ok(());
    }
    if rdb.pass2_tool(parent)? == Some(a.id) {
        revert_pass2(rdb, a, parent, lines)?;
        a.jobs = (a.jobs - 1).max(0);
        a.refresh(tool, ap, vb_limit);
        a.version = rdb.save_tool(a)?;
    } else {
        rdb.set_pass_status(parent, 2, "replaced", false)?;
        lines.push(format!("이전 작업 #{} 의 2차 계획은 리셋 전 공구 기록에 남기고 실행되지 않은 것으로 표시", parent));
    }
    Ok(())
}

fn record_inner(rdb: &Rdb, tool: Option<&ToolInstanceRow>, inp: &JobInput) -> Result<(JobRecordOutcome, Option<ToolInstanceRow>), String> {
    let p = inp.profile;
    let rep = inp.report;
    let comp = inp.comp;
    let d = &comp.draft;
    let action = comp.decision.final_action;
    let change = action == CompAction::ToolChange;
    let tan_clear = inp.ctx.tool.clearance_deg.clamp(2.0, 30.0).to_radians().tan();
    let ap = p.conditions.axial_doc_mm;
    let vb_limit = inp.ctx.vb_limit_mm();
    let mut lines: Vec<String> = Vec::new();
    let mut work: Option<ToolInstanceRow> = match tool {
        Some(t) if t.id == 0 => {
            let ident = tool_identity(p);
            match rdb.active_tool(&ident.slot)? {
                Some(a) if a.geom_key.is_empty() || a.geom_key == ident.geom => Some(a),
                old => {
                    let reason = match old.as_ref() {
                        Some(o) => format!("프리셋의 공구 사양 변경 ({} → {}) — 다른 공구로 보고 이전 개체 #{} 은퇴", o.geom_key, ident.geom, o.id),
                        None => "새 공구 개체".to_string(),
                    };
                    let (n, l) = rdb.replace_active(&ident, inp.endmill_id, &inp.ctx.tool, vb_limit, "geometry_change", &reason, "system")?;
                    lines.extend(l);
                    Some(n)
                }
            }
        }
        Some(t) => match rdb.tool_by_id(t.id)? {
            Some(fresh) if fresh.retired_at.is_none() => Some(fresh),
            _ => {
                lines.push(format!("공구 #{} 가 리셋(교체)되어 이번 기록의 마모는 그 공구 개체에 더하지 않음", t.id));
                None
            }
        },
        None => None,
    };
    let root = match (inp.replan, inp.parent_job_id) {
        (true, Some(parent)) => Some(rdb.root_of(parent)?),
        _ => None,
    };
    let job = JobRow {
        project_id: inp.project_id,
        name: format!("{} · {} · {}{}", inp.stamp, p.name, inp.pattern, if inp.replan { " · 재계획" } else { "" }),
        profile_name: p.name.clone(),
        pattern: inp.pattern.to_string(),
        stamp: inp.stamp.to_string(),
        parent_job_id: inp.parent_job_id,
        root_job_id: root,
        tool_instance_id: work.as_ref().map(|t| t.id).or(tool.map(|t| t.id)),
        endmill_id: inp.endmill_id,
        workpiece_id: inp.workpiece_id,
        material_key: material_key(p),
        coating_family: inp.ctx.tool.coating.family.clone(),
        coolant: p.coolant_config.method.key().to_string(),
        final_action: action.key().to_string(),
        decided_by: comp.decision.decided_by.clone(),
        vb_start_mm: rep.vb_start_mm,
        vb_end_mm: if action.cuts() && !change { comp.vb_after_mm } else { d.vb_now_mm.max(rep.vb_end_mm) },
        report_line: comp.report_line.clone(),
        created_at: now_ms(),
        ..Default::default()
    };
    let job_id = rdb.insert_job(&job, &inp.summary)?;
    let root_id = root.unwrap_or(job_id);
    let vb_series = downsample(&inp.log.t_cut_s, &inp.log.vb_mm, HISTORY_PER_PASS);
    let p1_base = base_growth_um(rep.growth_calib_mm, rep.growth_fixed_mm, d.calib_wear, tan_clear);
    rdb.upsert_pass(&JobPassRow {
        job_id,
        pass: 1,
        status: if inp.replan { "reused".into() } else { "done".into() },
        action: "pass1".into(),
        cut_min: rep.cut_time_min,
        removed_cm3: rep.removed_volume_cm3,
        vb_start_mm: rep.vb_start_mm,
        vb_end_mm: rep.vb_end_mm,
        radial_end_um: rep.radial_loss_end_um,
        residual_p50_um: Some(d.residual.p50_um),
        residual_p90_um: Some(d.residual.p90_um),
        max_temp_c: Some(rep.max_temp_c),
        max_force_n: Some(rep.max_force_n),
        chatter_fraction: Some(rep.chatter_fraction),
        chatter_wear: Some(rep.chatter_wear_factor),
        committed: !inp.replan,
        field: Some(rep.wear_field.clone()),
        metrics: serde_json::json!({
            "vb_series": vb_series,
            "pred_base_um": p1_base,
            "growth_calib_mm": rep.growth_calib_mm,
            "growth_fixed_mm": rep.growth_fixed_mm,
            "calib_wear": d.calib_wear,
            "flute_shares": rep.flute_shares,
            "wp_temp_end_c": rep.wp_temp_end_c,
            "removed_cm3": rep.removed_volume_cm3,
            "parent_job_id": inp.parent_job_id,
            "root_job_id": root_id,
            "thermal_damage": rep.thermal_damage,
            "thermal_fatigue_per_min": rep.thermal_fatigue_per_min,
            "ap_mm": ap,
        }),
    })?;
    let p2_cut_min = if action.cuts() { comp.cut_time_s / 60.0 } else { 0.0 };
    let p2_vb0 = if change { 0.0 } else { d.vb_now_mm };
    let p2_base = if action.cuts() { (comp.vb_after_mm - p2_vb0).max(0.0) / d.calib_wear.max(1e-6) * tan_clear * 1000.0 } else { 0.0 };
    let regimes2 = pass2_regimes(p, inp.ctx, comp);
    let p2_removed: f64 = regimes2.iter().map(|g| g.removed_mm3).sum::<f64>() / 1000.0;
    rdb.upsert_pass(&JobPassRow {
        job_id,
        pass: 2,
        status: match action {
            CompAction::Hold => "hold".into(),
            CompAction::Skip => "skip".into(),
            CompAction::ToolChange => "awaiting_tool".into(),
            a if a.cuts() => "planned".into(),
            _ => "none".into(),
        },
        action: action.key().to_string(),
        cut_min: p2_cut_min,
        removed_cm3: p2_removed,
        vb_start_mm: p2_vb0,
        vb_end_mm: if action.cuts() { comp.vb_after_mm } else { d.vb_now_mm },
        radial_end_um: if action.cuts() { comp.vb_after_mm * tan_clear * 1000.0 } else { d.radial_used_um },
        residual_p50_um: Some(comp.after_p50_um),
        residual_p90_um: None,
        max_temp_c: None,
        max_force_n: None,
        chatter_fraction: None,
        chatter_wear: None,
        committed: action.cuts() && !change && work.is_some(),
        field: if action.cuts() { Some(comp.wear_field2.clone()) } else { None },
        metrics: serde_json::json!({
            "pred_base_um": p2_base,
            "after_min_um": comp.after_min_um,
            "after_max_um": comp.after_max_um,
            "parts_left_p50": comp.parts_left_p50,
            "parts_left_p90": comp.parts_left_p90,
            "parts_source": comp.parts_source,
            "program_name": comp.program_name,
            "thermal_damage": comp.thermal_damage_pass2,
            "thermal_damage_after": comp.thermal_damage_after,
            "ap_mm": ap,
            "new_tool": change,
        }),
    })?;
    let tool_id = work.as_ref().map(|t| t.id);
    let endmill_id = work.as_ref().and_then(|t| t.endmill_id).or(inp.endmill_id);
    let mut records: Vec<CutRecordRow> = Vec::new();
    if !inp.replan {
        for g in rep.regimes.iter() {
            records.push(regime_record(p, inp.ctx, job_id, 1, tool_id, endmill_id, g, rep.vb_start_mm, &rep.flute_shares));
        }
    }
    for g in regimes2.iter() {
        records.push(regime_record(p, inp.ctx, job_id, 2, if change { None } else { tool_id }, endmill_id, g, p2_vb0, &rep.flute_shares));
    }
    let n_rec = rdb.insert_cut_records(&records)?;
    lines.push(format!(
        "가공 기록 프로젝트 #{} 생성{} — 1차 {} · 2차 {} · 절삭 영역 기록 {}건 (진입·이탈 각, ap·ae, 날 하중 분배 포함)",
        job_id,
        if inp.replan { format!(" (원 작업 #{} 의 재계획)", root_id) } else { String::new() },
        if inp.replan { "재사용" } else { "기록" },
        match action {
            CompAction::Hold => "보류",
            CompAction::Skip => "안 함",
            CompAction::ToolChange => "새 공구 대기",
            _ => "계획",
        },
        n_rec
    ));
    let mut handoff: Option<ToolInstanceRow> = None;
    let mut handoff_vb_before_mm = 0.0;
    if change && work.is_none() {
        if let Some(t) = tool.filter(|t| t.id > 0) {
            match rdb.active_tool(&t.slot_key)? {
                Some(mut a) if a.geom_key == t.geom_key => {
                    if let (true, Some(parent)) = (inp.replan, inp.parent_job_id) {
                        release_parent_pass2(rdb, &mut a, parent, &inp.ctx.tool, ap, vb_limit, &mut lines)?;
                    }
                    handoff_vb_before_mm = a.vb_band_mm;
                    rdb.charge_pending(&mut a, job_id, &inp.ctx.tool, vb_limit, &mut lines)?;
                    handoff = Some(a);
                }
                _ => {
                    rdb.set_pass_status(job_id, 2, "replaced", false)?;
                    lines.push("공구가 바뀌었는데 같은 사양의 새 공구가 없어 이 작업의 공구 교체 2차 계획을 연결하지 않음 — 새 공구로 다시 계획하세요".into());
                }
            }
        }
    }
    if let Some(w) = work.as_mut() {
        if w.conform(&inp.ctx.tool) {
            lines.push(format!("공구 #{} 의 마모 지도를 바뀐 날 길이 {:.2} mm 에 맞춰 축방향으로 다시 표본화", w.id, inp.ctx.tool.loc_mm));
        }
        if inp.replan {
            if let Some(parent) = inp.parent_job_id {
                revert_pass2(rdb, w, parent, &mut lines)?;
            }
            if d.radial_source == "measured" {
                let measured = d.radial_used_um.max(0.0);
                let before = w.vb_band_mm;
                let earlier = match d.tool.as_ref() {
                    Some(t) => t.anchor_um,
                    None => w.anchor_um,
                };
                let m = match earlier {
                    Some(a) if measured < a => {
                        lines.push(format!("실측 반경 마모 {:.1} µm 가 이 부품 전에 잰 {:.1} µm 보다 작아 측정 산포로 보고 그 값을 유지 (마모는 줄지 않음)", measured, a));
                        a
                    }
                    _ => measured,
                };
                if let Some(prev) = w.anchor_um.filter(|a| Some(*a) != earlier && (a - m).abs() > 1e-9) {
                    lines.push(format!("같은 부품의 앞선 재계획 실측 {:.1} µm 를 {:.1} µm 로 정정", prev, m));
                }
                if let Some((meas, base)) = crate::wearcomp::calibration_pair(d) {
                    let ratio = meas / base.max(1e-9);
                    let n = rdb.set_measured_ratio(root_id, 1, ratio)?;
                    if n > 0 {
                        lines.push(format!(
                            "실측/보정 전 예측 마모 비 ×{:.2} 를 원 작업 #{} 의 1차 절삭 기록 {}건에 기록 — 이후 비슷한 절삭은 그때의 보정 계수로 나눈 나머지만 참조",
                            ratio, root_id, n
                        ));
                    }
                }
                let axial = AxialProfile::of(inp.ctx);
                w.field.anchor_band(ap, m / 1000.0 / tan_clear, &axial, &rep.flute_shares);
                w.anchor_um = Some(m);
                w.pred_base_um = 0.0;
                w.last_measured_um = Some(measured);
                w.life_used_max = 0.0;
                w.refresh(&inp.ctx.tool, ap, vb_limit);
                let (cut, band) = (w.cut_min, w.vb_band_mm);
                w.push_history(cut, band);
                rdb.add_wear_event(&WearEventRow {
                    tool_instance_id: w.id,
                    job_id: Some(job_id),
                    kind: "measure".into(),
                    cut_min_after: w.cut_min,
                    vb_before_mm: before,
                    vb_after_mm: w.vb_band_mm,
                    radial_um: Some(measured),
                    source: "measured".into(),
                    note: "재계획 실측 반경 마모로 공구 상태를 고정 (이후 학습 기준점)".into(),
                    ..Default::default()
                })?;
                lines.push(format!("공구 #{} 상태를 실측 반경 마모 {:.1} µm (VB {:.3} mm) 로 고정", w.id, m, w.vb_band_mm));
            }
        } else {
            if let Some(prev) = w.pending_job_id.take() {
                lines.push(format!(
                    "이전 작업 #{} 의 공구 교체 결정 뒤 리셋이 없어 같은 공구로 계속 가공한 것으로 기록 (그 작업의 새 공구 2차는 미반영)",
                    prev
                ));
                rdb.set_pass_status(prev, 2, "replaced", false)?;
            }
            let before = w.vb_band_mm;
            let t0 = w.cut_min;
            if !w.field.add_any(&rep.wear_field) {
                w.field = w.field_for(&inp.ctx.tool);
                w.field.add_any(&rep.wear_field);
            }
            w.cut_min += rep.cut_time_min;
            w.removed_cm3 += rep.removed_volume_cm3;
            w.jobs += 1;
            w.pred_base_um += p1_base;
            w.thermal_damage += rep.thermal_damage.max(0.0);
            for (tc, v) in vb_series.iter() {
                w.push_history(t0 + tc / 60.0, *v);
            }
            w.refresh(&inp.ctx.tool, ap, vb_limit);
            rdb.add_wear_event(&WearEventRow {
                tool_instance_id: w.id,
                job_id: Some(job_id),
                kind: "pass1".into(),
                cut_min_after: w.cut_min,
                vb_before_mm: before,
                vb_after_mm: w.vb_band_mm,
                radial_um: Some(rep.radial_loss_end_um),
                source: "sim".into(),
                note: format!(
                    "1차 절삭 {:.2}분 · 제거 {:.2} cm³{}",
                    rep.cut_time_min,
                    rep.removed_volume_cm3,
                    if rep.thermal_damage > 1e-4 { format!(" · 열피로 +{:.3}", rep.thermal_damage) } else { String::new() }
                ),
                ..Default::default()
            })?;
        }
        if change {
            w.pending_job_id = Some(job_id);
            rdb.add_wear_event(&WearEventRow {
                tool_instance_id: w.id,
                job_id: Some(job_id),
                kind: "change_pending".into(),
                cut_min_after: w.cut_min,
                vb_before_mm: w.vb_band_mm,
                vb_after_mm: w.vb_band_mm,
                source: "plan".into(),
                note: "공구 교체 후 보정 결정 — 새 공구를 장착하고 리셋하면 2차 마모가 새 공구 개체에 기록됨".into(),
                ..Default::default()
            })?;
            lines.push(format!(
                "결정이 공구 교체 후 보정이라 2차 {:.2}분은 지금 공구 #{} 에 더하지 않음 — 새 공구를 장착한 뒤 '공구 리셋'을 누르면 새 공구 개체에 기록",
                p2_cut_min, w.id
            ));
        } else if action.cuts() {
            let before = w.vb_band_mm;
            rdb.set_pass_metric(job_id, 2, "life_used_max_before", serde_json::json!(fin(w.life_used_max)))?;
            if !w.field.add_any(&comp.wear_field2) {
                w.field = w.field_for(&inp.ctx.tool);
                w.field.add_any(&comp.wear_field2);
            }
            w.cut_min += p2_cut_min;
            w.removed_cm3 += p2_removed;
            w.pred_base_um += p2_base;
            w.thermal_damage += comp.thermal_damage_pass2.max(0.0);
            w.refresh(&inp.ctx.tool, ap, vb_limit);
            let (cut, band) = (w.cut_min, w.vb_band_mm);
            w.push_history(cut, band);
            rdb.add_wear_event(&WearEventRow {
                tool_instance_id: w.id,
                job_id: Some(job_id),
                kind: "pass2".into(),
                cut_min_after: w.cut_min,
                vb_before_mm: before,
                vb_after_mm: w.vb_band_mm,
                radial_um: None,
                source: "sim".into(),
                note: format!("2차 {} {:.2}분", action.label(), p2_cut_min),
                ..Default::default()
            })?;
        } else if inp.replan {
            w.pending_job_id = None;
        }
        w.refresh(&inp.ctx.tool, ap, vb_limit);
        w.version = rdb.save_tool(w)?;
        let s = toolwear::summarize_state(&w.field, &inp.ctx.tool, ap, vb_limit, w.thermal_damage, w.life_used_max);
        lines.push(format!(
            "공구 #{} ({}세대) 누적: 절삭 {:.1}분 · 작업 {}회 · VB {:.3} mm (국부 최대 {:.3}) · 열피로 {:.2} · 수명 사용 최대 {:.0}%",
            w.id,
            w.generation,
            w.cut_min,
            w.jobs,
            w.vb_band_mm,
            w.vb_max_mm,
            w.thermal_damage,
            s.life_used_max * 100.0
        ));
    }
    Ok((
        JobRecordOutcome {
            job_id,
            root_job_id: root_id,
            tool: work.clone(),
            records: n_rec,
            lines,
            handoff,
            handoff_vb_before_mm,
        },
        work,
    ))
}

pub fn record_job(rdb: &Rdb, tool: Option<&mut ToolInstanceRow>, inp: &JobInput) -> Result<JobRecordOutcome, String> {
    let tx = rdb.conn().unchecked_transaction().map_err(err)?;
    let (out, after) = record_inner(rdb, tool.as_deref(), inp)?;
    tx.commit().map_err(err)?;
    if let (Some(t), Some(a)) = (tool, after) {
        *t = a;
    }
    Ok(out)
}

fn nose_family(key: &str) -> (&str, f64) {
    if let Some(r) = key.strip_prefix("cr") {
        ("cr", r.parse::<f64>().unwrap_or(0.0))
    } else {
        (key, 0.0)
    }
}

pub fn reference_summary(rdb: &Rdb, p: &MachiningProfile, ctx: &CutContext, query: &[CutRegime], exclude_root: Option<i64>, k: usize) -> Result<RefSummary, String> {
    let cands = rdb.candidate_records(&material_key(p), ctx.tool.flutes as i64, exclude_root, 3000)?;
    let mut out = RefSummary::default();
    if cands.is_empty() || query.is_empty() {
        return Ok(out);
    }
    let d = ctx.tool.diameter_mm.max(0.1);
    let coat = ctx.tool.coating.family.clone();
    let coolant = p.coolant_config.method.key().to_string();
    let nose_key = ctx.tool.nose.key();
    let (nose_fam, nose_r) = nose_family(&nose_key);
    let calib = ctx.calib.wear.max(1e-6);
    let t_max: f64 = query.iter().map(|q| q.t_cut_s).fold(0.0, f64::max).max(1e-9);
    let mut best_by_job: std::collections::HashMap<i64, (f64, &CutRecordRow, f64)> = std::collections::HashMap::new();
    for c in cands.iter() {
        let (fam, r) = nose_family(&c.nose);
        if fam != nose_fam {
            continue;
        }
        let cd = c.diameter_mm.max(0.1);
        let mut best = 0.0f64;
        let mut best_rate_ratio = 1.0;
        for q in query.iter() {
            let lr = |a: f64, b: f64| (a.max(1e-9) / b.max(1e-9)).ln();
            let mut d2 = ((q.ap_mm / d - c.ap_mm / cd) / 0.15).powi(2)
                + ((q.ae_mm / d - c.ae_mm / cd) / 0.05).powi(2)
                + ((q.phi_st_deg - c.phi_st_deg) / 30.0).powi(2)
                + ((q.phi_ex_deg - c.phi_ex_deg) / 30.0).powi(2)
                + (lr(q.vc_m_min, c.vc_m_min) / 0.3).powi(2)
                + (lr(q.fz_mm, c.fz_mm) / 0.3).powi(2)
                + (lr(d, cd) / 0.3).powi(2)
                + ((nose_r - r) / (0.1 * d)).powi(2)
                + ((ctx.tool.helix_deg - c.helix_deg) / 15.0).powi(2);
            if c.coating_family != coat {
                d2 += 4.0;
            }
            if c.coolant != coolant {
                d2 += 1.0;
            }
            if c.mode != q.mode {
                d2 += 1.0;
            }
            let sim = (-0.5 * d2).exp() * (0.5 + 0.5 * q.t_cut_s / t_max);
            if sim > best {
                best = sim;
                best_rate_ratio = if c.vb_rate_um_min > 1e-9 { q.vb_rate_um_min / c.vb_rate_um_min } else { 1.0 };
            }
        }
        if best > REF_MIN_SIMILARITY {
            let e = best_by_job.entry(c.job_id).or_insert((best, c, best_rate_ratio));
            let better = best > e.0 || (c.measured_ratio.is_some() && e.1.measured_ratio.is_none() && best > 0.8 * e.0);
            if better {
                *e = (best, c, best_rate_ratio);
            }
        }
    }
    let mut scored: Vec<(f64, &CutRecordRow, f64)> = best_by_job.into_values().collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k.max(1));
    if scored.is_empty() {
        return Ok(out);
    }
    out.n = scored.len();
    out.mean_similarity = scored.iter().map(|s| s.0).sum::<f64>() / scored.len() as f64;
    let wsum: f64 = scored.iter().map(|s| s.0).sum::<f64>().max(1e-12);
    out.sim_rate_ratio = Some((scored.iter().map(|s| s.0 * s.2.max(1e-6).ln()).sum::<f64>() / wsum).exp());
    let mut meas: Vec<(f64, f64)> = scored
        .iter()
        .filter_map(|s| s.1.measured_ratio.filter(|r| r.is_finite() && *r > 0.0).map(|r| (r / calib, s.0)))
        .collect();
    out.n_measured = meas.len();
    if !meas.is_empty() {
        meas.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let tw: f64 = meas.iter().map(|m| m.1).sum();
        let q = |p: f64| {
            let mut acc = 0.0;
            for (v, w) in meas.iter() {
                acc += w;
                if acc >= p * tw {
                    return *v;
                }
            }
            meas[meas.len() - 1].0
        };
        out.wear_ratio_p50 = Some(q(0.5));
        out.wear_ratio_p75 = Some(q(0.75));
    }
    out.refs = scored
        .iter()
        .map(|(s, c, _)| RefCut {
            record_id: c.id,
            job_id: c.job_id,
            pass: c.pass,
            similarity: *s,
            mode: c.mode.clone(),
            ap_mm: c.ap_mm,
            ae_mm: c.ae_mm,
            phi_st_deg: c.phi_st_deg,
            phi_ex_deg: c.phi_ex_deg,
            vc_m_min: c.vc_m_min,
            fz_mm: c.fz_mm,
            vb_rate_um_min: c.vb_rate_um_min,
            chatter_min: c.chatter_min,
            measured_ratio: c.measured_ratio.map(|r| r / calib),
            tool_label: c.tool_label.clone(),
            material_key: c.material_key.clone(),
            created_at: c.created_at,
        })
        .collect();
    out.lines.push(format!(
        "비슷한 과거 절삭 {}개 작업 (같은 피삭재·날 수·날끝 형상, 진입·이탈 각·ap/D·ae/D·vc·fz·코너 R·헬릭스 거리, 작업마다 가장 비슷한 1건, 평균 유사도 {:.2}) · 실측이 있는 작업 {}개{}",
        out.n,
        out.mean_similarity,
        out.n_measured,
        match (out.wear_ratio_p50, out.wear_ratio_p75) {
            (Some(a), Some(b)) => format!(" · 현재 보정 대비 실측/예측 마모 비 중앙 ×{:.2} · 상위 75% ×{:.2}{}", a, b, if out.n_measured < 3 { " (작업 3개 미만이라 계획에는 미반영)" } else { "" }),
            _ => String::new(),
        }
    ));
    for r in out.refs.iter().take(3) {
        out.lines.push(format!(
            "  작업 #{} {}차 {} ap {:.2} · ae {:.2} mm · 각 {:.0}°→{:.0}° · 마모 속도 {:.2} µm/min{} (유사도 {:.2})",
            r.job_id,
            r.pass,
            r.mode,
            r.ap_mm,
            r.ae_mm,
            r.phi_st_deg,
            r.phi_ex_deg,
            r.vb_rate_um_min,
            r.measured_ratio.map(|m| format!(" · 실측 비 ×{:.2}", m)).unwrap_or_default(),
            r.similarity
        ));
    }
    Ok(out)
}

impl super::Store {
    pub fn prepare_tool(&mut self, p: &MachiningProfile, ctx: &mut CutContext) -> Result<(ToolInstanceRow, ToolStart, Option<String>), String> {
        let id = tool_identity(p);
        let endmill_id = self.index_endmill(&p.endmill_setting).ok();
        let (row, note) = self.rdb.ensure_tool(&id, endmill_id, &ctx.tool)?;
        let start = row.start(&ctx.tool, p.conditions.axial_doc_mm);
        ctx.tool.flank_wear_mm = start.vb_band_mm;
        ctx.tool_age_min = start.cut_min.max(0.0);
        Ok((row, start, note))
    }

    pub fn tool_for(&self, p: &MachiningProfile) -> Result<ToolInstanceRow, String> {
        let id = tool_identity(p);
        if let Some(t) = self.rdb.active_tool(&id.slot)? {
            return Ok(t);
        }
        let ctx = CutContext::from_profile(p);
        Ok(ToolInstanceRow {
            slot_key: id.slot,
            geom_key: id.geom,
            endmill_id: self.rdb.endmill_id_of(&p.endmill_setting)?,
            label: id.label,
            field: WearField::for_tool(&ctx.tool),
            history: Vec::new(),
            ..Default::default()
        })
    }

    pub fn reset_tool_for(&mut self, p: &MachiningProfile, reason: &str) -> Result<(ToolInstanceRow, Vec<String>), String> {
        let id = tool_identity(p);
        let endmill_id = self.index_endmill(&p.endmill_setting).ok();
        let ctx = CutContext::from_profile(p);
        self.rdb.reset_tool(&id, endmill_id, &ctx.tool, ctx.vb_limit_mm(), reason)
    }
}
