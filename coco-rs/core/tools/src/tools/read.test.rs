use crate::tools::read::ReadTool;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;

// ── R7-T25: read prompt content check ──
//
// Regression guard: the model-facing prompt() must include the
// multimodal capabilities (images/PDF/notebooks), the full-file
// default, and the cat -n format hint. Without these the model won't
// discover the tool's full surface. The short description() is only
// the per-call UI label.
#[tokio::test]
async fn test_read_prompt_mentions_multimodal_capabilities() {
    let desc = <ReadTool as DynTool>::prompt(&ReadTool, &PromptOptions::default()).await;
    assert!(desc.contains("PNG"), "missing image format hint");
    assert!(desc.contains("PDF"), "missing PDF support hint");
    assert!(
        desc.contains("Jupyter notebook") || desc.contains(".ipynb"),
        "missing notebook hint"
    );
    assert!(desc.contains("full file"), "missing full-file default");
    assert!(desc.contains("cat -n"), "missing cat -n format hint");
}

#[test]
fn test_read_description_is_short_label() {
    let fixture = json!({"file_path": "/tmp/x"});
    let desc =
        <ReadTool as DynTool>::description(&ReadTool, &fixture, &DescriptionOptions::default());
    assert_eq!(desc, "Read a file from the local filesystem.");
}

#[tokio::test]
async fn test_read_basic_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hello.txt");
    std::fs::write(&file, "line one\nline two\nline three\n").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("1\tline one"));
    assert!(text.contains("2\tline two"));
    assert!(text.contains("3\tline three"));
}

#[tokio::test]
async fn test_read_without_limit_reads_past_2000_lines_and_records_full() {
    use coco_context::FileReadRange;
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("many_lines.txt");
    let content: String = (1..=2500).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, &content).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("2500\tline 2500"), "got: {text}");
    assert!(!text.contains("more lines not shown"), "got: {text}");

    let abs = std::fs::canonicalize(&file).unwrap();
    let frs = ctx.file_read_state.as_ref().unwrap().read().await;
    let entry = frs.peek(&abs).expect("read should populate file state");
    assert_eq!(entry.range, FileReadRange::Full);
}

/// `offset` is 1-based — it corresponds directly to the line number visible
/// in editors. `offset: 10` must start at line 10 (not line 11, which was
/// the pre-fix 0-based behavior). The rendered line numbers are 1-based too.
#[tokio::test]
async fn test_read_with_offset_limit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("lines.txt");
    let content: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, &content).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap(), "offset": 10, "limit": 5}),
        &ctx,
    )
    .await
    .unwrap();

    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("10\tline 10"), "got: {text}");
    assert!(text.contains("14\tline 14"), "got: {text}");
    assert!(!text.contains("15\tline 15"), "got: {text}");
    assert!(text.contains("more lines not shown"));
}

/// `offset: 0` and `offset: 1` both mean "start from the first line".
/// Regression guard.
#[tokio::test]
async fn test_read_offset_zero_and_one_equivalent() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("start.txt");
    std::fs::write(&file, "first\nsecond\nthird\n").unwrap();

    let ctx = ToolUseContext::test_default();
    for offset in [0_u64, 1] {
        let result = <ReadTool as DynTool>::execute(
            &ReadTool,
            json!({"file_path": file.to_str().unwrap(), "offset": offset, "limit": 1}),
            &ctx,
        )
        .await
        .unwrap();
        let text = result.data["file"]["content"].as_str().unwrap();
        assert!(
            text.contains("1\tfirst"),
            "offset={offset} should start at line 1; got: {text}"
        );
    }
}

#[tokio::test]
async fn test_read_nonexistent_file() {
    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": "/nonexistent/file.txt"}),
        &ctx,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_read_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("empty.txt");
    std::fs::File::create(&file).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    // Content is empty; an empty file routes through the offset warning
    // (readFileInRangeFast yields one empty line → totalLines == 1).
    assert_eq!(result.data["file"]["content"].as_str().unwrap(), "");
    assert_eq!(result.data["file"]["totalLines"].as_i64().unwrap(), 1);
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &result.data);
    let [ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected one Text part, got {parts:?}");
    };
    assert_eq!(
        text,
        "<system-reminder>Warning: the file exists but is shorter than the provided offset (1). The file has 1 lines.</system-reminder>"
    );
}

/// Helper — generate real image bytes in the requested format. Returns
/// a 1x1 pixel valid image that the `image` crate can decode and
/// round-trip. Replaces the old fake-byte fixtures which broke when
/// the D1 two-stage compression pipeline started actually decoding.
fn real_image_bytes(format: image::ImageFormat) -> Vec<u8> {
    use image::ColorType;
    use image::ImageEncoder;
    use image::codecs::gif::GifEncoder;
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::png::PngEncoder;
    use image::codecs::webp::WebPEncoder;
    // 1x1 RGBA image: one opaque red pixel.
    let pixel_rgba = [255u8, 0, 0, 255];
    let mut buf = Vec::new();
    match format {
        image::ImageFormat::Png => {
            PngEncoder::new(&mut buf)
                .write_image(&pixel_rgba, 1, 1, ColorType::Rgba8.into())
                .unwrap();
        }
        image::ImageFormat::Jpeg => {
            // JPEG needs RGB (no alpha).
            let pixel_rgb = [255u8, 0, 0];
            JpegEncoder::new_with_quality(&mut buf, 85)
                .write_image(&pixel_rgb, 1, 1, ColorType::Rgb8.into())
                .unwrap();
        }
        image::ImageFormat::Gif => {
            let frame =
                image::Frame::new(image::RgbaImage::from_raw(1, 1, pixel_rgba.to_vec()).unwrap());
            let mut encoder = GifEncoder::new(&mut buf);
            encoder.encode_frame(frame).unwrap();
        }
        image::ImageFormat::WebP => {
            WebPEncoder::new_lossless(&mut buf)
                .write_image(&pixel_rgba, 1, 1, ColorType::Rgba8.into())
                .unwrap();
        }
        _ => panic!("unsupported test format"),
    }
    buf
}

