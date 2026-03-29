// SPDX-License-Identifier: Apache-2.0

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::io::{self, Write};

/// Maximum bytes of base64 data per chunk (kitty protocol limit).
const CHUNK_SIZE: usize = 4096;

/// Check whether the terminal likely supports the kitty graphics protocol,
/// based on environment variables set by known-compatible terminals.
pub fn is_supported() -> bool {
    // Kitty sets this
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }

    // Konsole sets this
    if std::env::var_os("KONSOLE_VERSION").is_some() {
        return true;
    }

    // Many terminals set TERM_PROGRAM
    if let Some(prog) = std::env::var_os("TERM_PROGRAM") {
        let prog = prog.to_string_lossy();
        if matches!(
            prog.as_ref(),
            "kitty" | "WezTerm" | "ghostty" | "Ghostty" | "iTerm.app" | "iTerm2"
        ) {
            return true;
        }
    }

    // TERM often encodes the terminal name, e.g. xterm-kitty, xterm-ghostty
    if let Some(term) = std::env::var_os("TERM") {
        let term = term.to_string_lossy();
        let term = term.to_ascii_lowercase();
        if ["kitty", "ghostty", "wezterm"]
            .iter()
            .any(|t| term.contains(t))
        {
            return true;
        }
    }

    false
}

/// Build the full kitty graphics protocol output into a buffer, then write
/// it all at once. This minimizes the window for partial output if the
/// process is interrupted mid-write.
pub fn display_png(png_data: &[u8], out: &mut impl Write) -> io::Result<()> {
    let encoded = STANDARD.encode(png_data);
    let bytes = encoded.as_bytes();

    // Build the entire escape sequence output in memory first
    let mut buf = Vec::with_capacity(encoded.len() + 256);

    if bytes.len() <= CHUNK_SIZE {
        write!(buf, "\x1b_Ga=T,f=100;{encoded}\x1b\\")?;
    } else {
        let mut offset = 0;
        let mut first = true;

        while offset < bytes.len() {
            let end = (offset + CHUNK_SIZE).min(bytes.len());
            let chunk = &encoded[offset..end];
            let is_last = end == bytes.len();

            if first {
                write!(
                    buf,
                    "\x1b_Ga=T,f=100,m={m};{chunk}\x1b\\",
                    m = u8::from(!is_last),
                )?;
                first = false;
            } else {
                write!(buf, "\x1b_Gm={m};{chunk}\x1b\\", m = u8::from(!is_last),)?;
            }

            offset = end;
        }
    }

    // Single write + flush to minimize partial-output window
    out.write_all(&buf)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify single-chunk output structure for small payloads.
    #[test]
    fn single_chunk_format() {
        let data = b"tiny";
        let mut out = Vec::new();
        display_png(data, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();

        assert!(output.starts_with("\x1b_Ga=T,f=100;"));
        assert!(output.ends_with("\x1b\\"));
        // Should NOT contain m= (no chunking needed)
        assert!(!output.contains(",m="));
    }

    /// Verify multi-chunk output for large payloads.
    #[test]
    fn multi_chunk_format() {
        // 4096 bytes of base64 = ~3072 bytes of input, so use 4000 bytes to force chunking
        let data = vec![0xABu8; 4000];
        let mut out = Vec::new();
        display_png(&data, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();

        // First chunk has a=T,f=100,m=1
        assert!(output.starts_with("\x1b_Ga=T,f=100,m=1;"));
        // Last chunk has m=0
        assert!(output.ends_with("m=0;\x1b\\") || output.contains("m=0;"));
        // Must have at least 2 APC sequences
        assert!(output.matches("\x1b_G").count() >= 2);
    }

    /// Every APC sequence must be properly terminated with ST.
    #[test]
    fn all_chunks_terminated() {
        let data = vec![0u8; 8000];
        let mut out = Vec::new();
        display_png(&data, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();

        let starts = output.matches("\x1b_G").count();
        let ends = output.matches("\x1b\\").count();
        assert_eq!(starts, ends, "every APC start must have a matching ST");
    }

    /// Verify base64 payload is valid.
    #[test]
    fn payload_is_valid_base64() {
        let data = b"hello kitty";
        let mut out = Vec::new();
        display_png(data, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();

        // Extract payload between ; and \x1b
        let payload_start = output.find(';').unwrap() + 1;
        let payload_end = output.rfind("\x1b\\").unwrap();
        let payload = &output[payload_start..payload_end];

        let decoded = STANDARD
            .decode(payload)
            .expect("payload should be valid base64");
        assert_eq!(decoded, data);
    }

    /// Empty input should still produce a valid (single-chunk) escape sequence.
    #[test]
    fn empty_input() {
        let mut out = Vec::new();
        display_png(b"", &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();

        assert!(output.starts_with("\x1b_Ga=T,f=100;"));
        assert!(output.ends_with("\x1b\\"));
    }
}
