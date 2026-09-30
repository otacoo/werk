//! Memory estimation + launch suggestions: weights + KV across VRAM/RAM,
//! plus a tiered fit search for GPU offload and cache settings.

use serde::Serialize;

use crate::models::{hybrid_kv_arch_divisor, ModelMetadata};

/// Bytes per KV element by cache type; sub-byte quants count as 1.
fn cache_bytes_per_element(cache_type: &str) -> u64 {
    match cache_type.to_lowercase().as_str() {
        "f32" => 4,
        "q8_0" | "q4_0" | "q4_1" | "iq4_nl" | "q5_0" | "q5_1" => 1,
        _ => 2, // f16, bf16, and anything unknown
    }
}

pub fn kv_bytes_per_token(layers: u64, kv_embd: u64, cache_type_k: &str, cache_type_v: &str) -> u64 {
    layers
        .saturating_mul(kv_embd)
        .saturating_mul(cache_bytes_per_element(cache_type_k) + cache_bytes_per_element(cache_type_v))
}

/// KV cache in MB; `--ctx-size` is shared across slots, so this ignores `--parallel`.
fn kv_cache_mb(layers: u64, kv_embd: u64, ctx: u64, cache_type_k: &str, cache_type_v: &str) -> u64 {
    kv_bytes_per_token(layers, kv_embd, cache_type_k, cache_type_v)
        .saturating_mul(ctx)
        / (1024 * 1024)
}

/// Estimated memory breakdown for a model + settings on the current machine.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct MemoryEstimate {
    pub model_mb: u32,
    pub kv_cache_mb: u32,
    pub overhead_mb: u32,
    pub total_mb: u32,
    pub vram_total_mb: u32,
    pub ram_available_mb: u32,
    pub vram_used_mb: u32,
    pub ram_used_mb: u32,
    pub vram_model_mb: u32,
    pub vram_kv_mb: u32,
    pub vram_overhead_mb: u32,
    pub ram_model_mb: u32,
    pub ram_kv_mb: u32,
    pub ram_overhead_mb: u32,
    pub fits: bool,
    /// Fits the raw total but leaves nothing for CUDA/OS headroom.
    pub tight: bool,
    pub notes: Vec<EstimateNote>,
}

/// One estimate note; warnings highlight, info stays quiet.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct EstimateNote {
    pub text: String,
    pub warn: bool,
}

impl EstimateNote {
    fn info(text: impl Into<String>) -> Self {
        Self { text: text.into(), warn: false }
    }
    fn warn(text: impl Into<String>) -> Self {
        Self { text: text.into(), warn: true }
    }
}

/// MB as u32; values beyond 4 PB saturate (JSON-exact for the UI).
fn mb(v: u64) -> u32 {
    v.min(u32::MAX as u64) as u32
}

