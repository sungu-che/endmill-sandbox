use candle_core::{DType, Device, Module, Result, Tensor, D};
use candle_nn::{embedding, Embedding, LayerNorm, Linear, VarBuilder};

#[derive(Debug, Clone)]
pub struct ModernBertConfig {
    pub vocab_size: usize,
    pub hidden_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,
    pub intermediate_size: usize,
    pub norm_eps: f64,
    pub norm_bias: bool,
    pub attention_bias: bool,
    pub mlp_bias: bool,
    pub local_attention: usize,
    pub layer_is_global: Vec<bool>,
    pub global_rope_theta: f64,
    pub local_rope_theta: f64,
    pub pad_token_id: u32,
}

impl ModernBertConfig {
    pub fn from_json(v: &serde_json::Value) -> std::result::Result<Self, String> {
        let get_usize = |k: &str| -> std::result::Result<usize, String> {
            v.get(k)
                .and_then(|x| x.as_u64())
                .map(|x| x as usize)
                .ok_or_else(|| format!("ModernBERT config 에 '{}' 가 없습니다", k))
        };
        let hidden_size = get_usize("hidden_size")?;
        let num_hidden_layers = get_usize("num_hidden_layers")?;
        let num_attention_heads = get_usize("num_attention_heads")?;
        let intermediate_size = get_usize("intermediate_size")?;
        let vocab_size = get_usize("vocab_size")?;
        let norm_eps = v
            .get("norm_eps")
            .or_else(|| v.get("layer_norm_eps"))
            .and_then(|x| x.as_f64())
            .unwrap_or(1e-5);
        let flag = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        let local_attention = v.get("local_attention").and_then(|x| x.as_u64()).unwrap_or(128) as usize;
        let every = v
            .get("global_attn_every_n_layers")
            .and_then(|x| x.as_u64())
            .unwrap_or(3)
            .max(1) as usize;
        let layer_is_global: Vec<bool> = match v.get("layer_types").and_then(|x| x.as_array()) {
            Some(arr) if arr.len() == num_hidden_layers => arr
                .iter()
                .map(|t| t.as_str().map(|s| s != "sliding_attention").unwrap_or(true))
                .collect(),
            _ => (0..num_hidden_layers).map(|i| i % every == 0).collect(),
        };
        let rope = v.get("rope_parameters");
        let theta_of = |kind: &str, flat_key: &str, default: f64| -> f64 {
            if let Some(r) = rope {
                if let Some(t) = r.get(kind).and_then(|x| x.get("rope_theta")).and_then(|x| x.as_f64()) {
                    return t;
                }
                if let Some(t) = r.get("rope_theta").and_then(|x| x.as_f64()) {
                    return t;
                }
            }
            v.get(flat_key).and_then(|x| x.as_f64()).unwrap_or(default)
        };
        Ok(Self {
            vocab_size,
            hidden_size,
            num_hidden_layers,
            num_attention_heads,
            intermediate_size,
            norm_eps,
            norm_bias: flag("norm_bias"),
            attention_bias: flag("attention_bias"),
            mlp_bias: flag("mlp_bias"),
            local_attention,
            layer_is_global,
            global_rope_theta: theta_of("full_attention", "global_rope_theta", 160000.0),
            local_rope_theta: theta_of("sliding_attention", "local_rope_theta", 10000.0),
            pad_token_id: v.get("pad_token_id").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        })
    }
}

fn ln(size: usize, eps: f64, bias: bool, vb: VarBuilder) -> Result<LayerNorm> {
    if bias {
        candle_nn::layer_norm(size, eps, vb)
    } else {
        candle_nn::layer_norm_no_bias(size, eps, vb)
    }
}

fn lin(i: usize, o: usize, bias: bool, vb: VarBuilder) -> Result<Linear> {
    if bias {
        candle_nn::linear(i, o, vb)
    } else {
        candle_nn::linear_no_bias(i, o, vb)
    }
}

struct Layer {
    attn_norm: Option<LayerNorm>,
    wqkv: Linear,
    wo: Linear,
    mlp_norm: LayerNorm,
    wi: Linear,
    mlp_wo: Linear,
    global: bool,
}

pub struct ModernBert {
    tok: Embedding,
    emb_norm: LayerNorm,
    layers: Vec<Layer>,
    final_norm: LayerNorm,
    cfg: ModernBertConfig,
    device: Device,
}