#[tokio::test]
async fn test_read_image_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.png");
    std::fs::write(&file, real_image_bytes(image::ImageFormat::Png)).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    // Post-D1: the image goes through resize+re-encode and returns as a
    // multimodal block with processed bytes.
    assert_eq!(result.data["type"], "image");
}

#[tokio::test]
async fn test_read_directory_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": dir.path().to_str().unwrap()}),
        &ctx,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("directory"));
}

#[tokio::test]
async fn test_read_binary_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.sqlite");
    std::fs::write(&file, b"\x00\x01\x02\x03").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("binary"));
}

// ---------------------------------------------------------------------------
// B2.1: base64 image encoding + device blocklist + offset-beyond-file
// ---------------------------------------------------------------------------

/// PNG/JPG/GIF/WEBP files get returned as a structured multimodal image
/// block: `{type: image, source: {type: base64, media_type, data}}`.
/// Images are processed through the D1 resize+re-encode pipeline, so the returned media_type may
/// differ from the filename hint (e.g. WebP with alpha might be
/// downgraded to PNG).
#[tokio::test]
async fn test_read_png_returns_base64_block() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pixel.png");
    std::fs::write(&file, real_image_bytes(image::ImageFormat::Png)).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let data = &result.data;
    // Image envelope: `{ type: 'image', file: { base64,
    // type, originalSize } }`. The old `source.type=base64` discriminator
    // was removed when the output schema migrated to aligned shapes
    // — `file.type` now holds the MIME directly.
    assert_eq!(data["type"], "image");
    assert_eq!(data["file"]["type"], "image/png");
    let b64 = data["file"]["base64"].as_str().unwrap();
    assert!(!b64.is_empty(), "base64 data should not be empty");
    assert!(
        data["file"]["originalSize"].as_u64().is_some(),
        "originalSize should be populated"
    );
}

#[tokio::test]
async fn test_read_jpeg_returns_base64_block() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpg");
    std::fs::write(&file, real_image_bytes(image::ImageFormat::Jpeg)).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["type"], "image");
    assert_eq!(result.data["file"]["type"], "image/jpeg");
}

/// `jpeg` extension also maps to image/jpeg (same as `jpg`).
#[tokio::test]
async fn test_read_jpeg_alt_extension() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("photo.jpeg");
    std::fs::write(&file, real_image_bytes(image::ImageFormat::Jpeg)).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.data["file"]["type"], "image/jpeg");
}

#[tokio::test]
async fn test_read_webp_returns_base64_block() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("image.webp");
    std::fs::write(&file, real_image_bytes(image::ImageFormat::WebP)).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(result.data["file"]["type"], "image/webp");
}

#[tokio::test]
async fn test_read_gif_returns_base64_block() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("anim.gif");
    std::fs::write(&file, real_image_bytes(image::ImageFormat::Gif)).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    // GIF round-trips through coco-utils-image which re-encodes to PNG
    // for formats without a lossless path (GIF falls into this bucket
    // because the image crate can't encode-to-GIF without extra
    // features). The model still receives a valid image, just with
    // a different media_type.
    let media = result.data["file"]["type"].as_str().unwrap();
    assert!(
        media == "image/gif" || media == "image/png",
        "GIF should round-trip as image/gif or image/png, got {media}"
    );
}

/// Regression guard: images larger than `MAX_WIDTH × MAX_HEIGHT`
/// (2048 × 768 in coco-utils-image) must be resized by the pipeline,
/// proving the D1 two-stage compression is actually wired up. We
/// generate a 4096×1536 PNG — well over both dimension caps — and
/// verify the output base64 is smaller than the input raw bytes.
#[tokio::test]
async fn test_read_large_image_gets_resized() {
    use image::ColorType;
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("huge.png");

    // 4096×1536 all-red image → ~24MB of raw RGBA.
    let w = 4096u32;
    let h = 1536u32;
    let pixel_count = (w * h) as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for _ in 0..pixel_count {
        rgba.extend_from_slice(&[255u8, 0, 0, 255]);
    }
    let mut encoded = Vec::new();
    PngEncoder::new(&mut encoded)
        .write_image(&rgba, w, h, ColorType::Rgba8.into())
        .unwrap();
    let original_size = encoded.len();
    std::fs::write(&file, &encoded).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    // Sanity: still a valid image block.
    assert_eq!(result.data["type"], "image");
    // Extract the base64 and decode to verify it's smaller than the
    // original. We also check the decoded length (raw bytes after
    // base64 decode) because base64 inflates by ~4/3.
    let b64 = result.data["file"]["base64"].as_str().unwrap();
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    assert!(
        decoded.len() < original_size,
        "resized image ({} bytes) should be smaller than original ({} bytes)",
        decoded.len(),
        original_size
    );

    // R7-T20: dimensions metadata. originalWidth/originalHeight should
    // reflect the source 4096×1536, displayWidth/displayHeight should
    // be the post-resize size (≤ MAX_WIDTH × MAX_HEIGHT = 2048×768).
    let dims = &result.data["file"]["dimensions"];
    assert_eq!(dims["originalWidth"], 4096);
    assert_eq!(dims["originalHeight"], 1536);
    let display_w = dims["displayWidth"].as_u64().unwrap();
    let display_h = dims["displayHeight"].as_u64().unwrap();
    assert!(
        display_w <= 2048 && display_h <= 768,
        "displayWidth/Height should be capped to MAX bounds, got {display_w}x{display_h}"
    );
    // Resized must be strictly smaller in at least one dimension.
    assert!(display_w < 4096 || display_h < 1536);
}

