//! Clipboard images for prompts (macOS: `osascript`), returned base64-encoded for ACP.
use anyhow::{Result, bail};

/// Reads a PNG from the system clipboard and returns it base64-encoded.
pub(super) fn read_png_b64() -> Result<String> {
    if !cfg!(target_os = "macos") {
        bail!("clipboard images are only supported on macOS");
    }
    let out = std::process::Command::new("osascript")
        .args(["-e", "the clipboard as «class PNGf»"])
        .output()?;
    if !out.status.success() {
        bail!("no image in the clipboard");
    }
    let png = parse_osascript_data(&String::from_utf8_lossy(&out.stdout))?;
    Ok(base64(&png))
}

/// Decodes osascript's `«data PNGf<hex>»` output.
fn parse_osascript_data(s: &str) -> Result<Vec<u8>> {
    let hex = s
        .trim()
        .strip_prefix("«data PNGf")
        .and_then(|h| h.strip_suffix('»'));
    let Some(hex) = hex.filter(|h| h.len() % 2 == 0) else {
        bail!("unexpected clipboard data");
    };
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
                .ok_or_else(|| anyhow::anyhow!("invalid clipboard data"))
        })
        .collect()
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_correctly() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn parses_osascript_hex() {
        assert_eq!(
            parse_osascript_data("«data PNGf89504E47»\n").unwrap(),
            [0x89, 0x50, 0x4e, 0x47]
        );
        assert!(parse_osascript_data("«data TIFF00»").is_err());
        assert!(parse_osascript_data("«data PNGf8»").is_err());
    }
}
