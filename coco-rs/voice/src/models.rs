//! Whisper ggml model catalog + URL / path / download-request resolution.
//!
//! Pure data and logic — no `whisper-rs`, not behind the `local-voice` feature —
//! so the download trigger and unit tests use it without the heavy runtime.
//!
//! Checksums and sizes are the authoritative git-LFS values published by
//! `ggerganov/whisper.cpp` on HuggingFace. They are the security anchor: a known
//! model always verifies against its pinned SHA-256, so a redirected mirror
//! (`download_base`) can't substitute tampered weights. A custom `model_url`
//! opts out of the pin (the user chose a different source) and therefore also
//! opts out of silent auto-download.

use std::path::PathBuf;

use coco_config::LocalWhisperConfig;
use coco_utils_download::DownloadRequest;

/// Default base for ggml whisper weights (`ggerganov/whisper.cpp`, `resolve/main`).
pub const DEFAULT_DOWNLOAD_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// A known ggml model: config token → file name, pinned SHA-256, and byte size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperModelSpec {
    /// Config token, e.g. `base.en`.
    pub name: &'static str,
    /// ggml file name under the download base, e.g. `ggml-base.en.bin`.
    pub file: &'static str,
    /// Lowercase-hex SHA-256 of the file (HuggingFace git-LFS `oid`).
    pub sha256: &'static str,
    /// File size in bytes (git-LFS `size`).
    pub size: u64,
}

/// The built-in catalog. Sizes: tiny ~78 MB, base ~148 MB, small ~488 MB,
/// medium ~1.5 GB, large-v3 ~3.1 GB, large-v3-turbo ~1.6 GB.
pub const KNOWN_MODELS: &[WhisperModelSpec] = &[
    WhisperModelSpec {
        name: "tiny",
        file: "ggml-tiny.bin",
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        size: 77_691_713,
    },
    WhisperModelSpec {
        name: "tiny.en",
        file: "ggml-tiny.en.bin",
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
        size: 77_704_715,
    },
    WhisperModelSpec {
        name: "base",
        file: "ggml-base.bin",
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        size: 147_951_465,
    },
    WhisperModelSpec {
        name: "base.en",
        file: "ggml-base.en.bin",
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        size: 147_964_211,
    },
    WhisperModelSpec {
        name: "small",
        file: "ggml-small.bin",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        size: 487_601_967,
    },
    WhisperModelSpec {
        name: "small.en",
        file: "ggml-small.en.bin",
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
        size: 487_614_201,
    },
    WhisperModelSpec {
        name: "medium",
        file: "ggml-medium.bin",
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        size: 1_533_763_059,
    },
    WhisperModelSpec {
        name: "medium.en",
        file: "ggml-medium.en.bin",
        sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
        size: 1_533_774_781,
    },
    WhisperModelSpec {
        name: "large-v3",
        file: "ggml-large-v3.bin",
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        size: 3_095_033_483,
    },
    WhisperModelSpec {
        name: "large-v3-turbo",
        file: "ggml-large-v3-turbo.bin",
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        size: 1_624_555_275,
    },
];

/// Look up a model by its config token.
pub fn find_model(name: &str) -> Option<&'static WhisperModelSpec> {
    KNOWN_MODELS.iter().find(|m| m.name == name)
}

/// The ggml file name for `model`: a known model's `file`, else the
/// conventional `ggml-<model>.bin` (whisper.cpp's own naming).
pub fn model_file_name(model: &str) -> String {
    find_model(model).map_or_else(|| format!("ggml-{model}.bin"), |m| m.file.to_string())
}

/// Resolve the on-disk model path: `<cache_dir>/<file>`, defaulting the cache
/// dir to `<config_home>/models/whisper/`. The single source of truth for where
/// weights live (both the loader and the downloader use it).
pub fn resolve_model_path(config: &LocalWhisperConfig) -> PathBuf {
    let dir = config.cache_dir.clone().unwrap_or_else(|| {
        coco_config::global_config::config_home()
            .join("models")
            .join("whisper")
    });
    dir.join(model_file_name(&config.model))
}

/// Resolve the download URL, honoring overrides by priority:
/// 1. `model_url` — full override.
/// 2. `download_base` + file — mirror override.
/// 3. built-in base + file.
pub fn resolve_download_url(config: &LocalWhisperConfig) -> String {
    if let Some(url) = &config.model_url {
        return url.clone();
    }
    let base = config
        .download_base
        .as_deref()
        .unwrap_or(DEFAULT_DOWNLOAD_BASE)
        .trim_end_matches('/');
    format!("{base}/{}", model_file_name(&config.model))
}

/// Whether `config.model` may be auto-downloaded silently on first use: it must
/// be a known (checksum-pinned) model, `auto_download` on, and no custom
/// `model_url`. This is the trust boundary — a project settings file cannot
/// point auto-download at an unverified URL.
pub fn may_auto_download(config: &LocalWhisperConfig) -> bool {
    config.auto_download && config.model_url.is_none() && find_model(&config.model).is_some()
}

/// Build a verified download request for `config.model`. The pinned SHA-256 +
/// size are attached only for a known model fetched from a base URL (default or
/// mirror); a custom `model_url` downloads unverified (size/digest unknown).
pub fn build_download_request(config: &LocalWhisperConfig, user_agent: String) -> DownloadRequest {
    let dest = resolve_model_path(config);
    let url = resolve_download_url(config);
    let verified = config
        .model_url
        .is_none()
        .then(|| find_model(&config.model))
        .flatten();
    DownloadRequest {
        url,
        dest,
        expected_sha256: verified.map(|s| s.sha256.to_string()),
        expected_size: verified.map(|s| s.size),
        user_agent,
        // The URL can originate from project-overridable config
        // (download_base / model_url), so guard against SSRF to internal hosts.
        restrict_to_public: true,
    }
}

#[cfg(test)]
#[path = "models.test.rs"]
mod tests;