/// R7-T20: every successful Read must push the file path into
/// `ctx.nested_memory_attachment_triggers` so the next-turn message
/// builder can attach any nested CLAUDE.md memories from the file's
/// ancestry.
#[tokio::test]
async fn test_read_populates_nested_memory_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.md");
    std::fs::write(&file, "some content\n").unwrap();

    let ctx = ToolUseContext::test_default();
    let _ = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let triggers = ctx.nested_memory_attachment_triggers.read().await;
    let canonical = std::fs::canonicalize(&file).unwrap();
    let canonical_str = canonical.display().to_string();
    assert!(
        triggers.contains(&canonical_str),
        "expected {canonical_str} in nested_memory_attachment_triggers, got: {triggers:?}"
    );
}

/// Small image (under MAX_WIDTH × MAX_HEIGHT) should round-trip with
/// display dimensions equal to original dimensions.
#[tokio::test]
async fn test_read_small_image_dimensions_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("small.png");
    std::fs::write(&file, real_image_bytes(image::ImageFormat::Png)).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let dims = &result.data["file"]["dimensions"];
    // 1×1 fixture from real_image_bytes — both original and display
    // should report 1×1.
    assert_eq!(dims["originalWidth"], 1);
    assert_eq!(dims["originalHeight"], 1);
    assert_eq!(dims["displayWidth"], 1);
    assert_eq!(dims["displayHeight"], 1);
}

/// SVG is an intentionally-unsupported format (raster-only image crate
/// + Anthropic API doesn't accept it). SVGs get the placeholder response
/// like BMP/TIFF/ICO. `IMAGE_EXTENSIONS` does NOT include SVG.
#[tokio::test]
async fn test_read_svg_returns_placeholder_not_base64() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("logo.svg");
    std::fs::write(&file, b"<svg></svg>").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    // SVG is in PLACEHOLDER_IMAGE_EXTENSIONS, so we return a text
    // placeholder — not a multimodal image block.
    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("svg"));
    assert!(text.contains("not supported"));
}

/// BMP/ICO/TIFF still get a placeholder (not supported by Anthropic
/// multimodal API). These aren't errors — the tool tells the model the
/// file exists and hints at conversion.
#[tokio::test]
async fn test_read_bmp_returns_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("legacy.bmp");
    std::fs::write(&file, b"BM").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("bmp"));
    assert!(text.contains("not supported"));
}

/// Blocked device paths must be rejected with InvalidInput. These paths
/// never get `std::fs::read_to_string` called on them, so there's no risk
/// of hanging on /dev/stdin.
#[tokio::test]
async fn test_read_blocks_dev_zero() {
    let ctx = ToolUseContext::test_default();
    let result =
        <ReadTool as DynTool>::execute(&ReadTool, json!({"file_path": "/dev/zero"}), &ctx).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("device"), "error should mention device: {err}");
}

#[tokio::test]
async fn test_read_blocks_dev_stdin() {
    let ctx = ToolUseContext::test_default();
    let result =
        <ReadTool as DynTool>::execute(&ReadTool, json!({"file_path": "/dev/stdin"}), &ctx).await;
    assert!(result.is_err());
}

/// `/dev/null` is NOT blocked — it's a common sink and reading returns EOF
/// immediately. Opening it for read is harmless.
#[tokio::test]
async fn test_read_dev_null_is_not_blocked() {
    let ctx = ToolUseContext::test_default();
    let result =
        <ReadTool as DynTool>::execute(&ReadTool, json!({"file_path": "/dev/null"}), &ctx).await;
    // Either succeeds (treating as empty file) or fails with a different
    // reason — the key assertion is the error is NOT "cannot read device
    // file" which would only come from the blocklist.
    if let Err(e) = &result {
        assert!(
            !e.to_string().contains("Cannot read device file"),
            "/dev/null must not be blocklisted"
        );
    }
}

// ---------------------------------------------------------------------------
// B2.2: encoding detection
// ---------------------------------------------------------------------------

/// UTF-8 BOM files should be read correctly. coco-file-encoding detects
/// `Utf8WithBom` and strips the BOM on decode.
#[tokio::test]
async fn test_read_utf8_with_bom() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bom.txt");
    // UTF-8 BOM + "hello\nworld"
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"hello\nworld\n");
    std::fs::write(&file, &bytes).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("hello"), "got: {text}");
    assert!(text.contains("world"));
    // BOM should not appear in the decoded content.
    assert!(!text.contains('\u{FEFF}'), "BOM must be stripped: {text}");
}

/// UTF-16LE files (with BOM `FF FE`) must decode correctly instead of
/// erroring on "invalid UTF-8".
#[tokio::test]
async fn test_read_utf16le_bom() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("utf16.txt");

    // UTF-16LE BOM + "hi\n" encoded as u16 little-endian pairs.
    let mut bytes = vec![0xFF, 0xFE]; // BOM
    for ch in "hi\n".chars() {
        let code = ch as u16;
        bytes.extend_from_slice(&code.to_le_bytes());
    }
    std::fs::write(&file, &bytes).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("hi"), "should decode UTF-16LE: {text}");
}

// ---------------------------------------------------------------------------
// R6-T20: file-read permission gate on check_permissions
// ---------------------------------------------------------------------------

