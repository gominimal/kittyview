// SPDX-License-Identifier: Apache-2.0

use crate::placeholder::{self, IdSpace};
use crate::terminal::{Mux, wrap_for_stack};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum bytes of base64 data per chunk (kitty protocol limit).
const CHUNK_SIZE: usize = 4096;

/// Image ID used for direct animations, which need an ID but not a stable one.
pub(crate) const DEFAULT_ANIMATION_ID: u32 = 1;

/// How an image is anchored to the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Placed at the cursor by the terminal (`a=T`).
    ///
    /// The terminal owns the position, so anything that redraws the screen
    /// without knowing about the image -- a multiplexer, most obviously --
    /// leaves it stranded where it was first drawn.
    Direct,
    /// Placed on a grid of Unicode placeholder cells (`U=1`).
    ///
    /// The image follows those cells as ordinary text, so it scrolls, clips
    /// and redraws correctly under multiplexers and pagers.
    Virtual { image_id: u32, cols: u16, rows: u16 },
}

impl Placement {
    /// The image ID this placement transmits under.
    fn image_id(self) -> u32 {
        match self {
            Placement::Direct => DEFAULT_ANIMATION_ID,
            Placement::Virtual { image_id, .. } => image_id,
        }
    }

    /// Header parameters for transmitting and placing the image.
    ///
    /// `q=2` suppresses the terminal's replies: nothing here reads them back,
    /// and under a multiplexer they would surface as stray input.
    fn transmit_header(self, with_id: bool) -> String {
        match self {
            Placement::Direct if with_id => {
                format!("a=T,f=100,i={DEFAULT_ANIMATION_ID},q=2")
            }
            Placement::Direct => "a=T,f=100,q=2".to_string(),
            Placement::Virtual {
                image_id,
                cols,
                rows,
            } => format!("a=T,f=100,i={image_id},U=1,c={cols},r={rows},q=2"),
        }
    }
}

