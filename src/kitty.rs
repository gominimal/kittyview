// SPDX-License-Identifier: Apache-2.0

use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::io::{self, Write};

/// Maximum bytes of base64 data per chunk (kitty protocol limit).
const CHUNK_SIZE: usize = 4096;

/// Check whether the terminal likely supports the kitty graphics protocol,
/// based on environment variables set by known-compatible terminals.
pub fn is_supported() -> bool {
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    if std::env::var_os("KONSOLE_VERSION").is_some() {
        return true;
    }
    if let Some(prog) = std::env::var_os("TERM_PROGRAM") {
        let prog = prog.to_string_lossy();
        if matches!(
            prog.as_ref(),
            "kitty" | "WezTerm" | "ghostty" | "Ghostty" | "iTerm.app" | "iTerm2"
        ) {
            return true;
        }
    }
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

/// Write a single image's base64-encoded data with the given header parameters.
/// Handles chunking transparently.
fn write_frame_data(buf: &mut Vec<u8>, png_data: &[u8], header: &str) -> io::Result<()> {
    let encoded = STANDARD.encode(png_data);
    let bytes = encoded.as_bytes();

    if bytes.len() <= CHUNK_SIZE {
        write!(buf, "\x1b_G{header};{encoded}\x1b\\")?;
    } else {
        let mut offset = 0;
        let mut first = true;

        while offset < bytes.len() {
            let end = (offset + CHUNK_SIZE).min(bytes.len());
            let chunk = &encoded[offset..end];
            let is_last = end == bytes.len();

            if first {
                write!(buf, "\x1b_G{header},m={};{chunk}\x1b\\", u8::from(!is_last))?;
                first = false;
            } else {
                write!(buf, "\x1b_Gm={};{chunk}\x1b\\", u8::from(!is_last))?;
            }
            offset = end;
        }
    }
    Ok(())
}

/// Display a single PNG image via the kitty graphics protocol.
pub fn display_png(png_data: &[u8], out: &mut impl Write) -> io::Result<()> {
    let mut buf = Vec::with_capacity(png_data.len() * 2);
    write_frame_data(&mut buf, png_data, "a=T,f=100")?;
    out.write_all(&buf)?;
    out.flush()
}

/// Display an animated image via the kitty graphics protocol.
/// Each frame is a `(png_bytes, delay_ms)` pair.
pub fn display_animation(frames: &[(Vec<u8>, u32)], out: &mut impl Write) -> io::Result<()> {
    if frames.is_empty() {
        return Ok(());
    }
    if frames.len() == 1 {
        return display_png(&frames[0].0, out);
    }

    let mut buf = Vec::new();
    const ID: u32 = 1;

    // Base frame (frame 1)
    write_frame_data(&mut buf, &frames[0].0, &format!("a=T,f=100,i={ID},q=2"))?;

    // Additional frames
    for (i, (png_data, delay_ms)) in frames.iter().enumerate().skip(1) {
        let r = i + 1;
        write_frame_data(
            &mut buf,
            png_data,
            &format!("a=f,i={ID},r={r},z={delay_ms},f=100,q=2"),
        )?;
    }

    // Set frame 1's gap and start looping (s=3 = loop, v=1 = start)
    let first_delay = frames[0].1;
    write!(
        buf,
        "\x1b_Ga=a,i={ID},r=1,z={first_delay},s=3,v=1,q=2;\x1b\\"
    )?;

    out.write_all(&buf)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_chunk_format() {
        let data = b"tiny";
        let mut out = Vec::new();
        display_png(data, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100;"));
        assert!(output.ends_with("\x1b\\"));
        assert!(!output.contains(",m="));
    }

    #[test]
    fn multi_chunk_format() {
        let data = vec![0xABu8; 4000];
        let mut out = Vec::new();
        display_png(&data, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,m=1;"));
        assert!(output.contains("m=0;"));
        assert!(output.matches("\x1b_G").count() >= 2);
    }

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

    #[test]
    fn payload_is_valid_base64() {
        let data = b"hello kitty";
        let mut out = Vec::new();
        display_png(data, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();
        let payload_start = output.find(';').unwrap() + 1;
        let payload_end = output.rfind("\x1b\\").unwrap();
        let payload = &output[payload_start..payload_end];
        let decoded = STANDARD
            .decode(payload)
            .expect("payload should be valid base64");
        assert_eq!(decoded, data);
    }

    #[test]
    fn empty_input() {
        let mut out = Vec::new();
        display_png(b"", &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100;"));
        assert!(output.ends_with("\x1b\\"));
    }

    #[test]
    fn animation_single_frame_delegates_to_display_png() {
        let frame = vec![(b"single".to_vec(), 100u32)];
        let mut out = Vec::new();
        display_animation(&frame, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();
        // Should be a plain single image, no animation commands
        assert!(output.starts_with("\x1b_Ga=T,f=100;"));
        assert!(!output.contains("a=f"));
        assert!(!output.contains("a=a"));
    }

    #[test]
    fn animation_multi_frame_structure() {
        let frames = vec![
            (b"frame1".to_vec(), 100u32),
            (b"frame2".to_vec(), 150),
            (b"frame3".to_vec(), 200),
        ];
        let mut out = Vec::new();
        display_animation(&frames, &mut out).unwrap();
        let output = String::from_utf8(out).unwrap();

        // Base frame with image ID
        assert!(output.contains("a=T,f=100,i=1"));
        // Additional frames
        assert!(output.contains("a=f,i=1,r=2,z=150"));
        assert!(output.contains("a=f,i=1,r=3,z=200"));
        // Animation start command with frame 1 gap and loop
        assert!(output.contains("a=a,i=1,r=1,z=100,s=3,v=1"));
    }
}