/// `Read` performs its path permission check directly, so cwd paths
/// are allowed without falling through to the generic rule pipeline.
#[tokio::test]
async fn test_read_check_permissions_allows_cwd_path() {
    use coco_types::ToolCheckResult;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ordinary.txt");
    std::fs::write(&file, "ok").unwrap();
    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(dir.path().to_path_buf());

    let decision = <ReadTool as DynTool>::check_permissions(
        &ReadTool,
        &json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await;
    assert!(matches!(decision, ToolCheckResult::Allow { .. }));
}

// ---------------------------------------------------------------------------
// R6-T16: PDF extraction
// ---------------------------------------------------------------------------

use crate::tools::read::parse_page_range;

/// Single-page spec: `"3"` → `(3, 3)`.
#[test]
fn test_parse_page_range_single() {
    assert_eq!(parse_page_range("3", 10), Some((3, 3)));
    assert_eq!(parse_page_range("  5  ", 10), Some((5, 5)));
}

/// Dash range: `"1-5"` → `(1, 5)`.
#[test]
fn test_parse_page_range_dash() {
    assert_eq!(parse_page_range("1-5", 10), Some((1, 5)));
    assert_eq!(parse_page_range("2 - 4", 10), Some((2, 4)));
}

/// Over-range is clamped to the document length.
#[test]
fn test_parse_page_range_clamps_to_total() {
    assert_eq!(parse_page_range("1-100", 10), Some((1, 10)));
}

/// Invalid specs return `None`.
#[test]
fn test_parse_page_range_invalid() {
    assert_eq!(parse_page_range("0", 10), None, "page 0 is invalid");
    assert_eq!(parse_page_range("abc", 10), None, "non-numeric");
    assert_eq!(parse_page_range("5-1", 10), None, "end < start");
    assert_eq!(parse_page_range("", 10), None, "empty spec");
}

/// A malformed PDF file must surface as ExecutionFailed without panic.
#[tokio::test]
async fn test_read_malformed_pdf_errors_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("broken.pdf");
    // Not a valid PDF — just random bytes.
    std::fs::write(&file, b"not a real pdf").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "malformed PDF should error");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("PDF") || err.contains("pdf") || err.contains("extract"),
        "error should mention PDF extraction: {err}"
    );
}

/// #25: a FULL read of a file larger than MAX_READ_OUTPUT_BYTES throws
/// `FileTooLargeError` instead of truncating — the model must narrow
/// with offset/limit.
#[tokio::test]
async fn test_read_full_read_too_large_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("huge.txt");
    // 500K bytes — over the 256K full-read cap.
    let content: String = "x".repeat(500_000);
    std::fs::write(&file, &content).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await;
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("exceeds maximum allowed size"),
        "full read of an oversized file must error: {err}"
    );
}

/// #25: a PARTIAL read (explicit `limit`) of a big file skips the
/// full-file size cap — only the line and token caps apply.
#[tokio::test]
async fn test_read_partial_read_skips_size_cap() {
    use coco_context::FileReadRange;
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big_lines.txt");
    // 50K short lines → file well over 256K total, but the slice is tiny.
    let content: String = (0..50_000).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, &content).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap(), "offset": 1, "limit": 3}),
        &ctx,
    )
    .await
    .unwrap();
    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("1\tline 0"), "got: {text}");
    assert!(text.contains("more lines not shown"));

    let abs = std::fs::canonicalize(&file).unwrap();
    let frs = ctx.file_read_state.as_ref().unwrap().read().await;
    let entry = frs.peek(&abs).expect("read should populate file state");
    assert_eq!(
        entry.range,
        FileReadRange::Lines {
            offset: None,
            limit: 3,
        }
    );
}

#[tokio::test]
async fn test_read_large_explicit_limit_streams_range() {
    use coco_context::FileReadRange;
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("large_stream.txt");
    // >10MB UTF-8 file. The requested range is tiny and near EOF, so this
    // must not go through the full-file size cap.
    let content: String = (1..=1_200_000).map(|i| format!("line {i:07}\n")).collect();
    assert!(content.len() > 10 * 1024 * 1024);
    std::fs::write(&file, content).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap(), "offset": 1_199_999, "limit": 2}),
        &ctx,
    )
    .await
    .unwrap();

    let text = result.data["file"]["content"].as_str().unwrap();
    assert!(text.contains("1199999\tline 1199999"), "got: {text}");
    assert!(text.contains("1200000\tline 1200000"), "got: {text}");
    assert!(!text.contains("more lines not shown"), "got: {text}");
    assert_eq!(result.data["file"]["totalLines"], 1_200_000);

    let abs = std::fs::canonicalize(&file).unwrap();
    let frs = ctx.file_read_state.as_ref().unwrap().read().await;
    let entry = frs.peek(&abs).expect("read should populate file state");
    assert_eq!(
        entry.range,
        FileReadRange::Lines {
            offset: Some(1_199_999),
            limit: 2,
        }
    );
    assert_eq!(entry.content, "line 1199999\nline 1200000");
}

/// #17: a slice whose token estimate exceeds the budget throws
/// `MaxFileReadTokenExceededError`.
#[tokio::test]
async fn test_read_token_cap_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dense.txt");
    // First line ~160K chars → /4 = 40K tokens > 25K cap. limit=1 skips
    // the full-file size cap so the token cap is what fires.
    let content = format!("{}\nsecond\n", "z".repeat(160_000));
    std::fs::write(&file, &content).unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap(), "limit": 1}),
        &ctx,
    )
    .await;
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("exceeds maximum allowed tokens"),
        "oversized slice must hit the token cap: {err}"
    );
}

