// SPDX-License-Identifier: Apache-2.0

use crate::terminal::{Mux, wrap_for_stack};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::io::{self, Write};

/// Maximum bytes of base64 data per chunk (kitty protocol limit).
const CHUNK_SIZE: usize = 4096;

/// Write a single APC sequence to the output buffer, wrapped for the mux stack.
fn write_apc(buf: &mut Vec<u8>, apc: &[u8], mux_stack: &[Mux]) {
    if mux_stack.is_empty() || mux_stack.iter().all(|m| matches!(m, Mux::Zellij)) {
        buf.extend_from_slice(apc);
    } else {
        buf.extend_from_slice(&wrap_for_stack(apc, mux_stack));
    }
}

/// Write a single image's base64-encoded data with the given header parameters.
/// Handles chunking transparently. Each APC chunk is individually wrapped for
/// the multiplexer stack.
fn write_frame_data(
    buf: &mut Vec<u8>,
    png_data: &[u8],
    header: &str,
    mux_stack: &[Mux],
) -> io::Result<()> {
    let encoded = STANDARD.encode(png_data);
    let bytes = encoded.as_bytes();

    if bytes.len() <= CHUNK_SIZE {
        let mut apc = Vec::new();
        write!(apc, "\x1b_G{header};{encoded}\x1b\\")?;
        write_apc(buf, &apc, mux_stack);
    } else {
        let mut offset = 0;
        let mut first = true;

        while offset < bytes.len() {
            let end = (offset + CHUNK_SIZE).min(bytes.len());
            let chunk = &encoded[offset..end];
            let is_last = end == bytes.len();

            let mut apc = Vec::new();
            if first {
                write!(apc, "\x1b_G{header},m={};{chunk}\x1b\\", u8::from(!is_last))?;
                first = false;
            } else {
                write!(apc, "\x1b_Gm={};{chunk}\x1b\\", u8::from(!is_last))?;
            }
            write_apc(buf, &apc, mux_stack);
            offset = end;
        }
    }
    Ok(())
}

/// Display a single PNG image via the kitty graphics protocol.
pub fn display_png(
    png_data: &[u8],
    out: &mut impl Write,
    mux_stack: &[Mux],
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(png_data.len() * 2);
    write_frame_data(&mut buf, png_data, "a=T,f=100", mux_stack)?;
    out.write_all(&buf)?;
    out.flush()
}