/// Estimate memory footprint; layer info comes from the GGUF header with fallbacks.
pub fn estimate_memory(
    meta: &ModelMetadata,
    model_size_mb: u64,
    n_ctx: u32,
    cache_type_k: &str,
    cache_type_v: &str,
    n_gpu_layers: i32,
    fit: bool,
    vram_total_mb: u64,
    ram_available_mb: u64,
) -> MemoryEstimate {
    let mut notes = Vec::new();
    let layers = meta.block_count.unwrap_or(32);
    let embd = meta.embedding_length.unwrap_or(4096);
    let model_ctx = meta.context_length;

    // KV per token per layer = kv_heads × head_dim (head_dim from key_length,
    // else embd/heads). The embd×GQA approximation breaks models whose
    // head_dim ≠ embd/heads (e.g. hybrid SSM architectures).
    let heads = meta.attention_head_count;
    let kv_heads = meta.attention_head_count_kv.or(heads).unwrap_or(8);
    let head_dim = meta.attention_key_length.unwrap_or_else(|| {
        heads
            .filter(|h| *h > 0)
            .map(|h| (embd / h).max(1))
            .unwrap_or(128)
    });
    let kv_embd = (kv_heads * head_dim).max(1);

    // Hybrid SSM/linear-attention archs keep KV in only a fraction of their
    // blocks; estimating every block massively overstates the cache.
    let kv_layers = match meta.architecture.as_deref().and_then(hybrid_kv_arch_divisor) {
        Some((divisor, note)) => {
            if let Some(note) = note {
                notes.push(EstimateNote::info(note));
            }
            if divisor.is_infinite() {
                0
            } else {
                (layers as f64 / divisor).max(1.0) as u64
            }
        }
        None => layers,
    };

    // Effective context: 0 means "use model default".
    let effective_ctx = if n_ctx > 0 {
        n_ctx as u64
    } else {
        model_ctx.unwrap_or(4096)
    };
    if n_ctx == 0 {
        notes.push(EstimateNote::info(format!(
            "Context: model default ({effective_ctx})"
        )));
    }
    // Neutral guidance, not a warning: the agent needs room beyond history.
    const AGENT_CTX_MIN: u64 = 30_000;
    if effective_ctx < AGENT_CTX_MIN {
        notes.push(EstimateNote::info(
            "Small Ctx: ~30K or more may be required for compaction to work.",
        ));
    }

    let kv_cache_mb = kv_cache_mb(kv_layers, kv_embd, effective_ctx, cache_type_k, cache_type_v);

    let offload_layers = if n_gpu_layers < 0 {
        layers as i64 // -1 = all layers
    } else {
        n_gpu_layers as i64
    };
    let offload_ratio = (offload_layers as f64 / layers.max(1) as f64).clamp(0.0, 1.0);
    let model_in_vram_mb = (model_size_mb as f64 * offload_ratio) as u64;
    let model_in_ram_mb = model_size_mb - model_in_vram_mb;

    // Overhead & KV live on GPU when offloading.
    let overhead_mb = 512;
    let gpu_offload = n_gpu_layers != 0;
    let kv_in_vram_mb = if gpu_offload { kv_cache_mb } else { 0 };
    let kv_in_ram_mb = kv_cache_mb - kv_in_vram_mb;
    let overhead_in_vram_mb = if gpu_offload { overhead_mb } else { 0 };
    let overhead_in_ram_mb = overhead_mb - overhead_in_vram_mb;

    let vram_used_mb = model_in_vram_mb + kv_in_vram_mb + overhead_in_vram_mb;
    let ram_used_mb = model_in_ram_mb + kv_in_ram_mb + overhead_in_ram_mb;

    let fits = vram_used_mb <= vram_total_mb.max(1) && ram_used_mb <= ram_available_mb.max(1);
    // llama.cpp needs ~10% VRAM (min 1 GB) for CUDA context, graph, and the OS.
    let usable_vram_mb = vram_total_mb.saturating_sub((vram_total_mb / 10).max(1024));
    let tight = gpu_offload
        && vram_total_mb > 0
        && fits
        && vram_used_mb > usable_vram_mb
        && ram_used_mb <= ram_available_mb.max(1);
    if tight {
        notes.push(EstimateNote::warn(if fit {
            "Tight fit: no VRAM left for the ~1 GB CUDA/OS overhead. With Fit on, some layers \
             will run on CPU and generation will be much slower — lower Ctx or use a smaller quant."
        } else {
            "Tight fit: no VRAM left for the ~1 GB CUDA/OS overhead; the GPU may spill to \
             shared memory (much slower). Lower Ctx or use a smaller quant."
        }));
    }
    if !fits {
        if vram_used_mb > vram_total_mb && vram_total_mb > 0 {
            notes.push(EstimateNote::warn(format!(
                "Estimated VRAM usage ({:.1} GB) exceeds available VRAM ({:.1} GB).",
                vram_used_mb as f64 / 1024.0,
                vram_total_mb as f64 / 1024.0
            )));
        }
        if ram_used_mb > ram_available_mb {
            notes.push(EstimateNote::warn(format!(
                "Estimated RAM usage ({:.1} GB) exceeds available RAM ({:.1} GB).",
                ram_used_mb as f64 / 1024.0,
                ram_available_mb as f64 / 1024.0
            )));
        }
        notes.push(EstimateNote::warn(if fit {
            "Fit is on, llama.cpp reduces context and keeps some layers on CPU to \
             make it fit — expect slower generation."
        } else {
            "Fit is off, nothing auto-adjusts, you may encounter an error: lower Ctx, offload fewer layers, or use a \
             smaller quant."
        }));
    }

    MemoryEstimate {
        model_mb: mb(model_size_mb),
        kv_cache_mb: mb(kv_cache_mb),
        overhead_mb: mb(overhead_mb),
        total_mb: mb(model_size_mb + kv_cache_mb + overhead_mb),
        vram_total_mb: mb(vram_total_mb),
        ram_available_mb: mb(ram_available_mb),
        vram_used_mb: mb(vram_used_mb),
        ram_used_mb: mb(ram_used_mb),
        vram_model_mb: mb(model_in_vram_mb),
        vram_kv_mb: mb(kv_in_vram_mb),
        vram_overhead_mb: mb(overhead_in_vram_mb),
        ram_model_mb: mb(model_in_ram_mb),
        ram_kv_mb: mb(kv_in_ram_mb),
        ram_overhead_mb: mb(overhead_in_ram_mb),
        fits,
        tight,
        notes,
    }
}