/// Offset > total_lines returns a helpful warning instead of empty output.
#[tokio::test]
async fn test_read_offset_beyond_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("short.txt");
    std::fs::write(&file, "line1\nline2\nline3\n").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap(), "offset": 100}),
        &ctx,
    )
    .await
    .unwrap();

    // Content is empty; the warning is produced at render time (map layer).
    assert_eq!(result.data["file"]["content"].as_str().unwrap(), "");
    assert_eq!(result.data["file"]["startLine"].as_i64().unwrap(), 100);
    assert_eq!(result.data["file"]["totalLines"].as_i64().unwrap(), 3);
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &result.data);
    let [ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected one Text part, got {parts:?}");
    };
    assert_eq!(
        text,
        "<system-reminder>Warning: the file exists but is shorter than the provided offset (100). The file has 3 lines.</system-reminder>"
    );
}

// ── R7-T16: notebook structured cells tests ──
//
// Projects each cell into
// `{ cellType, source, cell_id, language?, execution_count?, outputs? }`.
// The tests below verify the projection: a basic two-cell notebook
// (markdown + code) plus output handling.

/// Helper — write a minimal Jupyter notebook fixture to a temp file.
fn write_notebook_fixture(file: &std::path::Path, cells: serde_json::Value) {
    let notebook = serde_json::json!({
        "metadata": {
            "language_info": { "name": "python" }
        },
        "nbformat": 4,
        "nbformat_minor": 5,
        "cells": cells,
    });
    std::fs::write(file, serde_json::to_string(&notebook).unwrap()).unwrap();
}

#[tokio::test]
async fn test_read_notebook_returns_structured_cells() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("simple.ipynb");
    write_notebook_fixture(
        &file,
        serde_json::json!([
            {
                "cell_type": "markdown",
                "id": "intro",
                "source": ["# Heading\n", "Some text"],
                "metadata": {},
            },
            {
                "cell_type": "code",
                "id": "compute",
                "source": "result = 1 + 1\nresult",
                "execution_count": 1,
                "metadata": {},
                "outputs": [
                    {
                        "output_type": "execute_result",
                        "execution_count": 1,
                        "data": { "text/plain": "2" },
                        "metadata": {}
                    }
                ]
            }
        ]),
    );

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["type"], "notebook");
    let cells = result.data["file"]["cells"]
        .as_array()
        .expect("cells array");
    assert_eq!(cells.len(), 2);

    // Markdown cell: cellType, source joined, cell_id from `id`,
    // NO language/execution_count/outputs (omits for non-code).
    assert_eq!(cells[0]["cellType"], "markdown");
    assert_eq!(cells[0]["source"], "# Heading\nSome text");
    assert_eq!(cells[0]["cell_id"], "intro");
    assert!(cells[0].get("language").is_none());
    assert!(cells[0].get("execution_count").is_none());

    // Code cell: cellType, source string-as-is, language defaulted from
    // notebook metadata, execution_count carried, outputs projected.
    assert_eq!(cells[1]["cellType"], "code");
    assert_eq!(cells[1]["source"], "result = 1 + 1\nresult");
    assert_eq!(cells[1]["cell_id"], "compute");
    assert_eq!(cells[1]["language"], "python");
    assert_eq!(cells[1]["execution_count"], 1);
    let outputs = cells[1]["outputs"].as_array().expect("outputs array");
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0]["output_type"], "execute_result");
    assert_eq!(outputs[0]["text"], "2");
}

#[tokio::test]
async fn test_read_notebook_synthesizes_cell_id_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("legacy.ipynb");
    // Old-style notebooks (nbformat < 4.5) don't have `id` on cells.
    // Synthesize `cell-N` (0-based index).
    write_notebook_fixture(
        &file,
        serde_json::json!([
            { "cell_type": "code", "source": "x = 1", "outputs": [], "metadata": {} },
            { "cell_type": "code", "source": "y = 2", "outputs": [], "metadata": {} },
        ]),
    );

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let cells = result.data["file"]["cells"].as_array().unwrap();
    assert_eq!(cells[0]["cell_id"], "cell-0");
    assert_eq!(cells[1]["cell_id"], "cell-1");
}

#[tokio::test]
async fn test_read_notebook_truncates_large_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big.ipynb");
    // 12K-char output exceeds the 10K LARGE_OUTPUT_THRESHOLD.
    let big_text = "x".repeat(12_000);
    write_notebook_fixture(
        &file,
        serde_json::json!([{
            "cell_type": "code",
            "id": "big",
            "source": "print('big')",
            "execution_count": 1,
            "metadata": {},
            "outputs": [{
                "output_type": "stream",
                "text": [big_text]
            }]
        }]),
    );

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let outputs = result.data["file"]["cells"][0]["outputs"]
        .as_array()
        .unwrap();
    assert_eq!(outputs.len(), 1);
    let text = outputs[0]["text"].as_str().unwrap();
    // Should be the truncation hint, not the original 12K of x's.
    assert!(
        text.contains("too large to include") && text.contains("jq"),
        "expected truncation hint, got: {text}"
    );
}

#[tokio::test]
async fn test_read_notebook_stream_output() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("stream.ipynb");
    write_notebook_fixture(
        &file,
        serde_json::json!([{
            "cell_type": "code",
            "id": "printer",
            "source": "print('hello')",
            "execution_count": 1,
            "metadata": {},
            "outputs": [{
                "output_type": "stream",
                "name": "stdout",
                "text": ["hello\n"]
            }]
        }]),
    );

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let outputs = result.data["file"]["cells"][0]["outputs"]
        .as_array()
        .unwrap();
    assert_eq!(outputs[0]["output_type"], "stream");
    assert_eq!(outputs[0]["text"], "hello\n");
}