/// Write a single APC sequence to the output buffer, wrapped for the mux stack.
fn write_apc(buf: &mut Vec<u8>, apc: &[u8], mux_stack: &[Mux]) {
    if mux_stack.is_empty() || mux_stack.iter().all(|m| matches!(m, Mux::Zellij(_))) {
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

/// Pick an image ID for this invocation.
///
/// Image IDs are global to the terminal session and shared with every other
/// program drawing into it, and nothing hands them out: transmitting under an
/// ID that is already in use replaces that image and drops the placements
/// drawing it, blanking an image that may still be sitting in scrollback. The
/// only defence is to draw at random from as much of the namespace as the
/// placeholder cells can carry, which is [`IdSpace::Full`] straight to the
/// terminal and [`IdSpace::MuxSafe`] through a multiplexer.
///
/// Any multiplexer in the stack forces the narrow space, not just the ones
/// needing passthrough: a multiplexer re-renders the cells it stores, so its
/// idea of what colours the outer terminal supports decides what is emitted.
pub fn pick_image_id(mux_stack: &[Mux]) -> u32 {
    let space = if mux_stack.is_empty() {
        IdSpace::Full
    } else {
        IdSpace::MuxSafe
    };
    space.id_at(random_index(space.capacity()))
}

/// A random index below `bound`, seeded by the operating system.
///
/// `RandomState` takes its keys from the OS random source, which makes it the
/// random number generator the standard library already ships: no extra
/// dependency and no per-platform code. The PID and the wall clock go in as
/// well, so two invocations still differ from each other if those keys ever
/// stopped being random.
fn random_index(bound: u32) -> u32 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    if let Ok(since_epoch) = SystemTime::now().duration_since(UNIX_EPOCH) {
        hasher.write_u128(since_epoch.as_nanos());
    }
    // Reduced below `bound`, so the result fits a u32 whatever `bound` is.
    (hasher.finish() % u64::from(bound)) as u32
}

/// Write the placeholder cells that anchor a virtual placement.
///
/// These are deliberately *not* wrapped for passthrough: they are ordinary
/// text, and the multiplexer has to see them to scroll the image with them.
fn write_placeholders(buf: &mut Vec<u8>, placement: Placement) {
    let Placement::Virtual {
        image_id,
        cols,
        rows,
    } = placement
    else {
        return;
    };
    let mut grid = String::new();
    placeholder::write_grid(&mut grid, image_id, cols, rows);
    buf.extend_from_slice(grid.as_bytes());
}

/// Transmit a PNG and create its virtual placement in one command
/// (`a=T,U=1`), without the placeholder grid; the caller positions the grid
/// rows itself.
///
/// A virtual placement shows nothing until placeholder cells reference it,
/// so this whole transmission is invisible -- which is what lets a
/// slideshow stream the next image while the previous slide is still on
/// screen, using the same single command as ordinary display rather than a
/// transmit-then-place split (`a=t` + `a=p`) that at least one
/// implementation in the wild renders blank.
/// The placement is created under an explicit `placement_id`, which the
/// placeholder cells repeat in their underline colour: that pins the cells
/// to exactly this placement, where an ID-less placement leaves the choice
/// to the terminal -- and Ghostty through 1.3.1 chooses in hash order when
/// the image ever carries a second virtual placement.
pub fn transmit_virtual(
    png_data: &[u8],
    out: &mut impl Write,
    mux_stack: &[Mux],
    image_id: u32,
    placement_id: u32,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(png_data.len() * 2);
    write_frame_data(
        &mut buf,
        png_data,
        &format!("a=T,f=100,i={image_id},U=1,c={cols},r={rows},p={placement_id},q=2"),
        mux_stack,
    )?;
    out.write_all(&buf)
}

/// Transmit an animation and create its virtual placement, without the
/// placeholder grid; the caller positions the grid rows itself.
///
/// The [`transmit_virtual`] reasoning extends to whole animations: the base
/// frame, the extra frames (`a=f`), and the loop start (`a=a`) all address
/// the image, and nothing becomes visible until placeholder cells reference
/// the placement -- so the entire sequence can go out while the previous
/// slide is still on screen.
pub fn transmit_animation_virtual(
    frames: &[(Vec<u8>, u32)],
    out: &mut impl Write,
    mux_stack: &[Mux],
    image_id: u32,
    placement_id: u32,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let Some((base, first_delay)) = frames.first() else {
        return Ok(());
    };
    if frames.len() == 1 {
        return transmit_virtual(base, out, mux_stack, image_id, placement_id, cols, rows);
    }

    let mut buf = Vec::new();
    write_frame_data(
        &mut buf,
        base,
        &format!("a=T,f=100,i={image_id},U=1,c={cols},r={rows},p={placement_id},q=2"),
        mux_stack,
    )?;
    for (i, (png_data, delay_ms)) in frames.iter().enumerate().skip(1) {
        let r = i + 1;
        write_frame_data(
            &mut buf,
            png_data,
            &format!("a=f,i={image_id},r={r},z={delay_ms},f=100,q=2"),
            mux_stack,
        )?;
    }
    let mut apc = Vec::new();
    write!(
        apc,
        "\x1b_Ga=a,i={image_id},r=1,z={first_delay},s=3,v=1,q=2;\x1b\\"
    )?;
    write_apc(&mut buf, &apc, mux_stack);
    out.write_all(&buf)
}

/// Display a single PNG at the cursor under an explicit ID (`a=T,i=`).
///
/// The one-phase fallback for targets where transmit-then-place is not
/// trusted; the ID is what lets the next slide delete this one.
pub fn display_png_with_id(
    png_data: &[u8],
    out: &mut impl Write,
    mux_stack: &[Mux],
    image_id: u32,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(png_data.len() * 2);
    write_frame_data(
        &mut buf,
        png_data,
        &format!("a=T,f=100,i={image_id},q=2"),
        mux_stack,
    )?;
    out.write_all(&buf)
}

/// Delete an image's placements (`d=i`); with `free_data`, its stored data
/// as well (`d=I`).
///
/// The case matters: lowercase keeps the data resident for redisplay,
/// uppercase gives the terminal its memory back. Both forms also remove
/// virtual placements, which no text erase and no `d=a`/`d=A` ever touches.
pub fn delete_image(
    out: &mut impl Write,
    mux_stack: &[Mux],
    image_id: u32,
    free_data: bool,
) -> io::Result<()> {
    let verb = if free_data { 'I' } else { 'i' };
    let mut buf = Vec::new();
    let mut apc = Vec::new();
    write!(apc, "\x1b_Ga=d,d={verb},i={image_id},q=2;\x1b\\")?;
    write_apc(&mut buf, &apc, mux_stack);
    out.write_all(&buf)
}

/// Delete every image visible on screen and free their data (`a=d,d=A`).
///
/// Exit hygiene: virtual placements are not covered (the protocol scopes
/// `d=A` to placements with a physical location), so the slideshow also
/// deletes its current image by ID.
pub fn delete_all(out: &mut impl Write, mux_stack: &[Mux]) -> io::Result<()> {
    let mut buf = Vec::new();
    write_apc(&mut buf, b"\x1b_Ga=d,d=A,q=2;\x1b\\", mux_stack);
    out.write_all(&buf)
}

/// Display a single PNG image via the kitty graphics protocol.
pub fn display_png(
    png_data: &[u8],
    out: &mut impl Write,
    mux_stack: &[Mux],
    placement: Placement,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(png_data.len() * 2);
    write_frame_data(
        &mut buf,
        png_data,
        &placement.transmit_header(false),
        mux_stack,
    )?;
    write_placeholders(&mut buf, placement);
    out.write_all(&buf)?;
    out.flush()
}

/// Display an animated image via the kitty graphics protocol.
/// Each frame is a `(png_bytes, delay_ms)` pair.
pub fn display_animation(
    frames: &[(Vec<u8>, u32)],
    out: &mut impl Write,
    mux_stack: &[Mux],
    placement: Placement,
) -> io::Result<()> {
    if frames.is_empty() {
        return Ok(());
    }
    if frames.len() == 1 {
        return display_png(&frames[0].0, out, mux_stack, placement);
    }

    let mut buf = Vec::new();
    let id = placement.image_id();

    // Base frame (frame 1)
    write_frame_data(
        &mut buf,
        &frames[0].0,
        &placement.transmit_header(true),
        mux_stack,
    )?;

    // Additional frames
    for (i, (png_data, delay_ms)) in frames.iter().enumerate().skip(1) {
        let r = i + 1;
        write_frame_data(
            &mut buf,
            png_data,
            &format!("a=f,i={id},r={r},z={delay_ms},f=100,q=2"),
            mux_stack,
        )?;
    }

    // Set frame 1's gap and start looping (s=3 = loop, v=1 = start)
    let first_delay = frames[0].1;
    let mut apc = Vec::new();
    write!(
        apc,
        "\x1b_Ga=a,i={id},r=1,z={first_delay},s=3,v=1,q=2;\x1b\\"
    )?;
    write_apc(&mut buf, &apc, mux_stack);

    write_placeholders(&mut buf, placement);

    out.write_all(&buf)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn single_chunk_format() {
        let data = b"tiny";
        let mut out = Vec::new();
        display_png(data, &mut out, &[], Placement::Direct).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,q=2;"));
        assert!(output.ends_with("\x1b\\"));
        assert!(!output.contains(",m="));
    }

    #[test]
    fn multi_chunk_format() {
        let data = vec![0xABu8; 4000];
        let mut out = Vec::new();
        display_png(&data, &mut out, &[], Placement::Direct).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,q=2,m=1;"));
        assert!(output.contains("m=0;"));
        assert!(output.matches("\x1b_G").count() >= 2);
    }

    #[test]
    fn all_chunks_terminated() {
        let data = vec![0u8; 8000];
        let mut out = Vec::new();
        display_png(&data, &mut out, &[], Placement::Direct).unwrap();
        let output = String::from_utf8(out).unwrap();
        let starts = output.matches("\x1b_G").count();
        let ends = output.matches("\x1b\\").count();
        assert_eq!(starts, ends, "every APC start must have a matching ST");
    }

    #[test]
    fn payload_is_valid_base64() {
        let data = b"hello kitty";
        let mut out = Vec::new();
        display_png(data, &mut out, &[], Placement::Direct).unwrap();
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
        display_png(b"", &mut out, &[], Placement::Direct).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,q=2;"));
        assert!(output.ends_with("\x1b\\"));
    }

    #[test]
    fn animation_single_frame_delegates_to_display_png() {
        let frame = vec![(b"single".to_vec(), 100u32)];
        let mut out = Vec::new();
        display_animation(&frame, &mut out, &[], Placement::Direct).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,q=2;"));
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
        display_animation(&frames, &mut out, &[], Placement::Direct).unwrap();
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
        display_png(data, &mut out, &stack, Placement::Direct).unwrap();
        assert!(out.starts_with(b"\x1bPtmux;"));
        assert!(out.ends_with(b"\x1b\\"));
        assert!(out.windows(3).any(|w| w == b"\x1b\x1b_"));
    }

    #[test]
    fn tmux_multi_chunk_each_wrapped() {
        let data = vec![0xABu8; 4000];
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None)];
        display_png(&data, &mut out, &stack, Placement::Direct).unwrap();
        let tmux_starts = out.windows(7).filter(|w| *w == b"\x1bPtmux;").count();
        assert!(tmux_starts >= 2, "each chunk must be independently wrapped");
    }

    #[test]
    fn tmux_animation_start_wrapped() {
        let frames = vec![(b"f1".to_vec(), 100u32), (b"f2".to_vec(), 150)];
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None)];
        display_animation(&frames, &mut out, &stack, Placement::Direct).unwrap();
        let tmux_starts = out.windows(7).filter(|w| *w == b"\x1bPtmux;").count();
        assert!(tmux_starts >= 3, "animation start APC must also be wrapped");
    }

    #[test]
    fn screen_single_chunk_wrapped() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Screen(None)];
        display_png(data, &mut out, &stack, Placement::Direct).unwrap();
        assert!(out.starts_with(b"\x1bP"));
        assert!(!out.starts_with(b"\x1bPtmux;"));
        assert!(out.windows(2).any(|w| w == b"\x1b_"));
    }

    #[test]
    fn no_mux_passthrough() {
        let data = b"tiny";
        let mut out = Vec::new();
        display_png(data, &mut out, &[], Placement::Direct).unwrap();
        assert!(!out.starts_with(b"\x1bP"));
        assert!(out.starts_with(b"\x1b_G"));
    }

    #[test]
    fn zellij_no_wrapping() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Zellij(None)];
        display_png(data, &mut out, &stack, Placement::Direct).unwrap();
        assert!(!out.starts_with(b"\x1bP"));
        assert!(out.starts_with(b"\x1b_G"));
    }

    #[test]
    fn double_tmux_wrapping() {
        let data = b"tiny";
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None), Mux::Tmux(None)];
        display_png(data, &mut out, &stack, Placement::Direct).unwrap();
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
        display_png(data, &mut out, &stack, Placement::Direct).unwrap();
        // Outer screen wrapper
        assert!(out.starts_with(b"\x1bP"));
        assert!(!out.starts_with(b"\x1bPtmux;"));
        // Inner tmux wrapper should be present inside the screen DCS
        assert!(out.windows(7).any(|w| w == b"\x1bPtmux;"));
    }

    // ── virtual placement (Unicode placeholder) tests ───────

    fn virt(cols: u16, rows: u16) -> Placement {
        Placement::Virtual {
            image_id: 42,
            cols,
            rows,
        }
    }

    #[test]
    fn virtual_header_declares_the_cell_rectangle() {
        let mut out = Vec::new();
        display_png(b"tiny", &mut out, &[], virt(4, 2)).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,i=42,U=1,c=4,r=2,q=2;"));
    }

    #[test]
    fn virtual_placement_appends_the_placeholder_grid() {
        let mut out = Vec::new();
        display_png(b"tiny", &mut out, &[], virt(4, 2)).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert_eq!(output.matches(placeholder::PLACEHOLDER).count(), 8);
        // The grid follows the image data, not the other way round.
        let apc_end = output.rfind("\x1b\\").unwrap();
        let first_cell = output.find(placeholder::PLACEHOLDER).unwrap();
        assert!(first_cell > apc_end);
    }

    #[test]
    fn direct_placement_emits_no_placeholders() {
        let mut out = Vec::new();
        display_png(b"tiny", &mut out, &[], Placement::Direct).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(!output.contains(placeholder::PLACEHOLDER));
        assert!(!output.contains("U=1"));
    }

    #[test]
    fn placeholder_grid_is_never_passthrough_wrapped() {
        // The multiplexer has to see these cells as text -- wrapping them in a
        // DCS envelope would hide them and the image would stop scrolling.
        let mut out = Vec::new();
        let stack = [Mux::Tmux(None)];
        display_png(b"tiny", &mut out, &stack, virt(3, 1)).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("\x1b[38;5;42m"));
        assert!(
            !output.contains("\x1b\x1b[38;5;42m"),
            "grid must not be ESC-doubled"
        );
        assert!(output.ends_with("\x1b[39m"));
        // The image data itself is still wrapped.
        assert!(output.starts_with("\x1bPtmux;"));
    }

    #[test]
    fn virtual_animation_places_and_anchors_once() {
        let frames = vec![(b"f1".to_vec(), 100u32), (b"f2".to_vec(), 150)];
        let mut out = Vec::new();
        display_animation(&frames, &mut out, &[], virt(2, 2)).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("a=T,f=100,i=42,U=1,c=2,r=2,q=2"));
        assert!(output.contains("a=f,i=42,r=2,z=150"));
        assert!(output.contains("a=a,i=42,r=1,z=100,s=3,v=1"));
        // One grid, emitted after the animation is started.
        assert_eq!(output.matches(placeholder::PLACEHOLDER).count(), 4);
        let grid_start = output.find(placeholder::PLACEHOLDER).unwrap();
        assert!(grid_start > output.find("a=a,i=42").unwrap());
    }

    #[test]
    fn image_ids_under_a_multiplexer_keep_the_colour_a_multiplexer_relays() {
        for mux in [Mux::Tmux(None), Mux::Screen(None), Mux::Zellij(None)] {
            for _ in 0..64 {
                let id = pick_image_id(std::slice::from_ref(&mux));
                assert_ne!(id, 0, "{mux:?}");
                assert!(id & 0x00FF_FFFF <= 0xFF, "{mux:?} got {id:#x}");
            }
        }
    }

    #[test]
    fn image_ids_without_a_multiplexer_span_the_full_range() {
        // Every ID is usable, so the only invariant is that none is zero --
        // that the wide space is actually reached is covered by the spread.
        let ids: HashSet<u32> = (0..256).map(|_| pick_image_id(&[])).collect();
        assert!(ids.iter().all(|&id| id != 0));
        assert!(
            ids.iter().any(|&id| id & 0x00FF_FFFF > 0xFF),
            "no ID needed more than the low byte"
        );
    }

    #[test]
    fn image_ids_do_not_repeat_across_invocations() {
        // 64 draws from the full space collide with probability ~5e-7, so the
        // one-duplicate slack keeps this from ever being the flaky test.
        let ids: HashSet<u32> = (0..64).map(|_| pick_image_id(&[])).collect();
        assert!(ids.len() >= 63, "got {} distinct IDs of 64", ids.len());
    }

    #[test]
    fn random_index_stays_below_its_bound() {
        for bound in [1, 2, 255, IdSpace::MuxSafe.capacity(), u32::MAX] {
            for _ in 0..64 {
                let n = random_index(bound);
                assert!(n < bound, "{n} not below {bound}");
            }
        }
    }

    #[test]
    fn quiet_mode_is_always_requested() {
        // Replies would be read as input by whatever is hosting us.
        for placement in [Placement::Direct, virt(2, 2)] {
            let mut out = Vec::new();
            display_png(b"tiny", &mut out, &[], placement).unwrap();
            let output = String::from_utf8(out).unwrap();
            assert!(output.contains("q=2"), "{placement:?}");
        }
    }

    // ── two-phase slideshow emitters ────────────────────────

    #[test]
    fn virtual_transmit_combines_placement_and_omits_the_grid() {
        let mut out = Vec::new();
        transmit_virtual(b"tiny", &mut out, &[], 42, 1, 4, 2).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,i=42,U=1,c=4,r=2,p=1,q=2;"));
        assert!(
            !output.contains(placeholder::PLACEHOLDER),
            "the caller positions the grid itself"
        );
    }

    #[test]
    fn virtual_animation_transmit_carries_the_whole_sequence_gridless() {
        let frames = vec![(b"f1".to_vec(), 100u32), (b"f2".to_vec(), 150)];
        let mut out = Vec::new();
        transmit_animation_virtual(&frames, &mut out, &[], 42, 1, 4, 2).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,i=42,U=1,c=4,r=2,p=1,q=2;"));
        assert!(output.contains("a=f,i=42,r=2,z=150"));
        assert!(output.contains("a=a,i=42,r=1,z=100,s=3,v=1"));
        assert!(
            !output.contains(placeholder::PLACEHOLDER),
            "the caller positions the grid itself"
        );
    }

    #[test]
    fn virtual_animation_transmit_with_one_frame_is_a_plain_transmit() {
        let frames = vec![(b"only".to_vec(), 100u32)];
        let mut out = Vec::new();
        transmit_animation_virtual(&frames, &mut out, &[], 42, 1, 4, 2).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,i=42,U=1,c=4,r=2,p=1,q=2;"));
        assert!(!output.contains("a=f,"));
        assert!(!output.contains("a=a,"));
    }

    #[test]
    fn virtual_transmit_chunks_large_images_like_display() {
        let data = vec![0xABu8; 8000];
        let mut out = Vec::new();
        transmit_virtual(&data, &mut out, &[], 7, 1, 4, 2).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,i=7,U=1,c=4,r=2,p=1,q=2,m=1;"));
        let starts = output.matches("\x1b_G").count();
        let ends = output.matches("\x1b\\").count();
        assert_eq!(starts, ends);
    }

    #[test]
    fn display_with_id_places_at_cursor_deletably() {
        let mut out = Vec::new();
        display_png_with_id(b"tiny", &mut out, &[], 9).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,f=100,i=9,q=2;"));
    }

    #[test]
    fn delete_case_decides_whether_data_is_freed() {
        let mut kept = Vec::new();
        delete_image(&mut kept, &[], 7, false).unwrap();
        assert_eq!(kept, b"\x1b_Ga=d,d=i,i=7,q=2;\x1b\\");

        let mut freed = Vec::new();
        delete_image(&mut freed, &[], 7, true).unwrap();
        assert_eq!(freed, b"\x1b_Ga=d,d=I,i=7,q=2;\x1b\\");
    }

    #[test]
    fn delete_all_frees_everything_visible() {
        let mut out = Vec::new();
        delete_all(&mut out, &[]).unwrap();
        assert_eq!(out, b"\x1b_Ga=d,d=A,q=2;\x1b\\");
    }

    #[test]
    fn slideshow_emitters_are_passthrough_wrapped() {
        let stack = [Mux::Tmux(None)];
        let mut out = Vec::new();
        transmit_virtual(b"tiny", &mut out, &stack, 7, 1, 2, 2).unwrap();
        display_png_with_id(b"tiny", &mut out, &stack, 7).unwrap();
        delete_image(&mut out, &stack, 7, true).unwrap();
        delete_all(&mut out, &stack).unwrap();
        let wraps = out.windows(7).filter(|w| *w == b"\x1bPtmux;").count();
        assert_eq!(wraps, 4, "every APC must be independently wrapped");
    }
}
