use super::modernbert::{ModernBert, ModernBertConfig};
use candle_core::{Device, IndexOp, Result, Tensor, D};
use candle_nn::{layer_norm, linear, LayerNorm, Linear, VarBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

pub const QTYPE_CHOICE: usize = 0;
pub const QTYPE_SCORE: usize = 1;
pub const QTYPE_NOUL: usize = 2;

struct HeadLayer {
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
    heads: usize,
    norm1: LayerNorm,
    norm2: LayerNorm,
    linear1: Linear,
    linear2: Linear,
}

impl HeadLayer {
    fn load(d: usize, vb: VarBuilder) -> Result<Self> {
        let w = vb.get((3 * d, d), "self_attn.in_proj_weight")?.chunk(3, 0)?;
        let b = vb.get(3 * d, "self_attn.in_proj_bias")?.chunk(3, 0)?;
        Ok(Self {
            q: Linear::new(w[0].clone(), Some(b[0].clone())),
            k: Linear::new(w[1].clone(), Some(b[1].clone())),
            v: Linear::new(w[2].clone(), Some(b[2].clone())),
            o: linear(d, d, vb.pp("self_attn.out_proj"))?,
            heads: (d / 64).max(1),
            norm1: layer_norm(d, 1e-5, vb.pp("norm1"))?,
            norm2: layer_norm(d, 1e-5, vb.pp("norm2"))?,
            linear1: linear(d, 4 * d, vb.pp("linear1"))?,
            linear2: linear(4 * d, d, vb.pp("linear2"))?,
        })
    }

    fn forward(&self, x: &Tensor, key_mask: Option<&Tensor>) -> Result<Tensor> {
        let (b, n, d) = x.dims3()?;
        let hd = d / self.heads;
        let y = x.apply(&self.norm1)?;
        let shape = (b, n, self.heads, hd);
        let q = y.apply(&self.q)?.reshape(shape)?.transpose(1, 2)?.contiguous()?;
        let k = y.apply(&self.k)?.reshape(shape)?.transpose(1, 2)?.contiguous()?;
        let v = y.apply(&self.v)?.reshape(shape)?.transpose(1, 2)?.contiguous()?;
        let mut att = (q.matmul(&k.t()?)? / (hd as f64).sqrt())?;
        if let Some(m) = key_mask {
            att = att.broadcast_add(m)?;
        }
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        let a = att.matmul(&v)?.transpose(1, 2)?.reshape((b, n, d))?.apply(&self.o)?;
        let x = (x + a)?;
        let f = x.apply(&self.norm2)?.apply(&self.linear1)?.relu()?.apply(&self.linear2)?;
        x + f
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvisorAnswer {
    pub qtype: String,
    pub labels: Vec<String>,
    pub probabilities: Vec<f32>,
    pub top: String,
    pub top_probability: f32,
    pub act_probability: f32,
    pub temperature: f32,
    pub state_tokens_used: usize,
    pub state_tokens_dropped: usize,
}

pub struct LayaAdvisor {
    encoder: ModernBert,
    head: Vec<HeadLayer>,
    type_emb: Tensor,
    scorer_ln: LayerNorm,
    scorer_l1: Linear,
    scorer_l2: Linear,
    act_l1: Linear,
    act_l2: Linear,
    temperature: [f32; 3],
    temperature_by_options: HashMap<String, f32>,
    tokenizer: Option<Tokenizer>,
    cls_id: u32,
    sep_id: u32,
    mask_id: u32,
    pub max_len: usize,
    pub head_max_len: usize,
    pub dir: PathBuf,
    device: Device,
}

fn clamp_temperature(t: f64) -> f32 {
    if !t.is_finite() {
        return 1.0;
    }
    t.clamp(0.5, 5.0) as f32
}

pub fn temp_bucket(qtype: usize, k: usize) -> String {
    let size = if k <= 2 {
        "2"
    } else if k <= 5 {
        "3-5"
    } else if k <= 10 {
        "6-10"
    } else {
        "11+"
    };
    let name = match qtype {
        QTYPE_CHOICE => "choice",
        QTYPE_SCORE => "score",
        _ => "noul",
    };
    format!("{}:{}", name, size)
}

impl LayaAdvisor {
    fn load_parts(enc_cfg: &ModernBertConfig, vb: VarBuilder, head_layers: usize, n_act: usize) -> Result<(ModernBert, Vec<HeadLayer>, Tensor, LayerNorm, Linear, Linear, Linear, Linear)> {
        let d = enc_cfg.hidden_size;
        let encoder = ModernBert::load(enc_cfg, vb.pp("encoder"))?;
        let mut head = Vec::with_capacity(head_layers);
        for i in 0..head_layers {
            head.push(HeadLayer::load(d, vb.pp(format!("head.layers.{}", i)))?);
        }
        let type_emb = vb.get((3, d), "type_emb.weight")?;
        let scorer_ln = layer_norm(d, 1e-5, vb.pp("scorer.0"))?;
        let scorer_l1 = linear(d, d, vb.pp("scorer.1"))?;
        let scorer_l2 = linear(d, 1, vb.pp("scorer.3"))?;
        let act_l1 = linear(d + 4, 256, vb.pp("act_head.0"))?;
        let act_l2 = linear(256, n_act, vb.pp("act_head.2"))?;
        Ok((encoder, head, type_emb, scorer_ln, scorer_l1, scorer_l2, act_l1, act_l2))
    }

    pub fn load(dir: &Path, device: &Device) -> std::result::Result<Self, String> {
        let agent_cfg = super::read_json(&dir.join("rl_agent_config.json")).unwrap_or(serde_json::Value::Null);
        let enc_v = super::read_json(&dir.join("encoder").join("config.json"))?;
        let enc_cfg = ModernBertConfig::from_json(&enc_v)?;
        let head_layers = agent_cfg.get("head_layers").and_then(|x| x.as_u64()).unwrap_or(2) as usize;
        let n_act = agent_cfg
            .get("act_costs")
            .and_then(|x| x.as_object())
            .map(|m| m.len() + 1)
            .unwrap_or(2);
        let vb = super::load_varbuilder(dir, device)?;
        let temp_raw: Vec<f64> = vb
            .get(3, "temperature")
            .ok()
            .and_then(|t| t.to_vec1::<f32>().ok())
            .map(|v| v.into_iter().map(|x| x as f64).collect())
            .unwrap_or_default();
        let (encoder, head, type_emb, scorer_ln, scorer_l1, scorer_l2, act_l1, act_l2) =
            Self::load_parts(&enc_cfg, vb, head_layers, n_act).map_err(|e| format!("laya 가중치 로드 실패: {}", e))?;
        let cfg_temp: Vec<f64> = agent_cfg
            .get("temperature")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
            .unwrap_or_default();
        let base = if cfg_temp.len() == 3 {
            cfg_temp
        } else if temp_raw.len() == 3 {
            temp_raw
        } else {
            vec![1.0, 1.0, 1.0]
        };
        let mut temperature_by_options = HashMap::new();
        if let Some(m) = agent_cfg.get("temperature_by_options").and_then(|x| x.as_object()) {
            for (k, v) in m.iter() {
                if let Some(t) = v.as_f64() {
                    temperature_by_options.insert(k.clone(), clamp_temperature(t));
                }
            }
        }
        let tok_path = [dir.join("tokenizer").join("tokenizer.json"), dir.join("tokenizer.json")]
            .into_iter()
            .find(|p| p.exists());
        let tokenizer = match tok_path {
            Some(p) => Some(Tokenizer::from_file(&p).map_err(|e| format!("laya tokenizer: {}", e))?),
            None => None,
        };
        let id_of = |tok: &Option<Tokenizer>, s: &str, fallback: u32| -> u32 {
            tok.as_ref().and_then(|t| t.token_to_id(s)).unwrap_or(fallback)
        };
        let cls_id = id_of(&tokenizer, "[CLS]", enc_v.get("cls_token_id").and_then(|x| x.as_u64()).unwrap_or(50281) as u32);
        let sep_id = id_of(&tokenizer, "[SEP]", enc_v.get("sep_token_id").and_then(|x| x.as_u64()).unwrap_or(50282) as u32);
        let mask_id = id_of(&tokenizer, "[MASK]", 50284);
        Ok(Self {
            encoder,
            head,
            type_emb,
            scorer_ln,
            scorer_l1,
            scorer_l2,
            act_l1,
            act_l2,
            temperature: [
                clamp_temperature(base[0]),
                clamp_temperature(base[1]),
                clamp_temperature(base[2]),
            ],
            temperature_by_options,
            tokenizer,
            cls_id,
            sep_id,
            mask_id,
            max_len: agent_cfg.get("max_len").and_then(|x| x.as_u64()).unwrap_or(1024) as usize,
            head_max_len: agent_cfg.get("head_max_len").and_then(|x| x.as_u64()).unwrap_or(256) as usize,
            dir: dir.to_path_buf(),
            device: device.clone(),
        })
    }

    pub fn forward_ids(&self, ids: &[u32], markers: &[usize], qtype: usize) -> Result<(Vec<f32>, Vec<f32>)> {
        let n = ids.len();
        let t = Tensor::from_vec(ids.to_vec(), (1, n), &self.device)?;
        let mut h = self.encoder.forward(&t, None)?;
        let te = self.type_emb.i(qtype)?.reshape((1, 1, ()))?;
        h = h.broadcast_add(&te)?;
        for l in self.head.iter() {
            h = l.forward(&h, None)?;
        }
        let h0 = h.i(0)?;
        let mut logits = Vec::with_capacity(markers.len());
        for &m in markers.iter() {
            let row = h0.i(m.min(n - 1))?.unsqueeze(0)?;
            let s = row
                .apply(&self.scorer_ln)?
                .apply(&self.scorer_l1)?
                .gelu_erf()?
                .apply(&self.scorer_l2)?;
            logits.push(s.flatten_all()?.to_vec1::<f32>()?[0]);
        }
        let k = markers.len();
        let mx = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let ex: Vec<f64> = logits.iter().map(|l| ((*l - mx) as f64).exp()).collect();
        let sum: f64 = ex.iter().sum::<f64>().max(1e-30);
        let p: Vec<f64> = ex.iter().map(|e| e / sum).collect();
        let kk = (k.max(2)) as f64;
        let ent = -p.iter().map(|x| x * x.max(1e-9).ln()).sum::<f64>() / kk.ln();
        let mut sorted = p.clone();
        sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let top1 = sorted.first().cloned().unwrap_or(0.0);
        let top2 = sorted.get(1).cloned().unwrap_or(0.0);
        let feats = Tensor::from_vec(
            vec![top1 as f32, (top1 - top2) as f32, ent as f32, (kk / 255.0) as f32],
            (1, 4),
            &self.device,
        )?;
        let pooled = h0.i(0)?.unsqueeze(0)?;
        let act_in = Tensor::cat(&[&pooled, &feats], D::Minus1)?;
        let act = act_in
            .apply(&self.act_l1)?
            .gelu_erf()?
            .apply(&self.act_l2)?
            .flatten_all()?
            .to_vec1::<f32>()?;
        Ok((logits, act))
    }

    fn encode(&self, text: &str, max: Option<usize>) -> std::result::Result<Vec<u32>, String> {
        let tok = self
            .tokenizer
            .as_ref()
            .ok_or_else(|| "laya tokenizer 가 없습니다".to_string())?;
        let clean = text.replace("[MASK]", " ");
        let enc = tok.encode(clean.as_str(), false).map_err(|e| e.to_string())?;
        let mut ids = enc.get_ids().to_vec();
        if let Some(m) = max {
            ids.truncate(m);
        }
        Ok(ids)
    }

    pub fn build_sequence(
        &self,
        qtype: usize,
        instruction: &str,
        options: &[String],
        state: &str,
    ) -> std::result::Result<(Vec<u32>, Vec<usize>, usize, usize), String> {
        let tname = match qtype {
            QTYPE_CHOICE => "choice",
            QTYPE_SCORE => "score",
            _ => "noul",
        };
        let mut head_ids = self.encode(&format!("{} question: {}", tname, instruction), None)?;
        let mut opt_ids: Vec<Vec<u32>> = Vec::with_capacity(options.len());
        for o in options.iter() {
            let mut v = vec![self.mask_id];
            v.extend(self.encode(&format!(" {}", o), Some(48))?);
            opt_ids.push(v);
        }
        let mut opt_budget = self.head_max_len as i64 - opt_ids.iter().map(|o| o.len() as i64).sum::<i64>();
        if opt_budget < 16 {
            let per = ((self.head_max_len as i64 - 16) / (opt_ids.len().max(1) as i64)).max(4) as usize;
            for o in opt_ids.iter_mut() {
                o.truncate(per);
            }
            opt_budget = self.head_max_len as i64 - opt_ids.iter().map(|o| o.len() as i64).sum::<i64>();
        }
        head_ids.truncate(opt_budget.max(8) as usize);
        let mut ids = vec![self.cls_id];
        ids.extend(head_ids);
        ids.push(self.sep_id);
        let mut markers = Vec::with_capacity(opt_ids.len());
        for o in opt_ids.iter() {
            markers.push(ids.len());
            ids.extend(o.iter().cloned());
        }
        ids.push(self.sep_id);
        let room = self.max_len.saturating_sub(ids.len() + 1);
        let state_ids = self.encode(state, None)?;
        let used = state_ids.len().min(room);
        ids.extend(state_ids[..used].iter().cloned());
        ids.push(self.sep_id);
        ids.truncate(self.max_len);
        markers.retain(|m| *m < self.max_len);
        Ok((ids, markers, used, state_ids.len() - used))
    }

    fn answer(&self, qtype: usize, labels: Vec<String>, options: &[String], instruction: &str, state: &str) -> std::result::Result<AdvisorAnswer, String> {
        let (ids, markers, used, dropped) = self.build_sequence(qtype, instruction, options, state)?;
        if markers.len() != options.len() {
            return Err("옵션 마커가 시퀀스 길이를 초과했습니다".into());
        }
        let (logits, act) = self.forward_ids(&ids, &markers, qtype).map_err(|e| e.to_string())?;
        let k = logits.len();
        let t = *self
            .temperature_by_options
            .get(&temp_bucket(qtype, k))
            .unwrap_or(&self.temperature[qtype.min(2)]);
        let z: Vec<f64> = logits.iter().map(|l| (*l / t) as f64).collect();
        let mx = z.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let ex: Vec<f64> = z.iter().map(|v| (v - mx).exp()).collect();
        let s: f64 = ex.iter().sum::<f64>().max(1e-30);
        let p: Vec<f32> = ex.iter().map(|e| (e / s) as f32).collect();
        let amx = act.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let aex: Vec<f32> = act.iter().map(|a| (a - amx).exp()).collect();
        let asum: f32 = aex.iter().sum::<f32>().max(1e-30);
        let (ti, tp) = p
            .iter()
            .enumerate()
            .fold((0usize, f32::MIN), |acc, (i, v)| if *v > acc.1 { (i, *v) } else { acc });
        Ok(AdvisorAnswer {
            qtype: match qtype {
                QTYPE_CHOICE => "choice".into(),
                QTYPE_SCORE => "score".into(),
                _ => "noul".into(),
            },
            top: labels.get(ti).cloned().unwrap_or_default(),
            top_probability: tp,
            labels,
            probabilities: p,
            act_probability: aex.first().cloned().unwrap_or(0.0) / asum,
            temperature: t,
            state_tokens_used: used,
            state_tokens_dropped: dropped,
        })
    }

    pub fn ask_choice(&self, instruction: &str, options: &[(String, String)], state: &str) -> std::result::Result<AdvisorAnswer, String> {
        let labels: Vec<String> = options.iter().map(|(l, _)| l.clone()).collect();
        let rendered: Vec<String> = options
            .iter()
            .map(|(l, d)| if d.is_empty() { l.clone() } else { format!("{}: {}", l, d) })
            .collect();
        self.answer(QTYPE_CHOICE, labels, &rendered, instruction, state)
    }

    pub fn ask_noul(&self, statement: &str, state: &str) -> std::result::Result<AdvisorAnswer, String> {
        let rendered = vec![
            "false: no, the statement does not hold".to_string(),
            "true: yes, the statement holds".to_string(),
        ];
        self.answer(QTYPE_NOUL, vec!["false".into(), "true".into()], &rendered, statement, state)
    }

    pub fn ask_score(&self, instruction: &str, levels: &[String], state: &str) -> std::result::Result<AdvisorAnswer, String> {
        let rendered: Vec<String> = levels
            .iter()
            .enumerate()
            .map(|(i, c)| format!("level {}: {}", i, c))
            .collect();
        let labels: Vec<String> = (0..levels.len()).map(|i| i.to_string()).collect();
        self.answer(QTYPE_SCORE, labels, &rendered, instruction, state)
    }

    pub fn has_tokenizer(&self) -> bool {
        self.tokenizer.is_some()
    }
}