#[tokio::test]
async fn test_read_notebook_error_output() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("error.ipynb");
    write_notebook_fixture(
        &file,
        serde_json::json!([{
            "cell_type": "code",
            "id": "broken",
            "source": "1 / 0",
            "execution_count": 1,
            "metadata": {},
            "outputs": [{
                "output_type": "error",
                "ename": "ZeroDivisionError",
                "evalue": "division by zero",
                "traceback": ["Traceback line 1", "Traceback line 2"]
            }]
        }]),
    );

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let outputs = result.data["file"]["cells"][0]["outputs"]
        .as_array()
        .unwrap();
    assert_eq!(outputs[0]["output_type"], "error");
    let text = outputs[0]["text"].as_str().unwrap();
    assert!(text.contains("ZeroDivisionError: division by zero"));
    assert!(text.contains("Traceback line 1"));
    assert!(text.contains("Traceback line 2"));
}

#[tokio::test]
async fn test_read_notebook_image_output() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("plot.ipynb");
    write_notebook_fixture(
        &file,
        serde_json::json!([{
            "cell_type": "code",
            "id": "plot",
            "source": "plt.plot(...)",
            "execution_count": 1,
            "metadata": {},
            "outputs": [{
                "output_type": "display_data",
                "data": {
                    "text/plain": "<Figure>",
                    "image/png": "iVBORw0KGgo=  whitespace test"
                },
                "metadata": {}
            }]
        }]),
    );

    let ctx = ToolUseContext::test_default();
    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let output = &result.data["file"]["cells"][0]["outputs"][0];
    assert_eq!(output["output_type"], "display_data");
    assert_eq!(output["text"], "<Figure>");
    let image = &output["image"];
    assert_eq!(image["media_type"], "image/png");
    // Whitespace-stripped: `data['image/png'].replace(/\s/g,'')`.
    assert_eq!(image["image_data"], "iVBORw0KGgo=whitespacetest");
}

// ── R7-T9: file_unchanged dedup tests ──
//
// Returns a `{ type: 'file_unchanged' }` stub instead of resending the full
// content when:
//   1. the same path was previously read via the Read tool (not Edit/Write)
//   2. the same input offset/limit are requested
//   3. the disk mtime hasn't changed
// The tests below exercise each gate.

/// Two consecutive default Read calls on the same file: the second should
/// return a stub. This is the core dedup path — BQ telemetry shows ~18%
/// of Read calls are repeats of this shape.
#[tokio::test]
async fn test_read_dedup_same_call_twice() {
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dedup.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    // First call — full content returned in the text envelope.
    let first = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(first.data["type"], "text");
    let first_text = first.data["file"]["content"]
        .as_str()
        .expect("first call returns text");
    assert!(first_text.contains("alpha"));

    // Second call — should hit the dedup stub.
    let second = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        second.data["type"], "file_unchanged",
        "second identical Read should return file_unchanged stub, got: {:?}",
        second.data
    );
    assert_eq!(second.data["file"]["filePath"], file.to_str().unwrap());
}

/// Repeat Read with same explicit offset/limit args should still dedup.
#[tokio::test]
async fn test_read_dedup_same_explicit_range() {
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dedup_range.txt");
    let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, content).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    let args = json!({
        "file_path": file.to_str().unwrap(),
        "offset": 10,
        "limit": 5
    });
    let _first = <ReadTool as DynTool>::execute(&ReadTool, args.clone(), &ctx)
        .await
        .unwrap();
    let second = <ReadTool as DynTool>::execute(&ReadTool, args, &ctx)
        .await
        .unwrap();
    assert_eq!(
        second.data["type"], "file_unchanged",
        "second Read with identical offset/limit should dedup, got: {:?}",
        second.data
    );
}

/// Different offset/limit should NOT dedup — the cached entry doesn't
/// cover the new range.
#[tokio::test]
async fn test_read_dedup_skipped_for_different_range() {
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("no_dedup.txt");
    let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, content).unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    let _first = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap(), "offset": 10, "limit": 5}),
        &ctx,
    )
    .await
    .unwrap();
    let second = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap(), "offset": 20, "limit": 5}),
        &ctx,
    )
    .await
    .unwrap();
    // Should be the text envelope, not a file_unchanged stub.
    assert_eq!(
        second.data["type"], "text",
        "expected text envelope, got: {:?}",
        second.data
    );
}

/// Mtime change invalidates the dedup — file modified externally between
/// reads should return fresh content. We mutate the cached `mtime_ms`
/// directly to simulate "disk mtime moved forward" without relying on
/// filesystem mtime precision (which can be 1s on ext4/HFS+).
#[tokio::test]
async fn test_read_dedup_invalidated_by_mtime_change() {
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mtime.txt");
    std::fs::write(&file, "version one\n").unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    // Prime the cache.
    let _first = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    // Forge a stale cached mtime (1 ms behind the disk mtime) so the dedup
    // gate fails on the second call. This is more robust than relying on
    // filesystem mtime precision in tests.
    {
        let abs = std::fs::canonicalize(&file).unwrap();
        let frs = ctx.file_read_state.as_ref().unwrap();
        let mut frs_w = frs.write().await;
        // Pull the existing entry, decrement mtime, reinsert via
        // `set_from_read` to preserve the from-read marker + input range.
        if let Some(stale) = frs_w.peek(&abs).cloned() {
            let mut stale = stale;
            stale.mtime_ms -= 1;
            frs_w.set_from_read(abs, stale, None, None);
        }
    }

    let second = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    // Cache mtime is stale → dedup gate fails → fresh read returned.
    assert_eq!(second.data["type"], "text");
    let text = second.data["file"]["content"]
        .as_str()
        .expect("expected text after mtime mismatch");
    assert!(text.contains("version one"), "got: {text}");
}