// ── Launch suggestions ────────────────────────────────────────────────────

/// Model dimensions from the GGUF header; unknown fields use dense-model defaults.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub size_mb: u64,
    pub layers: u64,
    pub embedding_length: u64,
    pub attention_head_count: u64,
    pub attention_head_count_kv: u64,
    pub context_length: Option<u64>,
    pub expert_count: Option<u64>,
}

impl ModelSpec {
    pub fn size_only(size_mb: u64) -> Self {
        Self {
            size_mb,
            layers: 32,
            embedding_length: 4096,
            attention_head_count: 32,
            attention_head_count_kv: 32,
            context_length: None,
            expert_count: None,
        }
    }

    pub fn from_metadata(size_mb: u64, meta: Option<&ModelMetadata>) -> Self {
        Self {
            size_mb,
            layers: meta.and_then(|m| m.block_count).unwrap_or(32),
            embedding_length: meta.and_then(|m| m.embedding_length).unwrap_or(4096),
            attention_head_count: meta.and_then(|m| m.attention_head_count).unwrap_or(32),
            attention_head_count_kv: meta
                .and_then(|m| m.attention_head_count_kv)
                .or_else(|| meta.and_then(|m| m.attention_head_count))
                .unwrap_or(32),
            context_length: meta.and_then(|m| m.context_length),
            expert_count: meta.and_then(|m| m.expert_count),
        }
    }

    pub fn is_moe(&self) -> bool {
        self.expert_count.unwrap_or(0) > 0
    }

    pub fn kv_embd(&self) -> u64 {
        if self.attention_head_count == 0 {
            return self.embedding_length.max(1);
        }
        let factor = self.attention_head_count_kv as f64 / self.attention_head_count as f64;
        ((self.embedding_length as f64) * factor).max(1.0) as u64
    }

    /// Weight bytes per layer (includes output layer — close enough).
    pub fn bytes_per_layer(&self) -> u64 {
        if self.layers == 0 {
            return 0;
        }
        self.size_mb.saturating_mul(1024 * 1024) / self.layers
    }

    /// Native context, or a sane default when unknown.
    pub fn native_ctx(&self) -> u64 {
        self.context_length.unwrap_or(32768).clamp(4096, 131072)
    }
}

/// Suggested launch knobs; the estimator always picks an explicit context.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct SuggestedConfig {
    pub n_gpu_layers: i32,
    pub n_ctx: u32,
    pub can_fit_fully_in_vram: bool,
    pub total_usable_mb: u32,
    pub notes: Vec<String>,
    pub n_threads: Option<i32>,
    pub n_batch: Option<u32>,
    pub n_ubatch: Option<u32>,
}