impl ModernBert {
    pub fn load(cfg: &ModernBertConfig, vb: VarBuilder) -> Result<Self> {
        let h = cfg.hidden_size;
        let mut layers = Vec::with_capacity(cfg.num_hidden_layers);
        for i in 0..cfg.num_hidden_layers {
            let lv = vb.pp(format!("layers.{}", i));
            let attn_norm = if lv.contains_tensor("attn_norm.weight") {
                Some(ln(h, cfg.norm_eps, cfg.norm_bias, lv.pp("attn_norm"))?)
            } else {
                None
            };
            layers.push(Layer {
                attn_norm,
                wqkv: lin(h, 3 * h, cfg.attention_bias, lv.pp("attn.Wqkv"))?,
                wo: lin(h, h, cfg.attention_bias, lv.pp("attn.Wo"))?,
                mlp_norm: ln(h, cfg.norm_eps, cfg.norm_bias, lv.pp("mlp_norm"))?,
                wi: lin(h, 2 * cfg.intermediate_size, cfg.mlp_bias, lv.pp("mlp.Wi"))?,
                mlp_wo: lin(cfg.intermediate_size, h, cfg.mlp_bias, lv.pp("mlp.Wo"))?,
                global: cfg.layer_is_global.get(i).cloned().unwrap_or(i % 3 == 0),
            });
        }
        Ok(Self {
            tok: embedding(cfg.vocab_size, h, vb.pp("embeddings.tok_embeddings"))?,
            emb_norm: ln(h, cfg.norm_eps, cfg.norm_bias, vb.pp("embeddings.norm"))?,
            layers,
            final_norm: ln(h, cfg.norm_eps, cfg.norm_bias, vb.pp("final_norm"))?,
            cfg: cfg.clone(),
            device: vb.device().clone(),
        })
    }

    fn rope_tables(&self, seq_len: usize, theta: f64) -> Result<(Tensor, Tensor)> {
        let dim = self.cfg.hidden_size / self.cfg.num_attention_heads;
        let half = dim / 2;
        let mut cos = vec![0f32; seq_len * half];
        let mut sin = vec![0f32; seq_len * half];
        for p in 0..seq_len {
            for i in 0..half {
                let inv = 1.0 / theta.powf((2 * i) as f64 / dim as f64);
                let a = p as f64 * inv;
                cos[p * half + i] = a.cos() as f32;
                sin[p * half + i] = a.sin() as f32;
            }
        }
        Ok((
            Tensor::from_vec(cos, (seq_len, half), &self.device)?,
            Tensor::from_vec(sin, (seq_len, half), &self.device)?,
        ))
    }

    pub fn forward(&self, ids: &Tensor, attention_mask: Option<&Tensor>) -> Result<Tensor> {
        let (b, n) = ids.dims2()?;
        let h = self.cfg.hidden_size;
        let heads = self.cfg.num_attention_heads;
        let hd = h / heads;
        let pad_mask = match attention_mask {
            Some(m) => {
                let m = m.to_dtype(DType::F32)?;
                let inv = (1.0 - m)?;
                Some((inv * -1e30f64)?.reshape((b, 1, 1, n))?)
            }
            None => None,
        };
        let half = self.cfg.local_attention / 2;
        let mut local = vec![0f32; n * n];
        for i in 0..n {
            for j in 0..n {
                if (i as i64 - j as i64).unsigned_abs() as usize > half {
                    local[i * n + j] = f32::NEG_INFINITY;
                }
            }
        }
        let local = Tensor::from_vec(local, (1, 1, n, n), &self.device)?;
        let (gcos, gsin) = self.rope_tables(n, self.cfg.global_rope_theta)?;
        let (lcos, lsin) = self.rope_tables(n, self.cfg.local_rope_theta)?;
        let mut x = ids.apply(&self.tok)?.apply(&self.emb_norm)?;
        for layer in self.layers.iter() {
            let res = x.clone();
            let y = match &layer.attn_norm {
                Some(norm) => x.apply(norm)?,
                None => x.clone(),
            };
            let qkv = y
                .apply(&layer.wqkv)?
                .reshape((b, n, 3, heads, hd))?
                .permute((2, 0, 3, 1, 4))?;
            let q = qkv.get(0)?.contiguous()?;
            let k = qkv.get(1)?.contiguous()?;
            let v = qkv.get(2)?.contiguous()?;
            let (cos, sin) = if layer.global { (&gcos, &gsin) } else { (&lcos, &lsin) };
            let q = candle_nn::rotary_emb::rope(&q, cos, sin)?;
            let k = candle_nn::rotary_emb::rope(&k, cos, sin)?;
            let mut att = (q.matmul(&k.transpose(D::Minus2, D::Minus1)?)? * (hd as f64).powf(-0.5))?;
            if let Some(pm) = pad_mask.as_ref() {
                att = att.broadcast_add(pm)?;
            }
            if !layer.global {
                att = att.broadcast_add(&local)?;
            }
            let att = candle_nn::ops::softmax_last_dim(&att)?;
            let o = att
                .matmul(&v)?
                .transpose(1, 2)?
                .reshape((b, n, h))?
                .apply(&layer.wo)?;
            x = (res + o)?;
            let m = x.apply(&layer.mlp_norm)?.apply(&layer.wi)?;
            let parts = m.chunk(2, D::Minus1)?;
            let m = (parts[0].gelu_erf()? * &parts[1])?.apply(&layer.mlp_wo)?;
            x = (x + m)?;
        }
        x.apply(&self.final_norm)
    }

    pub fn hidden_size(&self) -> usize {
        self.cfg.hidden_size
    }

    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl Module for ModernBert {
    fn forward(&self, ids: &Tensor) -> Result<Tensor> {
        ModernBert::forward(self, ids, None)
    }
}