/// Display an animated image via the kitty graphics protocol.
/// Each frame is a `(png_bytes, delay_ms)` pair.
pub fn display_animation(
    frames: &[(Vec<u8>, u32)],
    out: &mut impl Write,
    mux_stack: &[Mux],
) -> io::Result<()> {
    if frames.is_empty() {
        return Ok(());
    }
    if frames.len() == 1 {
        return display_png(&frames[0].0, out, mux_stack);
    }

    let mut buf = Vec::new();
    const ID: u32 = 1;

    // Base frame (frame 1)
    write_frame_data(&mut buf, &frames[0].0, &format!("a=T,f=100,i={ID},q=2"), mux_stack)?;

    // Additional frames
    for (i, (png_data, delay_ms)) in frames.iter().enumerate().skip(1) {
        let r = i + 1;
        write_frame_data(
            &mut buf,
            png_data,
            &format!("a=f,i={ID},r={r},z={delay_ms},f=100,q=2"),
            mux_stack,
        )?;
    }

    // Set frame 1's gap and start looping (s=3 = loop, v=1 = start)
    let first_delay = frames[0].1;
    let mut apc = Vec::new();
    write!(
        apc,
        "\x1b_Ga=a,i={ID},r=1,z={first_delay},s=3,v=1,q=2;\x1b\\"
    )?;
    write_apc(&mut buf, &apc, mux_stack);

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
        display_png(data, &mut out, &[]).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100;"));
        assert!(output.ends_with("\x1b\\"));
        assert!(!output.contains(",m="));
    }

    #[test]
    fn multi_chunk_format() {
        let data = vec![0xABu8; 4000];
        let mut out = Vec::new();
        display_png(&data, &mut out, &[]).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,m=1;"));
        assert!(output.contains("m=0;"));
        assert!(output.matches("\x1b_G").count() >= 2);
    }

    #[test]
    fn all_chunks_terminated() {
        let data = vec![0u8; 8000];
        let mut out = Vec::new();
        display_png(&data, &mut out, &[]).unwrap();
        let output = String::from_utf8(out).unwrap();
        let starts = output.matches("\x1b_G").count();
        let ends = output.matches("\x1b\\").count();
        assert_eq!(starts, ends, "every APC start must have a matching ST");
    }

    #[test]
    fn payload_is_valid_base64() {
        let data = b"hello kitty";
        let mut out = Vec::new();
        display_png(data, &mut out, &[]).unwrap();
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
        display_png(b"", &mut out, &[]).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100;"));
        assert!(output.ends_with("\x1b\\"));
    }

    #[test]
    fn animation_single_frame_delegates_to_display_png() {
        let frame = vec![(b"single".to_vec(), 100u32)];
        let mut out = Vec::new();
        display_animation(&frame, &mut out, &[]).unwrap();
        let output = String::from_utf8(out).unwrap();
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
        display_animation(&frames, &mut out, &[]).unwrap();
        let output = String::from_utf8(out).unwrap();

        assert!(output.contains("a=T,f=100,i=1"));
        assert!(output.contains("a=f,i=1,r=2,z=150"));
        assert!(output.contains("a=f,i=1,r=3,z=200"));
        assert!(output.contains("a=a,i=1,r=1,z=100,s=3,v=1"));
    }

    // ── tmux wrapping tests ─────────────────────────────────

    #[test]
    fn tmux_single_chunk_wrapped() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None)];
        display_png(data, &mut out, &stack).unwrap();
        assert!(out.starts_with(b"\x1bPtmux;"));
        assert!(out.ends_with(b"\x1b\\"));
        assert!(out.windows(3).any(|w| w == b"\x1b\x1b_"));
    }

    #[test]
    fn tmux_multi_chunk_each_wrapped() {
        let data = vec![0xABu8; 4000];
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None)];
        display_png(&data, &mut out, &stack).unwrap();
        let tmux_starts = out.windows(7).filter(|w| *w == b"\x1bPtmux;").count();
        assert!(tmux_starts >= 2, "each chunk must be independently wrapped");
    }

    #[test]
    fn tmux_animation_start_wrapped() {
        let frames = vec![
            (b"f1".to_vec(), 100u32),
            (b"f2".to_vec(), 150),
        ];
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None)];
        display_animation(&frames, &mut out, &stack).unwrap();
        let tmux_starts = out.windows(7).filter(|w| *w == b"\x1bPtmux;").count();
        assert!(tmux_starts >= 3, "animation start APC must also be wrapped");
    }

    #[test]
    fn screen_single_chunk_wrapped() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Screen(None)];
        display_png(data, &mut out, &stack).unwrap();
        assert!(out.starts_with(b"\x1bP"));
        assert!(!out.starts_with(b"\x1bPtmux;"));
        assert!(out.windows(2).any(|w| w == b"\x1b_"));
    }

    #[test]
    fn no_mux_passthrough() {
        let data = b"tiny";
        let mut out = Vec::new();
        display_png(data, &mut out, &[]).unwrap();
        assert!(!out.starts_with(b"\x1bP"));
        assert!(out.starts_with(b"\x1b_G"));
    }

    #[test]
    fn zellij_no_wrapping() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Zellij];
        display_png(data, &mut out, &stack).unwrap();
        assert!(!out.starts_with(b"\x1bP"));
        assert!(out.starts_with(b"\x1b_G"));
    }

    #[test]
    fn double_tmux_wrapping() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None), Mux::Tmux(None)];
        display_png(data, &mut out, &stack).unwrap();
        // Outer tmux wrapper
        assert!(out.starts_with(b"\x1bPtmux;"));
        // Should contain a nested tmux wrapper (ESC P tmux; with doubled ESC)
        assert!(out.windows(8).any(|w| w == b"\x1b\x1bPtmux;"));
    }

    #[test]
    fn tmux_in_screen_wrapping() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None), Mux::Screen(None)];
        display_png(data, &mut out, &stack).unwrap();
        // Outer screen wrapper
        assert!(out.starts_with(b"\x1bP"));
        assert!(!out.starts_with(b"\x1bPtmux;"));
        // Inner tmux wrapper should be present inside the screen DCS
        assert!(out.windows(7).any(|w| w == b"\x1bPtmux;"));
    }
}
