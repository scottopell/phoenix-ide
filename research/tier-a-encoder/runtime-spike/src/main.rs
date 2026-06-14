//! Tier A inference-latency spike (task 29001, workstream W3).
//!
//! Builds a BERT-mini-shaped encoder stack in candle with RANDOM weights and
//! times a batch=1 forward pass. Outputs are meaningless; the shape
//! (layers/heads/hidden/vocab/seq-len) determines latency, which is all we
//! measure here. De-risks decision-points #1 (runtime/ANE) and #5 (latency
//! budget) before any model is trained.

use candle_core::{DType, Device, Result, Tensor, D};
use candle_nn::ops::softmax;
use candle_nn::{embedding, layer_norm, linear, Embedding, LayerNorm, Linear, Module, VarBuilder, VarMap};

// ---- BERT-mini reference shape (Notaro et al.) -----------------------------
const VOCAB: usize = 20_000;
const HIDDEN: usize = 256;
const INTERMEDIATE: usize = 1024;
const NUM_LAYERS: usize = 4;
const NUM_HEADS: usize = 4;
const HEAD_DIM: usize = HIDDEN / NUM_HEADS; // 64
const MAX_SEQ: usize = 64;
const NUM_CLASSES: usize = 3; // SAFE / RISKY / BLOCKED

// ---- Benchmark knobs -------------------------------------------------------
const WARMUP: usize = 20;
const ITERS: usize = 500;
const SEQ_LENS: [usize; 2] = [32, 64];

struct SelfAttention {
    query: Linear,
    key: Linear,
    value: Linear,
    out: Linear,
}

impl SelfAttention {
    fn new(vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            query: linear(HIDDEN, HIDDEN, vb.pp("query"))?,
            key: linear(HIDDEN, HIDDEN, vb.pp("key"))?,
            value: linear(HIDDEN, HIDDEN, vb.pp("value"))?,
            out: linear(HIDDEN, HIDDEN, vb.pp("out"))?,
        })
    }

    // x: (batch, seq, hidden) -> (batch, seq, hidden)
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let (b, seq, _) = x.dims3()?;

        let split_heads = |t: &Tensor| -> Result<Tensor> {
            // (b, seq, hidden) -> (b, heads, seq, head_dim)
            t.reshape((b, seq, NUM_HEADS, HEAD_DIM))?
                .transpose(1, 2)?
                .contiguous()
        };

        let q = split_heads(&self.query.forward(x)?)?;
        let k = split_heads(&self.key.forward(x)?)?;
        let v = split_heads(&self.value.forward(x)?)?;

        // scaled dot-product attention (no mask — padding ignored; latency probe)
        let scale = 1.0 / (HEAD_DIM as f64).sqrt();
        let scores = (q.matmul(&k.transpose(D::Minus1, D::Minus2)?)? * scale)?;
        let probs = softmax(&scores, D::Minus1)?;
        let ctx = probs.matmul(&v)?; // (b, heads, seq, head_dim)

        // merge heads -> (b, seq, hidden)
        let ctx = ctx
            .transpose(1, 2)?
            .contiguous()?
            .reshape((b, seq, HIDDEN))?;

        self.out.forward(&ctx)
    }
}

struct EncoderLayer {
    attn: SelfAttention,
    attn_norm: LayerNorm,
    ff1: Linear,
    ff2: Linear,
    ff_norm: LayerNorm,
}

impl EncoderLayer {
    fn new(vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            attn: SelfAttention::new(vb.pp("attn"))?,
            attn_norm: layer_norm(HIDDEN, 1e-12, vb.pp("attn_norm"))?,
            ff1: linear(HIDDEN, INTERMEDIATE, vb.pp("ff1"))?,
            ff2: linear(INTERMEDIATE, HIDDEN, vb.pp("ff2"))?,
            ff_norm: layer_norm(HIDDEN, 1e-12, vb.pp("ff_norm"))?,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        // post-LN transformer block with residuals
        let attn_out = self.attn.forward(x)?;
        let x = self.attn_norm.forward(&(x + attn_out)?)?;

        let ff = self.ff2.forward(&self.ff1.forward(&x)?.gelu()?)?;
        self.ff_norm.forward(&(&x + ff)?)
    }
}

