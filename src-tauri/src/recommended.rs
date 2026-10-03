//! Curated starter models with hardware-fit estimates.
//! Sizes estimate from params x quant width; installed state matches by
//! filename against the configured model dirs.

use std::path::PathBuf;

use serde::Serialize;

pub struct RecommendedDef {
    pub repo_id: &'static str,
    pub filename: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub params_b: u32,
    pub quant: &'static str,
    pub context: Option<u32>,
}

pub const RECOMMENDED: &[RecommendedDef] = &[
    RecommendedDef {
        repo_id: "LiquidAI/LFM2.5-2.6B-GGUF",
        filename: "LFM2.5-2.6B-Q4_K_M.gguf",
        name: "LFM2.5 2.6B",
        description: "LiquidAI hybrid SSM/Transformer. 131k context, very fast on low VRAM.",
        params_b: 3,
        quant: "Q4_K_M",
        context: Some(131072),
    },
    RecommendedDef {
        repo_id: "unsloth/gemma-4-E4B-it-GGUF",
        filename: "gemma-4-E4B-it-Q4_K_M.gguf",
        name: "Gemma E4B IT",
        description: "Google Gemma 4 4B instruct. Efficient edge model.",
        params_b: 4,
        quant: "Q4_K_M",
        context: None,
    },
    RecommendedDef {
        repo_id: "ornith-ai/Ornith-1.5-9B-GGUF",
        filename: "Ornith-1.5-9B-Q4_K_M.gguf",
        name: "Ornith 1.5 9B",
        description: "New 9B dense model. Strong reasoning for its size.",
        params_b: 9,
        quant: "Q4_K_M",
        context: None,
    },
    RecommendedDef {
        repo_id: "lmstudio-community/Qwen3.8-27B-GGUF",
        filename: "Qwen3.8-27B-Q4_K_M.gguf",
        name: "Qwen3.8 27B",
        description: "Latest Qwen 27B dense. High capability, hybrid architecture.",
        params_b: 27,
        quant: "Q4_K_M",
        context: None,
    },
    RecommendedDef {
        repo_id: "lmstudio-community/Muse-Glimmer-30B-GGUF",
        filename: "Muse-Glimmer-30B-Q4_K_M.gguf",
        name: "Muse Glimmer 30B",
        description: "Muse 30B dense glimmer release. Creative and capable.",
        params_b: 30,
        quant: "Q4_K_M",
        context: None,
    },
    RecommendedDef {
        repo_id: "lmstudio-community/gemma-4-31B-it-GGUF",
        filename: "gemma-4-31B-it-Q4_K_M.gguf",
        name: "Gemma 4 31B Instruct",
        description: "Google Gemma 4 31B. Well-rounded flagship.",
        params_b: 31,
        quant: "Q4_K_M",
        context: None,
    },
    RecommendedDef {
        repo_id: "lmstudio-community/Qwen3.6-35B-A3B-GGUF",
        filename: "Qwen3.6-35B-A3B-Q4_K_M.gguf",
        name: "Qwen3.6 35B MoE (3B active)",
        description: "Latest Qwen 35B MoE. Guide: --n-cpu-moe 32 on 12GB.",
        params_b: 35,
        quant: "Q4_K_M",
        context: None,
    },
    RecommendedDef {
        repo_id: "lmstudio-community/Nemotron-3-Nano-Omni-30B-A3B-Reasoning-GGUF",
        filename: "Nemotron-3-Nano-Omni-30B-A3B-Reasoning-Q4_K_M.gguf",
        name: "Nemotron 3 Omni 30B (3B active)",
        description: "NVIDIA Nemotron 30B MoE reasoning. Guide: --n-cpu-moe 32 on 12GB.",
        params_b: 30,
        quant: "Q4_K_M",
        context: None,
    },
    RecommendedDef {
        repo_id: "lmstudio-community/DeepSeek-V4-Flash-0731-GGUF",
        filename: "DeepSeek-V4-Flash-0731-Q4_K_M.gguf",
        name: "DeepSeek V4 Flash",
        description: "DeepSeek 30B MoE flash. Very fast, 3B active.",
        params_b: 30,
        quant: "Q4_K_M",
        context: None,
    },
];

/// Wire entry: sizes in MB, installed matched by filename.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct RecommendedDto {
    pub repo_id: String,
    pub filename: String,
    pub name: String,
    pub description: String,
    pub params_b: u32,
    pub quant: String,
    pub estimated_size_mb: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
    pub installed: bool,
}

/// Weight bytes per param from the quant prefix; unknown quants assume Q4.
pub fn estimate_size_mb(params_b: u32, quant: &str) -> u32 {
    let upper = quant.to_uppercase();
    let bits = if upper.starts_with("IQ2") {
        2.3
    } else if upper.starts_with("Q2") {
        2.5
    } else if upper.starts_with("IQ3") {
        3.3
    } else if upper.starts_with("Q3") {
        3.5
    } else if upper.starts_with("IQ4") {
        4.3
    } else if upper.starts_with("Q4") || upper.starts_with("MXFP4") {
        4.5
    } else if upper.starts_with("Q5") {
        5.5
    } else if upper.starts_with("Q6") {
        6.6
    } else if upper.starts_with("Q8") {
        8.5
    } else if upper == "F16" || upper == "BF16" {
        16.0
    } else if upper == "F32" {
        32.0
    } else {
        4.5
    };
    (params_b as f64 * 1e9 * bits / 8.0 / 1048576.0).min(u32::MAX as f64) as u32
}

pub fn list_recommended(model_dirs: &[PathBuf]) -> Vec<RecommendedDto> {
    let installed = crate::models::list_installed_models(model_dirs);
    RECOMMENDED
        .iter()
        .map(|def| RecommendedDto {
            repo_id: def.repo_id.to_string(),
            filename: def.filename.to_string(),
            name: def.name.to_string(),
            description: def.description.to_string(),
            params_b: def.params_b,
            quant: def.quant.to_string(),
            estimated_size_mb: estimate_size_mb(def.params_b, def.quant),
            context: def.context,
            installed: installed.iter().any(|m| m.filename == def.filename),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q4_scales_with_params() {
        let small = estimate_size_mb(3, "Q4_K_M");
        let big = estimate_size_mb(30, "Q4_K_M");
        assert!(small > 1000 && small < 2500);
        assert!((big as f64 / small as f64 - 10.0).abs() < 0.05);
    }

    #[test]
    fn quant_widths_order() {
        assert!(estimate_size_mb(8, "Q8_0") > estimate_size_mb(8, "Q4_K_M"));
        assert!(estimate_size_mb(8, "F16") > estimate_size_mb(8, "Q8_0"));
        assert_eq!(estimate_size_mb(8, "mystery"), estimate_size_mb(8, "Q4_K_M"));
    }

    #[test]
    fn empty_dirs_nothing_installed() {
        let dir = std::env::temp_dir().join(format!("werk-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let list = list_recommended(std::slice::from_ref(&dir));
        assert_eq!(list.len(), RECOMMENDED.len());
        assert!(list.iter().all(|m| !m.installed));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