pub struct FullSuggestion {
    /// -1 = all layers on GPU, else explicit count.
    pub n_gpu_layers: i32,
    pub n_ctx: u32,
    pub cache_type_k: String,
    pub cache_type_v: String,
    pub n_threads: i32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub can_fit_fully_in_vram: bool,
    pub total_usable_mb: u64,
    pub notes: Vec<String>,
}

/// Tiered fit: full GPU, balanced (half layers), then capacity; f16 first.
pub fn suggest_full(
    spec: &ModelSpec,
    total_vram_mb: u64,
    ram_mb: u64,
    cpu_cores: u32,
    cpu_threads: u32,
) -> FullSuggestion {
    let mut notes = Vec::new();

    // Headroom: 10% VRAM (min 1 GiB) for OS/CUDA/fragmentation.
    let usable_vram = total_vram_mb.saturating_sub((total_vram_mb / 10).max(1024));
    let total_usable_mb = if total_vram_mb > 0 {
        total_vram_mb + ram_mb
    } else {
        ram_mb
    };

    let layers = spec.layers.max(1);
    let kv_embd = spec.kv_embd();
    let native_ctx = spec.native_ctx();
    let bytes_per_layer = spec.bytes_per_layer().max(1);

    let kv_mb = |ctx: u64, k: &str, v: &str| kv_cache_mb(layers, kv_embd, ctx, k, v);
    let ngl_for = |kv: u64| -> u64 {
        if total_vram_mb == 0 {
            return 0;
        }
        let left = usable_vram.saturating_sub(kv.min(usable_vram));
        (left.saturating_mul(1024 * 1024) / bytes_per_layer).min(layers)
    };
    let candidates = |floor: u64| -> Vec<u64> {
        let mut out = Vec::new();
        let mut ctx = native_ctx;
        loop {
            out.push(ctx);
            if ctx <= floor {
                break;
            }
            ctx /= 2;
        }
        out
    };

    let mut pick: Option<(String, String, u64, i32, bool)> = None;

    // Tier 1: biggest ctx ≥ 8k fully on GPU (f16, then q8_0).
    'tier1: for (k, v) in [("f16", "f16"), ("q8_0", "q8_0")] {
        for ctx in candidates(native_ctx.min(8192)) {
            if spec.size_mb.saturating_add(kv_mb(ctx, k, v)) <= usable_vram {
                pick = Some((k.to_string(), v.to_string(), ctx, -1, true));
                break 'tier1;
            }
        }
    }

    // Tiers 2-3: biggest ctx within VRAM+RAM margin (f16, then q8_0).
    let margin_mb = 2048u64;
    let budget_mb = usable_vram.saturating_add(ram_mb).saturating_sub(margin_mb);
    if pick.is_none() {
        'tier2: for (k, v) in [("f16", "f16"), ("q8_0", "q8_0")] {
            for ctx in candidates(4096) {
                let total = spec.size_mb.saturating_add(kv_mb(ctx, k, v));
                if total > budget_mb {
                    continue;
                }
                if ngl_for(kv_mb(ctx, k, v)) * 2 >= layers {
                    pick = Some((k.to_string(), v.to_string(), ctx, 0, false));
                    break 'tier2;
                }
            }
        }
    }
    if pick.is_none() {
        'tier3: for (k, v) in [("f16", "f16"), ("q8_0", "q8_0")] {
            for ctx in candidates(4096) {
                let total = spec.size_mb.saturating_add(kv_mb(ctx, k, v));
                if total <= budget_mb {
                    pick = Some((k.to_string(), v.to_string(), ctx, 0, false));
                    break 'tier3;
                }
            }
        }
    }
    let (cache_type_k, cache_type_v, n_ctx, _full_gpu) = match pick {
        Some((k, v, ctx, _, full)) => (k, v, ctx, full),
        None => ("q8_0".to_string(), "q8_0".to_string(), 4096, false),
    };
    // Partial tiers defer ngl until after the KV choice.
    let kv_chosen_mb = kv_mb(n_ctx, &cache_type_k, &cache_type_v);
    if cache_type_k == "q8_0" {
        notes.push(format!(
            "f16 KV cache does not fit — using q8_0 ({:.1} GB at {} ctx), negligible quality loss.",
            kv_chosen_mb as f64 / 1024.0,
            n_ctx
        ));
    } else {
        notes.push(format!(
            "KV cache {:.1} GB at {} ctx fits in VRAM headroom — full-precision f16 cache.",
            kv_chosen_mb as f64 / 1024.0,
            n_ctx
        ));
    }
    if n_ctx < native_ctx {
        notes.push(format!(
            "Context {} does not fit (weights + KV need ~{} MB) — using {} instead.",
            native_ctx,
            spec.size_mb.saturating_add(kv_chosen_mb),
            n_ctx
        ));
    } else {
        notes.push(format!(
            "Context {} fits (weights + KV ≈ {} MB of {} MB usable).",
            n_ctx,
            spec.size_mb.saturating_add(kv_chosen_mb),
            total_usable_mb
        ));
    }

    // ── GPU layers: weights fitting VRAM after KV. Stay CPU-only when nothing fits anywhere.
    let fits_anywhere = spec.size_mb.saturating_add(kv_chosen_mb)
        <= usable_vram.saturating_add(ram_mb).saturating_sub(margin_mb);
    let layers_fit = if fits_anywhere { ngl_for(kv_chosen_mb) } else { 0 };
    if kv_chosen_mb > usable_vram && total_vram_mb > 0 {
        notes.push(format!(
            "KV cache ({:.1} GB) alone exceeds VRAM headroom — it will spill to RAM, expect slower prefill.",
            kv_chosen_mb as f64 / 1024.0
        ));
    }
    let (n_gpu_layers, can_fit_fully_in_vram) = if total_vram_mb == 0 {
        if spec.size_mb > ram_mb.saturating_sub(1024) {
            notes.push("Warning: model may not fit in available RAM.".to_string());
        }
        (0i32, false)
    } else if layers_fit >= layers {
        notes.push(format!(
            "Model + KV fit in VRAM headroom — all {} layers on GPU.",
            layers
        ));
        (-1i32, true)
    } else {
        notes.push(format!(
            "Partial offload: ~{} of {} layers on GPU ({:.0}% of weights), rest on CPU.",
            layers_fit,
            layers,
            100.0 * layers_fit as f64 / layers as f64
        ));
        (layers_fit as i32, false)
    };

    // ── MoE guidance ──
    if spec.is_moe() {
        let experts = spec.expert_count.unwrap_or(0);
        if can_fit_fully_in_vram {
            notes.push(format!(
                "MoE model ({} experts) fits fully — all experts on GPU.",
                experts
            ));
        } else {
            notes.push(format!(
                "MoE model ({} experts): if VRAM is tight, keep some experts on CPU via N CPU MoE Layers.",
                experts
            ));
        }
    }

    // ── Threads / batch ──
    // CPU-only wants all logical threads; GPU offload wants physical cores.
    let n_threads = if n_gpu_layers == 0 {
        cpu_threads.max(1).min(64)
    } else {
        cpu_cores.max(1).min(64)
    } as i32;
    notes.push(format!(
        "Threads: {} ({})",
        n_threads,
        if n_gpu_layers == 0 { "all logical threads for CPU inference" } else { "physical cores" }
    ));
    // Larger VRAM → larger ubatch (b = 4·ub), capped by ctx.
    let mut n_ubatch = if total_vram_mb >= 16384 { 1024 } else { 512 };
    if n_ubatch as u64 > n_ctx {
        let mut halved = 32u32;
        while halved * 2 <= n_ctx as u32 && halved < 1024 {
            halved *= 2;
        }
        n_ubatch = halved;
    }
    let n_batch = n_ubatch * 4;
    notes.push(format!(
        "Batch: {} / Micro-batch: {} (b=4·ub, power-of-two)",
        n_batch, n_ubatch
    ));

    FullSuggestion {
        n_gpu_layers,
        n_ctx: n_ctx as u32,
        cache_type_k,
        cache_type_v,
        n_threads,
        n_batch,
        n_ubatch,
        can_fit_fully_in_vram,
        total_usable_mb,
        notes,
    }
}