struct BertMini {
    word_emb: Embedding,
    pos_emb: Embedding,
    emb_norm: LayerNorm,
    layers: Vec<EncoderLayer>,
    pooler: Linear,
    classifier: Linear,
    device: Device,
}

impl BertMini {
    fn new(vb: VarBuilder, device: Device) -> Result<Self> {
        let layers = (0..NUM_LAYERS)
            .map(|i| EncoderLayer::new(vb.pp(format!("layer.{i}"))))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            word_emb: embedding(VOCAB, HIDDEN, vb.pp("word_emb"))?,
            pos_emb: embedding(MAX_SEQ, HIDDEN, vb.pp("pos_emb"))?,
            emb_norm: layer_norm(HIDDEN, 1e-12, vb.pp("emb_norm"))?,
            layers,
            pooler: linear(HIDDEN, HIDDEN, vb.pp("pooler"))?,
            classifier: linear(HIDDEN, NUM_CLASSES, vb.pp("classifier"))?,
            device,
        })
    }

    // input_ids: (batch, seq) -> logits (batch, NUM_CLASSES)
    fn forward(&self, input_ids: &Tensor) -> Result<Tensor> {
        let (_b, seq) = input_ids.dims2()?;

        let words = self.word_emb.forward(input_ids)?;
        let pos_ids = Tensor::arange(0u32, seq as u32, &self.device)?;
        let pos = self.pos_emb.forward(&pos_ids)?.unsqueeze(0)?; // (1, seq, hidden) broadcasts
        let mut h = self.emb_norm.forward(&words.broadcast_add(&pos)?)?;

        for layer in &self.layers {
            h = layer.forward(&h)?;
        }

        // pool [CLS] (position 0), tanh activation, then 3-way classifier head
        let cls = h.narrow(1, 0, 1)?.squeeze(1)?; // (b, hidden)
        let pooled = self.pooler.forward(&cls)?.tanh()?;
        self.classifier.forward(&pooled)
    }
}

/// Trivial whitespace/byte tokenization: hash each whitespace token into the
/// vocab range, pad/truncate to `seq_len`. Tokenizer quality is irrelevant —
/// only the embedding-lookup cost matters.
fn tokenize(cmd: &str, seq_len: usize, device: &Device) -> Result<Tensor> {
    let mut ids: Vec<u32> = Vec::with_capacity(seq_len);
    ids.push(1); // reserved [CLS] id at position 0
    for tok in cmd.split_whitespace() {
        if ids.len() >= seq_len {
            break;
        }
        let h = tok.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(u32::from(b)));
        ids.push(2 + (h % (VOCAB as u32 - 2)));
    }
    ids.resize(seq_len, 0); // pad id = 0
    Tensor::from_vec(ids, (1, seq_len), device)
}

fn percentile(sorted_us: &[f64], p: f64) -> f64 {
    if sorted_us.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0) * ((sorted_us.len() - 1) as f64);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted_us[lo]
    } else {
        let frac = rank - lo as f64;
        sorted_us[lo] * (1.0 - frac) + sorted_us[hi] * frac
    }
}

fn pick_device() -> (Device, &'static str) {
    #[cfg(feature = "metal")]
    {
        match Device::new_metal(0) {
            Ok(d) => return (d, "Metal(GPU)"),
            Err(e) => eprintln!("metal requested but unavailable ({e}); falling back to CPU"),
        }
    }
    (Device::Cpu, "CPU")
}

fn gemm_backend() -> &'static str {
    if cfg!(feature = "accelerate") {
        "Accelerate(vecLib BLAS)"
    } else {
        "pure-Rust gemm"
    }
}