/// Edit then Read should NOT dedup against the post-edit entry, because
/// the post-edit content was never returned to the model as a Read result.
/// Gated via `is_from_read_tool`.
#[tokio::test]
async fn test_read_dedup_skipped_after_edit() {
    use coco_context::FileReadEntry;
    use coco_context::FileReadState;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("post_edit.txt");
    std::fs::write(&file, "post-edit content\n").unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.file_read_state = Some(Arc::new(RwLock::new(FileReadState::new())));

    // Simulate a prior Edit by inserting an entry via `set` (not
    // `set_from_read`). That mirrors what `update_after_edit` would do
    // post-edit, but more directly.
    let abs = std::fs::canonicalize(&file).unwrap();
    let mtime = coco_context::file_mtime_ms(&abs).await.unwrap();
    {
        let frs = ctx.file_read_state.as_ref().unwrap();
        let mut frs_w = frs.write().await;
        frs_w.set(
            abs,
            FileReadEntry::full_real("post-edit content\n".into(), mtime),
        );
    }

    let result = <ReadTool as DynTool>::execute(
        &ReadTool,
        json!({"file_path": file.to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();
    // Should NOT be a stub — the entry came from `set`, not
    // `set_from_read`, so dedup is skipped. The result is the text envelope.
    assert_eq!(result.data["type"], "text");
    let text = result.data["file"]["content"]
        .as_str()
        .expect("expected text, got stub");
    assert!(text.contains("post-edit content"), "got: {text}");
}

// ── render_for_model branches (Phase 3) ──

#[test]
fn render_for_model_image_emits_filedata_part() {
    // The multimodal payoff: image data goes to providers as a typed
    // FileData block, NOT as JSON-stringified base64. Anthropic +
    // Gemini 3+ pass through to the vision model; OpenAI degrades at
    // the provider boundary.
    let data = json!({
        "type": "image",
        "file": {
            "base64": "iVBORw0KGgoAAAANS",
            "type": "image/png",
            "originalSize": 1024,
            "dimensions": { "originalWidth": 100, "originalHeight": 100 },
        },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    assert_eq!(
        parts,
        vec![ToolResultContentPart::FileData {
            data: "iVBORw0KGgoAAAANS".into(),
            media_type: "image/png".into(),
            filename: None,
            provider_options: None,
        }]
    );
}

#[test]
fn render_for_model_text_emits_content_only_no_json_wrapper() {
    // The token-efficiency payoff: cat-style content already lives in
    // `data.file.content`; the renderer hands it through as a single
    // Text part. Pre-refactor the model saw JSON `{"type":"text","file":{"content":"...","numLines":...}}`
    // — extra ~10–15% tokens from the envelope and escapes.
    let data = json!({
        "type": "text",
        "file": {
            "filePath": "/tmp/foo.py",
            "content": "1\tdef foo():\n2\t    return 42\n",
            "numLines": 2,
            "startLine": 1,
            "totalLines": 2,
        },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    assert_eq!(
        parts,
        vec![ToolResultContentPart::Text {
            text: "1\tdef foo():\n2\t    return 42\n".into(),
            provider_options: None,
        }]
    );
}

#[test]
fn render_for_model_pdf_emits_extracted_content() {
    let data = json!({
        "type": "pdf",
        "file": {
            "filePath": "/tmp/spec.pdf",
            "content": "[PDF file: /tmp/spec.pdf, 3 page(s), showing page(s) 1-3]\n\n--- Page 1 ---\nIntroduction\n\n",
            "totalPages": 3,
        },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    let [ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected single Text part, got {parts:?}");
    };
    assert!(text.starts_with("[PDF file: /tmp/spec.pdf"));
    assert!(text.contains("--- Page 1 ---"));
}

#[test]
fn render_for_model_file_unchanged_uses_ts_stub_phrasing() {
    // `FILE_UNCHANGED_STUB` is a bare string — no `<system-reminder>`
    // wrapper, no per-path interpolation. The model recognizes the
    // literal phrase as "skip the re-read".
    let data = json!({
        "type": "file_unchanged",
        "file": { "filePath": "/tmp/foo.py" },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    let [ToolResultContentPart::Text { text, .. }] = parts.as_slice() else {
        panic!("expected single Text part, got {parts:?}");
    };
    assert_eq!(
        text,
        "File unchanged since last read. The content from the earlier Read tool_result in this conversation is still current — refer to that instead of re-reading."
    );
}

#[test]
fn render_for_model_notebook_merges_adjacent_text_into_one_part() {
    // `cellContentToToolResult` shape:
    // `<cell id="X">[<cell_type>Y</cell_type>][<language>Z</language>]source</cell id="X">`.
    // Markdown cells get a `<cell_type>` tag; non-python code cells get
    // a `<language>` tag; python code cells get neither. Adjacent text
    // blocks fold into a single Text part joined with `'\n'` via
    // `allResults.reduce(...)`.
    let data = json!({
        "type": "notebook",
        "file": {
            "filePath": "/tmp/nb.ipynb",
            "cells": [
                {
                    "cellType": "markdown",
                    "source": "# Title\n",
                    "cell_id": "cell-0",
                },
                {
                    "cellType": "code",
                    "source": "print(42)\n",
                    "cell_id": "cell-1",
                    "execution_count": 5,
                    "language": "python",
                    "outputs": [
                        { "output_type": "stream", "text": "42\n" }
                    ],
                },
            ],
        },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    assert_eq!(
        parts.len(),
        1,
        "expected merged single Text part, got {parts:?}"
    );
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("part 0 should be Text, got {parts:?}");
    };
    assert_eq!(
        text,
        "<cell id=\"cell-0\"><cell_type>markdown</cell_type># Title\n</cell id=\"cell-0\">\n<cell id=\"cell-1\">print(42)\n</cell id=\"cell-1\">\n\n42\n"
    );
}

#[test]
fn render_for_model_notebook_image_output_becomes_filedata_part() {
    // Multimodal pass-through: image outputs from notebook cells
    // surface as FileData parts (real images on Anthropic/Gemini,
    // text marker on OpenAI/OpenAI-Compatible via provider degradation).
    // Adjacent text parts fold into one; the image breaks the chain so
    // we end up with `[Text(cell + output_text), FileData(image)]`.
    let data = json!({
        "type": "notebook",
        "file": {
            "filePath": "/tmp/plot.ipynb",
            "cells": [
                {
                    "cellType": "code",
                    "source": "plt.show()\n",
                    "cell_id": "cell-0",
                    "language": "python",
                    "outputs": [
                        {
                            "output_type": "display_data",
                            "text": "<Figure>",
                            "image": {
                                "image_data": "iVBOR...",
                                "media_type": "image/png",
                            }
                        }
                    ],
                },
            ],
        },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    assert_eq!(parts.len(), 2, "expected [Text, FileData], got {parts:?}");
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("part 0 should be Text, got {parts:?}");
    };
    assert_eq!(
        text,
        "<cell id=\"cell-0\">plt.show()\n</cell id=\"cell-0\">\n\n<Figure>"
    );
    match &parts[1] {
        ToolResultContentPart::FileData {
            data,
            media_type,
            filename,
            ..
        } => {
            assert_eq!(data, "iVBOR...");
            assert_eq!(media_type, "image/png");
            assert!(filename.is_none());
        }
        other => panic!("expected FileData for image output, got {other:?}"),
    }
}

#[test]
fn render_for_model_notebook_image_between_text_keeps_three_parts() {
    // Image breaks the merge chain. `[text-1, image, text-2]` must stay
    // as three parts — the image cleanly separates the text runs.
    let data = json!({
        "type": "notebook",
        "file": {
            "filePath": "/tmp/mixed.ipynb",
            "cells": [
                {
                    "cellType": "code",
                    "source": "plot()\n",
                    "cell_id": "cell-0",
                    "language": "python",
                    "outputs": [
                        {
                            "output_type": "display_data",
                            "image": {
                                "image_data": "AAAA",
                                "media_type": "image/png",
                            }
                        }
                    ],
                },
                {
                    "cellType": "code",
                    "source": "after()\n",
                    "cell_id": "cell-1",
                    "language": "python",
                },
            ],
        },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    assert_eq!(
        parts.len(),
        3,
        "expected [Text, FileData, Text], got {parts:?}"
    );
    assert!(matches!(parts[0], ToolResultContentPart::Text { .. }));
    assert!(matches!(parts[1], ToolResultContentPart::FileData { .. }));
    assert!(matches!(parts[2], ToolResultContentPart::Text { .. }));
}

#[test]
fn render_for_model_notebook_non_python_code_cell_emits_language_tag() {
    let data = json!({
        "type": "notebook",
        "file": {
            "filePath": "/tmp/r.ipynb",
            "cells": [
                {
                    "cellType": "code",
                    "source": "x <- 1",
                    "cell_id": "cell-0",
                    "language": "R",
                },
            ],
        },
    });
    let parts = <ReadTool as DynTool>::render_for_model(&ReadTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(
        text,
        "<cell id=\"cell-0\"><language>R</language>x <- 1</cell id=\"cell-0\">"
    );
}

// ---------------------------------------------------------------------------
// #24 — PDF pages parameter validation (errorCode 7/8)
// ---------------------------------------------------------------------------

#[test]
fn test_read_pages_validation() {
    use coco_tool_runtime::DynTool;
    let ctx = ToolUseContext::test_default();
    let validate = |pages: &str| {
        <ReadTool as DynTool>::validate_input(
            &ReadTool,
            &json!({"file_path": "/work/doc.pdf", "pages": pages}),
            &ctx,
        )
    };
    // Malformed → errorCode 7.
    match validate("abc") {
        coco_tool_runtime::ValidationResult::Invalid { error_code, .. } => {
            assert_eq!(error_code.as_deref(), Some("7"));
        }
        other => panic!("expected Invalid(7), got {other:?}"),
    }
    // Range wider than 20 → errorCode 8.
    match validate("1-25") {
        coco_tool_runtime::ValidationResult::Invalid { error_code, .. } => {
            assert_eq!(error_code.as_deref(), Some("8"));
        }
        other => panic!("expected Invalid(8), got {other:?}"),
    }
    // Open-ended "N-" is unbounded → errorCode 8.
    match validate("5-") {
        coco_tool_runtime::ValidationResult::Invalid { error_code, .. } => {
            assert_eq!(error_code.as_deref(), Some("8"));
        }
        other => panic!("expected Invalid(8), got {other:?}"),
    }
    // Valid in-range spec passes.
    assert!(matches!(
        validate("1-5"),
        coco_tool_runtime::ValidationResult::Valid
    ));
    assert!(matches!(
        validate("3"),
        coco_tool_runtime::ValidationResult::Valid
    ));
}