pub fn suggest_config(
    model_size_mb: u64,
    total_vram_mb: u64,
    ram_mb: u64,
    cpu_cores: u32,
    cpu_threads: u32,
) -> SuggestedConfig {
    let spec = ModelSpec::size_only(model_size_mb);
    let full = suggest_full(&spec, total_vram_mb, ram_mb, cpu_cores, cpu_threads);
    SuggestedConfig {
        n_gpu_layers: full.n_gpu_layers,
        n_ctx: full.n_ctx,
        can_fit_fully_in_vram: full.can_fit_fully_in_vram,
        total_usable_mb: mb(full.total_usable_mb),
        notes: full.notes,
        n_threads: Some(full.n_threads),
        n_batch: Some(full.n_batch),
        n_ubatch: Some(full.n_ubatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> ModelMetadata {
        ModelMetadata {
            block_count: Some(32),
            embedding_length: Some(4096),
            attention_head_count: Some(32),
            attention_head_count_kv: Some(8),
            context_length: Some(8192),
            file_size: 4_000_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn kv_scales_with_ctx_not_parallel() {
        let a = estimate_memory(&meta(), 4000, 4096, "f16", "f16", -1, true, 24000, 64000);
        let b = estimate_memory(&meta(), 4000, 8192, "f16", "f16", -1, true, 24000, 64000);
        assert_eq!(b.kv_cache_mb, a.kv_cache_mb * 2);
    }

    #[test]
    fn cpu_only_keeps_everything_in_ram() {
        let est = estimate_memory(&meta(), 4000, 4096, "f16", "f16", 0, true, 24000, 64000);
        assert_eq!(est.vram_used_mb, 0);
        assert!(est.ram_used_mb >= 4000);
    }

    #[test]
    fn partial_offload_splits_weights() {
        let est = estimate_memory(&meta(), 4000, 4096, "f16", "f16", 16, true, 24000, 64000);
        assert!(est.vram_used_mb > 0);
        assert!(est.ram_used_mb > 0);
        assert!(est.vram_model_mb > 0 && est.vram_model_mb < 4000);
    }

    #[test]
    fn tiny_gpu_does_not_fit() {
        let est = estimate_memory(&meta(), 20000, 32768, "f16", "f16", -1, true, 8000, 64000);
        assert!(!est.fits);
        assert!(est.notes.iter().any(|n| n.text.contains("VRAM")));
        assert!(!est.tight);
    }

    #[test]
    fn fitting_without_headroom_is_flagged_tight() {
        // 9B Q8 + 96k ctx on a 12 GB card: under the raw total, over the
        // usable budget once CUDA/OS overhead is reserved.
        let mut m = meta();
        m.architecture = Some("qwen35".to_string());
        m.block_count = Some(32);
        m.attention_head_count = Some(16);
        m.attention_head_count_kv = Some(4);
        m.attention_key_length = Some(256);
        let est = estimate_memory(&m, 9900, 96768, "q8_0", "q8_0", -1, true, 12287, 64000);
        assert!(est.fits, "{est:?}");
        assert!(est.tight, "{est:?}");
        assert!(est.notes.iter().any(|n| n.text.contains("Tight fit")));

        // Same model at 32k ctx has room and stays quiet.
        let ok = estimate_memory(&m, 9900, 32768, "q8_0", "q8_0", -1, true, 12287, 64000);
        assert!(ok.fits && !ok.tight, "{ok:?}");
        assert!(!ok.notes.iter().any(|n| n.text.contains("Tight fit")));
    }

    #[test]
    fn fit_notes_follow_the_toggle() {
        let mut m = meta();
        m.architecture = Some("qwen35".to_string());
        m.attention_head_count = Some(16);
        m.attention_head_count_kv = Some(4);
        m.attention_key_length = Some(256);
        let tight_fit_off = estimate_memory(&m, 9900, 96768, "q8_0", "q8_0", -1, false, 12287, 64000);
        assert!(tight_fit_off.tight);
        assert!(tight_fit_off.notes.iter().any(|n| n.text.contains("spill")));
        assert!(!tight_fit_off.notes.iter().any(|n| n.text.contains("With Fit on")));

        let over_off = estimate_memory(&m, 20000, 32768, "f16", "f16", -1, false, 8000, 64000);
        assert!(!over_off.fits);
        assert!(over_off.notes.iter().any(|n| n.text.contains("Fit is off")));
        assert!(!over_off.notes.iter().any(|n| n.text.contains("With Fit on")));

        let over_on = estimate_memory(&m, 20000, 32768, "f16", "f16", -1, true, 8000, 64000);
        assert!(over_on.notes.iter().any(|n| n.text.contains("Fit is on")));
    }

    #[test]
    fn zero_ctx_uses_model_default() {
        let est = estimate_memory(&meta(), 4000, 0, "f16", "f16", -1, true, 24000, 64000);
        assert!(est.notes.iter().any(|n| n.text.contains("model default")));
        assert!(est.kv_cache_mb > 0);
    }

    #[test]
    fn small_context_gets_a_compaction_note() {
        let small = estimate_memory(&meta(), 4000, 10_240, "f16", "f16", -1, true, 24000, 64000);
        let note = small
            .notes
            .iter()
            .find(|n| n.text.contains("compaction"))
            .expect("compaction note");
        // Plain guidance: it must not render as a warning, even beside one.
        assert!(!note.warn);
        assert!(small.fits && !small.tight);
        let roomy = estimate_memory(&meta(), 4000, 32_768, "f16", "f16", -1, true, 24000, 64000);
        assert!(!roomy.notes.iter().any(|n| n.text.contains("compaction")));
    }

    #[test]
    fn hybrid_archs_divide_kv_layers() {
        let mut m = meta();
        m.architecture = Some("lfm2".to_string());
        let hybrid = estimate_memory(&m, 4000, 4096, "f16", "f16", -1, true, 24000, 64000);
        let dense = estimate_memory(&meta(), 4000, 4096, "f16", "f16", -1, true, 24000, 64000);
        assert!(hybrid.kv_cache_mb < dense.kv_cache_mb);
        assert!(hybrid.notes.iter().any(|n| n.text.contains("Hybrid")));

        let mut ssm = meta();
        ssm.architecture = Some("mamba2".to_string());
        assert_eq!(estimate_memory(&ssm, 4000, 4096, "f16", "f16", -1, true, 24000, 64000).kv_cache_mb, 0);
    }

    #[test]
    fn suggest_config_fits_in_vram() {
        let config = suggest_config(4000, 8192, 16384, 8, 16);
        assert_eq!(config.n_gpu_layers, -1);
        assert!(config.can_fit_fully_in_vram);
    }

    #[test]
    fn suggest_config_partial_offload() {
        let config = suggest_config(12000, 8192, 32768, 8, 16);
        assert!(config.n_gpu_layers > 0, "should partially offload");
        assert!(config.n_gpu_layers < 32, "should not offload all layers");
        assert!(!config.can_fit_fully_in_vram);
    }

    #[test]
    fn suggest_config_no_gpu() {
        let config = suggest_config(4000, 0, 16384, 8, 16);
        assert_eq!(config.n_gpu_layers, 0);
        assert!(!config.can_fit_fully_in_vram);
    }

    #[test]
    fn suggest_config_model_too_large() {
        let config = suggest_config(50000, 8192, 8192, 8, 16);
        assert_eq!(config.n_gpu_layers, 0);
        assert!(!config.can_fit_fully_in_vram);
    }

    #[test]
    fn suggest_config_context_is_explicit() {
        let config = suggest_config(4000, 8192, 16384, 8, 16);
        // The estimator always picks an explicit context now (never 0).
        assert!(config.n_ctx >= 4096, "ctx should be explicit, got {}", config.n_ctx);
    }
}
