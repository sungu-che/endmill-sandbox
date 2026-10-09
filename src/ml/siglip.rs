use candle_core::{Device, IndexOp, Module, Result, Tensor};
use candle_nn::{layer_norm, linear, Activation, LayerNorm, Linear, VarBuilder};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

fn d_vocab() -> usize {
    32000
}
fn d_hidden() -> usize {
    768
}
fn d_inter() -> usize {
    3072
}
fn d_layers() -> usize {
    12
}
fn d_heads() -> usize {
    12
}
fn d_maxpos() -> usize {
    64
}
fn d_act() -> Activation {
    Activation::GeluPytorchTanh
}
fn d_eps() -> f64 {
    1e-6
}
fn d_image() -> usize {
    224
}
fn d_patch() -> usize {
    16
}
fn d_channels() -> usize {
    3
}

#[derive(Debug, Clone, Deserialize)]
pub struct SiglipTextConfig {
    #[serde(default = "d_vocab")]
    pub vocab_size: usize,
    #[serde(default = "d_hidden")]
    pub hidden_size: usize,
    #[serde(default = "d_inter")]
    pub intermediate_size: usize,
    #[serde(default = "d_layers")]
    pub num_hidden_layers: usize,
    #[serde(default = "d_heads")]
    pub num_attention_heads: usize,
    #[serde(default = "d_maxpos")]
    pub max_position_embeddings: usize,
    #[serde(default = "d_act")]
    pub hidden_act: Activation,
    #[serde(default = "d_eps")]
    pub layer_norm_eps: f64,
    #[serde(default)]
    pub projection_size: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SiglipVisionConfig {
    #[serde(default = "d_hidden")]
    pub hidden_size: usize,
    #[serde(default = "d_inter")]
    pub intermediate_size: usize,
    #[serde(default = "d_layers")]
    pub num_hidden_layers: usize,
    #[serde(default = "d_heads")]
    pub num_attention_heads: usize,
    #[serde(default = "d_channels")]
    pub num_channels: usize,
    #[serde(default = "d_image")]
    pub image_size: usize,
    #[serde(default = "d_patch")]
    pub patch_size: usize,
    #[serde(default = "d_act")]
    pub hidden_act: Activation,
    #[serde(default = "d_eps")]
    pub layer_norm_eps: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SiglipConfig {
    pub text_config: SiglipTextConfig,
    pub vision_config: SiglipVisionConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SiglipPreprocessConfig {
    #[serde(default)]
    pub image_mean: Option<Vec<f32>>,
    #[serde(default)]
    pub image_std: Option<Vec<f32>>,
    #[serde(default)]
    pub rescale_factor: Option<f32>,
}

#[derive(Debug, Clone)]
struct Attention {
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
    heads: usize,
    head_dim: usize,
}

impl Attention {
    fn new(hidden: usize, heads: usize, vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            q: linear(hidden, hidden, vb.pp("q_proj"))?,
            k: linear(hidden, hidden, vb.pp("k_proj"))?,
            v: linear(hidden, hidden, vb.pp("v_proj"))?,
            o: linear(hidden, hidden, vb.pp("out_proj"))?,
            heads,
            head_dim: hidden / heads,
        })
    }

    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let (b, n, _) = xs.dims3()?;
        let shape = (b, n, self.heads, self.head_dim);
        let q = xs.apply(&self.q)?.reshape(shape)?.transpose(1, 2)?.contiguous()?;
        let k = xs.apply(&self.k)?.reshape(shape)?.transpose(1, 2)?.contiguous()?;
        let v = xs.apply(&self.v)?.reshape(shape)?.transpose(1, 2)?.contiguous()?;
        let scale = (self.head_dim as f64).powf(-0.5);
        let att = (q.matmul(&k.t()?)? * scale)?;
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        att.matmul(&v)?
            .transpose(1, 2)?
            .reshape((b, n, self.heads * self.head_dim))?
            .apply(&self.o)
    }
}

#[derive(Debug, Clone)]
struct Mlp {
    fc1: Linear,
    fc2: Linear,
    act: Activation,
}

impl Mlp {
    fn new(hidden: usize, inter: usize, act: Activation, vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            fc1: linear(hidden, inter, vb.pp("fc1"))?,
            fc2: linear(inter, hidden, vb.pp("fc2"))?,
            act,
        })
    }
}

