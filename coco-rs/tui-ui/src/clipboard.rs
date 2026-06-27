//! Platform-specific clipboard image capture.
//!
//! NSPasteboard (macOS native), xclip/wl-paste (Linux), PowerShell (Windows).

use crate::paste::ImageData;

/// Attempt to read an image from the system clipboard.
///
/// Returns `Ok(None)` if no image is available or the clipboard tool is not found.
/// Uses platform-specific tools: `xclip`/`wl-paste` on Linux, `osascript` on macOS.
pub async fn read_clipboard_image() -> std::io::Result<Option<ImageData>> {
    #[cfg(target_os = "linux")]
    {
        read_clipboard_image_linux().await
    }
    #[cfg(target_os = "macos")]
    {
        read_clipboard_image_macos().await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Ok(None)
    }
}

/// Check if clipboard tools are available on this platform.
pub async fn has_clipboard_image_support() -> bool {
    #[cfg(target_os = "linux")]
    {
        which_exists("xclip").await || which_exists("wl-paste").await
    }
    #[cfg(target_os = "macos")]
    {
        true // osascript is always available on macOS
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

// -- Linux implementation --

#[cfg(target_os = "linux")]
async fn read_clipboard_image_linux() -> std::io::Result<Option<ImageData>> {
    // Try xclip first (X11)
    if which_exists("xclip").await {
        // Check if clipboard contains image data
        let targets = tokio::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
            .output()
            .await;
        if let Ok(output) = targets {
            let targets_str = String::from_utf8_lossy(&output.stdout);
            if targets_str.contains("image/png") {
                let img = tokio::process::Command::new("xclip")
                    .args(["-selection", "clipboard", "-t", "image/png", "-o"])
                    .output()
                    .await?;
                if img.status.success() && !img.stdout.is_empty() {
                    return Ok(Some(ImageData {
                        bytes: img.stdout,
                        mime: "image/png".to_string(),
                    }));
                }
            }
        }
    }

    // Fallback to wl-paste (Wayland)
    if which_exists("wl-paste").await {
        let img = tokio::process::Command::new("wl-paste")
            .args(["--type", "image/png"])
            .output()
            .await?;
        if img.status.success() && !img.stdout.is_empty() {
            return Ok(Some(ImageData {
                bytes: img.stdout,
                mime: "image/png".to_string(),
            }));
        }
    }

    Ok(None)
}

// -- macOS implementation --

#[cfg(target_os = "macos")]
async fn read_clipboard_image_macos() -> std::io::Result<Option<ImageData>> {
    // Use osascript to check for image in clipboard
    let check = tokio::process::Command::new("osascript")
        .args(["-e", "clipboard info for (clipboard info)"])
        .output()
        .await?;
    let info = String::from_utf8_lossy(&check.stdout);
    if !info.contains("«class PNGf»") && !info.contains("PNGf") {
        return Ok(None);
    }

    // Save clipboard image to temp file and read it
    let tmp = std::env::temp_dir().join(format!("coco-clip-{}.png", std::process::id()));
    let tmp_str = tmp.display().to_string();
    let script = format!(
        "set theFile to POSIX file \"{tmp_str}\"
set fh to open for access theFile with write permission
write (the clipboard as «class PNGf») to fh
close access fh"
    );
    let result = tokio::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .await?;

    if !result.status.success() {
        return Ok(None);
    }

    let bytes = tokio::fs::read(&tmp).await?;
    let _ = tokio::fs::remove_file(&tmp).await;

    if bytes.is_empty() {
        return Ok(None);
    }

    Ok(Some(ImageData {
        bytes,
        mime: "image/png".to_string(),
    }))
}

/// Check if a command exists on PATH.
#[allow(dead_code)]
async fn which_exists(cmd: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(cmd)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "clipboard.test.rs"]
mod tests;