fn main() -> Result<()> {
    // For batch=1 the per-op work is tiny; candle's rayon-parallel CPU backend
    // spends more time on thread scheduling than compute and produces a noisy,
    // contention-bound tail. Single-threaded is both faster at p50 and far
    // tighter at p99 for this workload, and matches the realistic case: Tier A
    // classifies one command synchronously, off the hot path of other work.
    let _ = rayon::ThreadPoolBuilder::new().num_threads(1).build_global();

    let (device, device_name) = pick_device();

    let varmap = VarMap::new();
    let vb = VarBuilder::from_varmap(&varmap, DType::F32, &device);
    let model = BertMini::new(vb, device.clone())?;

    let cmd = "rm -rf /tmp/build && curl -fsSL https://example.com/install.sh | sudo bash";

    let param_count: usize = varmap
        .all_vars()
        .iter()
        .map(|v| v.elem_count())
        .sum();

    println!("=== Tier A runtime spike — BERT-mini forward-pass latency ===");
    println!(
        "shape: {NUM_LAYERS} layers, {NUM_HEADS} heads, hidden {HIDDEN}, ffn {INTERMEDIATE}, vocab {VOCAB}, classes {NUM_CLASSES}"
    );
    println!(
        "device: {device_name}   gemm: {}   threads: 1",
        gemm_backend()
    );
    println!("params: {param_count} (random init)   batch=1");
    println!("warmup: {WARMUP} iters   measured: {ITERS} iters\n");

    // Per-op floor diagnostic: time the single largest matmul in the stack
    // (the FFN expand, seq=64 x hidden=256 @ hidden=256 x intermediate=1024).
    // If this alone is multi-ms, candle's CPU op-dispatch/gemm is the floor,
    // not the model depth.
    {
        let a = Tensor::randn(0f32, 1f32, (1, 64, HIDDEN), &device)?;
        let w = Tensor::randn(0f32, 1f32, (HIDDEN, INTERMEDIATE), &device)?;
        for _ in 0..WARMUP {
            let _ = a.broadcast_matmul(&w)?.to_vec3::<f32>()?;
        }
        let mut s: Vec<f64> = Vec::with_capacity(200);
        for _ in 0..200 {
            let t0 = std::time::Instant::now();
            let _ = a.broadcast_matmul(&w)?.to_vec3::<f32>()?;
            s.push(t0.elapsed().as_secs_f64() * 1e6);
        }
        s.sort_by(|x, y| x.partial_cmp(y).unwrap());
        println!(
            "diagnostic: single (1x64x256 @ 256x1024) matmul p50 = {:.1} us\n",
            percentile(&s, 50.0)
        );
    }

    for &seq_len in &SEQ_LENS {
        let input_ids = tokenize(cmd, seq_len, &device)?;

        // sanity: shape check the very first forward
        let logits = model.forward(&input_ids)?;
        debug_assert_eq!(logits.dims2()?, (1, NUM_CLASSES));

        for _ in 0..WARMUP {
            let _ = model.forward(&input_ids)?;
        }

        let mut samples_us: Vec<f64> = Vec::with_capacity(ITERS);
        for _ in 0..ITERS {
            let t0 = std::time::Instant::now();
            let out = model.forward(&input_ids)?;
            // Force materialization so we time real compute, not lazy graph build.
            let _ = out.to_vec2::<f32>()?;
            samples_us.push(t0.elapsed().as_secs_f64() * 1e6);
        }

        samples_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = percentile(&samples_us, 50.0);
        let p90 = percentile(&samples_us, 90.0);
        let p99 = percentile(&samples_us, 99.0);
        let max = *samples_us.last().unwrap();
        let fmt = |us: f64| format!("{us:>9.1} us  ({:.3} ms)", us / 1000.0);

        println!("seq_len = {seq_len}:");
        println!("  p50 = {}", fmt(p50));
        println!("  p90 = {}", fmt(p90));
        println!("  p99 = {}", fmt(p99));
        println!("  max = {}", fmt(max));
        println!();
    }

    Ok(())
}