impl Module for Mlp {
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        xs.apply(&self.fc1)?.apply(&self.act)?.apply(&self.fc2)
    }
}

#[derive(Debug, Clone)]
struct EncoderLayer {
    attn: Attention,
    ln1: LayerNorm,
    mlp: Mlp,
    ln2: LayerNorm,
}

impl EncoderLayer {
    fn new(hidden: usize, inter: usize, heads: usize, eps: f64, act: Activation, vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            attn: Attention::new(hidden, heads, vb.pp("self_attn"))?,
            ln1: layer_norm(hidden, eps, vb.pp("layer_norm1"))?,
            mlp: Mlp::new(hidden, inter, act, vb.pp("mlp"))?,
            ln2: layer_norm(hidden, eps, vb.pp("layer_norm2"))?,
        })
    }

    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let h = (xs + self.attn.forward(&xs.apply(&self.ln1)?)?)?;
        let m = h.apply(&self.ln2)?.apply(&self.mlp)?;
        h + m
    }
}

#[derive(Debug, Clone)]
struct Encoder {
    layers: Vec<EncoderLayer>,
}

impl Encoder {
    fn new(n: usize, hidden: usize, inter: usize, heads: usize, eps: f64, act: Activation, vb: VarBuilder) -> Result<Self> {
        let vb = vb.pp("layers");
        let mut layers = Vec::with_capacity(n);
        for i in 0..n {
            layers.push(EncoderLayer::new(hidden, inter, heads, eps, act, vb.pp(i))?);
        }
        Ok(Self { layers })
    }

    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let mut h = xs.clone();
        for l in self.layers.iter() {
            h = l.forward(&h)?;
        }
        Ok(h)
    }
}

#[derive(Debug, Clone)]
struct MapHead {
    probe: Tensor,
    q: Linear,
    k: Linear,
    v: Linear,
    o: Linear,
    heads: usize,
    ln: LayerNorm,
    mlp: Mlp,
}

impl MapHead {
    fn new(cfg: &SiglipVisionConfig, vb: VarBuilder) -> Result<Self> {
        let h = cfg.hidden_size;
        let w = vb.get((3 * h, h), "attention.in_proj_weight")?.chunk(3, 0)?;
        let b = vb.get(3 * h, "attention.in_proj_bias")?.chunk(3, 0)?;
        Ok(Self {
            probe: vb.get((1, 1, h), "probe")?,
            q: Linear::new(w[0].clone(), Some(b[0].clone())),
            k: Linear::new(w[1].clone(), Some(b[1].clone())),
            v: Linear::new(w[2].clone(), Some(b[2].clone())),
            o: linear(h, h, vb.pp("attention.out_proj"))?,
            heads: cfg.num_attention_heads,
            ln: layer_norm(h, cfg.layer_norm_eps, vb.pp("layernorm"))?,
            mlp: Mlp::new(h, cfg.intermediate_size, cfg.hidden_act, vb.pp("mlp"))?,
        })
    }

    fn forward(&self, xs: &Tensor) -> Result<(Tensor, Tensor)> {
        let (b, n, c) = xs.dims3()?;
        let hd = c / self.heads;
        let probe = self.probe.repeat((b, 1, 1))?;
        let q = probe.apply(&self.q)?.reshape((b, 1, self.heads, hd))?.transpose(1, 2)?.contiguous()?;
        let k = xs.apply(&self.k)?.reshape((b, n, self.heads, hd))?.transpose(1, 2)?.contiguous()?;
        let v = xs.apply(&self.v)?.reshape((b, n, self.heads, hd))?.transpose(1, 2)?.contiguous()?;
        let att = (q.matmul(&k.t()?)? / (hd as f64).sqrt())?;
        let att = candle_nn::ops::softmax_last_dim(&att)?;
        let pooled = att
            .matmul(&v)?
            .transpose(1, 2)?
            .reshape((b, 1, c))?
            .apply(&self.o)?;
        let pooled = (&pooled + pooled.apply(&self.ln)?.apply(&self.mlp)?)?.i((.., 0))?;
        let att_mean = att.mean(1)?.i((.., 0))?;
        Ok((pooled, att_mean))
    }

    fn dense(&self, xs: &Tensor) -> Result<Tensor> {
        let v = xs.apply(&self.v)?.apply(&self.o)?;
        &v + v.apply(&self.ln)?.apply(&self.mlp)?
    }
}

#[derive(Debug, Clone)]
pub struct SiglipVision {
    patch_embed: candle_nn::Conv2d,
    pos: Tensor,
    encoder: Encoder,
    post_ln: LayerNorm,
    head: MapHead,
    pub patch_size: usize,
    pub grid: usize,
    pub image_size: usize,
    pub hidden: usize,
}

#[derive(Debug, Clone)]
pub struct VisionOutput {
    pub pooled: Vec<f32>,
    pub patches: Vec<Vec<f32>>,
    pub dense: Vec<Vec<f32>>,
    pub attention: Vec<f32>,
    pub grid: usize,
}

impl SiglipVision {
    fn new(cfg: &SiglipVisionConfig, vb: VarBuilder) -> Result<Self> {
        let conv_cfg = candle_nn::Conv2dConfig {
            stride: cfg.patch_size,
            ..Default::default()
        };
        let patch_embed = candle_nn::conv2d(
            cfg.num_channels,
            cfg.hidden_size,
            cfg.patch_size,
            conv_cfg,
            vb.pp("embeddings.patch_embedding"),
        )?;
        let grid = cfg.image_size / cfg.patch_size;
        let pos = vb
            .get((grid * grid, cfg.hidden_size), "embeddings.position_embedding.weight")?
            .reshape((1, grid, grid, cfg.hidden_size))?
            .permute((0, 3, 1, 2))?
            .contiguous()?;
        Ok(Self {
            patch_embed,
            pos,
            encoder: Encoder::new(
                cfg.num_hidden_layers,
                cfg.hidden_size,
                cfg.intermediate_size,
                cfg.num_attention_heads,
                cfg.layer_norm_eps,
                cfg.hidden_act,
                vb.pp("encoder"),
            )?,
            post_ln: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("post_layernorm"))?,
            head: MapHead::new(cfg, vb.pp("head"))?,
            patch_size: cfg.patch_size,
            grid,
            image_size: cfg.image_size,
            hidden: cfg.hidden_size,
        })
    }

    pub fn forward_tensor(&self, pixel: &Tensor) -> Result<(Tensor, Tensor, Tensor)> {
        let (_, _, h, w) = pixel.dims4()?;
        let x = pixel.apply(&self.patch_embed)?;
        let gh = h / self.patch_size;
        let gw = w / self.patch_size;
        let pos = if gh == self.grid && gw == self.grid {
            self.pos.clone()
        } else {
            self.pos.interpolate2d(gh, gw)?
        };
        let x = x.broadcast_add(&pos)?.flatten_from(2)?.transpose(1, 2)?;
        let x = self.encoder.forward(&x)?.apply(&self.post_ln)?;
        let (pooled, att) = self.head.forward(&x)?;
        Ok((x, pooled, att))
    }

    pub fn forward(&self, pixel: &Tensor, with_dense: bool) -> Result<VisionOutput> {
        let (patches, pooled, att) = self.forward_tensor(pixel)?;
        let dense = if with_dense {
            self.head.dense(&patches)?.i(0)?.to_vec2::<f32>()?
        } else {
            Vec::new()
        };
        Ok(VisionOutput {
            pooled: pooled.i(0)?.to_vec1::<f32>()?,
            patches: patches.i(0)?.to_vec2::<f32>()?,
            dense,
            attention: att.i(0)?.to_vec1::<f32>()?,
            grid: self.grid,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SiglipText {
    tok: candle_nn::Embedding,
    pos: candle_nn::Embedding,
    encoder: Encoder,
    final_ln: LayerNorm,
    head: Linear,
    pub max_len: usize,
}

impl SiglipText {
    fn new(cfg: &SiglipTextConfig, vb: VarBuilder) -> Result<Self> {
        let proj = cfg.projection_size.unwrap_or(cfg.hidden_size);
        Ok(Self {
            tok: candle_nn::embedding(cfg.vocab_size, cfg.hidden_size, vb.pp("embeddings.token_embedding"))?,
            pos: candle_nn::embedding(cfg.max_position_embeddings, cfg.hidden_size, vb.pp("embeddings.position_embedding"))?,
            encoder: Encoder::new(
                cfg.num_hidden_layers,
                cfg.hidden_size,
                cfg.intermediate_size,
                cfg.num_attention_heads,
                cfg.layer_norm_eps,
                cfg.hidden_act,
                vb.pp("encoder"),
            )?,
            final_ln: layer_norm(cfg.hidden_size, cfg.layer_norm_eps, vb.pp("final_layer_norm"))?,
            head: linear(cfg.hidden_size, proj, vb.pp("head"))?,
            max_len: cfg.max_position_embeddings,
        })
    }

    pub fn forward(&self, ids: &Tensor) -> Result<Tensor> {
        let (_, n) = ids.dims2()?;
        let pos_ids = Tensor::arange(0u32, n as u32, ids.device())?;
        let x = ids.apply(&self.tok)?.broadcast_add(&pos_ids.apply(&self.pos)?)?;
        let x = self.encoder.forward(&x)?.apply(&self.final_ln)?;
        x.i((.., n - 1, ..))?.contiguous()?.apply(&self.head)
    }
}

pub struct SiglipModel {
    pub vision: SiglipVision,
    pub text: SiglipText,
    pub logit_scale: f32,
    pub logit_bias: f32,
    pub tokenizer: Option<Tokenizer>,
    pub pad_id: u32,
    pub eos_id: u32,
    pub mean: [f32; 3],
    pub std: [f32; 3],
    pub rescale: f32,
    pub dir: PathBuf,
    device: Device,
}

impl SiglipModel {
    pub fn load_from_varbuilder(cfg: &SiglipConfig, vb: VarBuilder, device: &Device) -> Result<(SiglipVision, SiglipText, f32, f32)> {
        let vision = SiglipVision::new(&cfg.vision_config, vb.pp("vision_model"))?;
        let text = SiglipText::new(&cfg.text_config, vb.pp("text_model"))?;
        let scale = vb.get(1, "logit_scale")?.to_device(device)?.to_vec1::<f32>()?[0];
        let bias = vb.get(1, "logit_bias")?.to_device(device)?.to_vec1::<f32>()?[0];
        Ok((vision, text, scale, bias))
    }

    pub fn load(dir: &Path, device: &Device) -> std::result::Result<Self, String> {
        let cfg_v = super::read_json(&dir.join("config.json"))?;
        let cfg: SiglipConfig = serde_json::from_value(cfg_v).map_err(|e| format!("SigLIP config: {}", e))?;
        let vb = super::load_varbuilder(dir, device)?;
        let (vision, text, logit_scale, logit_bias) =
            Self::load_from_varbuilder(&cfg, vb, device).map_err(|e| format!("SigLIP 가중치 로드 실패: {}", e))?;
        let tokenizer = {
            let p = dir.join("tokenizer.json");
            if p.exists() {
                Some(Tokenizer::from_file(&p).map_err(|e| format!("tokenizer.json: {}", e))?)
            } else {
                None
            }
        };
        let special = super::read_json(&dir.join("special_tokens_map.json")).unwrap_or(serde_json::Value::Null);
        let tok_name = |key: &str, fallback: &str| -> String {
            match special.get(key) {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Object(o)) => o
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or(fallback)
                    .to_string(),
                _ => fallback.to_string(),
            }
        };
        let (pad_id, eos_id) = match tokenizer.as_ref() {
            Some(t) => (
                t.token_to_id(&tok_name("pad_token", "<pad>")).unwrap_or(0),
                t.token_to_id(&tok_name("eos_token", "<eos>")).unwrap_or(1),
            ),
            None => (0, 1),
        };
        let pre: SiglipPreprocessConfig = super::read_json(&dir.join("preprocessor_config.json"))
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let mean = pre.image_mean.unwrap_or_else(|| vec![0.5, 0.5, 0.5]);
        let std = pre.image_std.unwrap_or_else(|| vec![0.5, 0.5, 0.5]);
        Ok(Self {
            vision,
            text,
            logit_scale,
            logit_bias,
            tokenizer,
            pad_id,
            eos_id,
            mean: [mean[0], mean[1 % mean.len()], mean[2 % mean.len()]],
            std: [std[0], std[1 % std.len()], std[2 % std.len()]],
            rescale: pre.rescale_factor.unwrap_or(1.0 / 255.0),
            dir: dir.to_path_buf(),
            device: device.clone(),
        })
    }

    pub fn image_size(&self) -> usize {
        self.vision.image_size
    }

    pub fn pixel_tensor(&self, rgb: &[u8], size: usize) -> std::result::Result<Tensor, String> {
        if rgb.len() != size * size * 3 {
            return Err(format!("픽셀 버퍼 크기 불일치: {} != {}", rgb.len(), size * size * 3));
        }
        let mut data = vec![0f32; 3 * size * size];
        for y in 0..size {
            for x in 0..size {
                for c in 0..3 {
                    let v = rgb[(y * size + x) * 3 + c] as f32 * self.rescale;
                    data[c * size * size + y * size + x] = (v - self.mean[c]) / self.std[c];
                }
            }
        }
        Tensor::from_vec(data, (1, 3, size, size), &self.device).map_err(|e| e.to_string())
    }

    pub fn embed_image(&self, rgb: &[u8], with_dense: bool) -> std::result::Result<VisionOutput, String> {
        let size = self.image_size();
        let t = self.pixel_tensor(rgb, size)?;
        let out = self.vision.forward(&t, with_dense).map_err(|e| e.to_string());
        crate::ml::trim_idle_gpu_pool(&self.device);
        out
    }

    pub fn tokenize(&self, text: &str) -> std::result::Result<Vec<u32>, String> {
        let tok = self
            .tokenizer
            .as_ref()
            .ok_or_else(|| "SigLIP tokenizer.json 이 없습니다".to_string())?;
        let lower = text.to_lowercase();
        let enc = tok.encode(lower.as_str(), false).map_err(|e| e.to_string())?;
        let mut ids: Vec<u32> = enc.get_ids().to_vec();
        let max_len = self.text.max_len;
        if ids.len() > max_len - 1 {
            ids.truncate(max_len - 1);
        }
        ids.push(self.eos_id);
        while ids.len() < max_len {
            ids.push(self.pad_id);
        }
        Ok(ids)
    }

    pub fn embed_texts(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String> {
        let mut rows: Vec<u32> = Vec::new();
        for t in texts.iter() {
            rows.extend(self.tokenize(t)?);
        }
        let n = texts.len();
        let ids = Tensor::from_vec(rows, (n, self.text.max_len), &self.device).map_err(|e| e.to_string())?;
        let out = self
            .text
            .forward(&ids)
            .and_then(|t| t.to_vec2::<f32>())
            .map_err(|e| e.to_string());
        crate::ml::trim_idle_gpu_pool(&self.device);
        out
    }

    pub fn embed_text_ids(&self, ids: &[Vec<u32>]) -> std::result::Result<Vec<Vec<f32>>, String> {
        let n = ids.len();
        if n == 0 {
            return Ok(Vec::new());
        }
        let len = ids[0].len();
        let flat: Vec<u32> = ids.iter().flat_map(|r| r.iter().cloned()).collect();
        let t = Tensor::from_vec(flat, (n, len), &self.device).map_err(|e| e.to_string())?;
        let out = self
            .text
            .forward(&t)
            .and_then(|t| t.to_vec2::<f32>())
            .map_err(|e| e.to_string());
        crate::ml::trim_idle_gpu_pool(&self.device);
        out
    }

    pub fn probability(&self, image: &[f32], text: &[f32]) -> f32 {
        let c = super::cosine(image, text);
        let z = self.logit_scale.exp() * c + self.logit_bias;
        1.0 / (1.0 + (-z).exp())
    }

    pub fn logit(&self, image: &[f32], text: &[f32]) -> f32 {
        let c = super::cosine(image, text);
        self.logit_scale.exp() * c + self.logit_bias
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn hidden(&self) -> usize {
        self.vision.hidden
    }

    pub fn patch_count(&self) -> usize {
        self.vision.grid * self.vision.grid
    }

    pub fn grid(&self) -> usize {
        self.vision.grid
    }

    pub fn max_text_len(&self) -> usize {
        self.text.max_len
    }
}