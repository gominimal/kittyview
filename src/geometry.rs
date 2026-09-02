// SPDX-License-Identifier: Apache-2.0

//! Terminal geometry: window size in cells and the pixel size of one cell.
//!
//! Virtual placements are sized in cells, so an image's pixel dimensions have
//! to be converted into a cell rectangle before it can be displayed. That needs
//! the size of a cell in pixels, which is resolved from, in order of
//! preference:
//!
//! 1. `TIOCGWINSZ`, which most graphics-capable terminals populate with pixel
//!    dimensions (but a multiplexer's pty does not);
//! 2. tmux's own `client_cell_width` / `client_cell_height`, which tmux
//!    measures from the outer terminal;
//! 3. an `XTWINOPS` `CSI 16 t` query sent through the multiplexer stack;
//! 4. `CSI 14 t` (text area in pixels) divided by the size in cells;
//! 5. a conventional 8x16 default.
//!
//! A wrong cell size never distorts an image -- the terminal fits it to the
//! cell rectangle preserving aspect ratio -- it only makes it the wrong size,
//! so falling back to a plausible default is safe.

use crate::terminal::{Mux, wrap_for_stack};
use std::time::Duration;

/// Cell size assumed when nothing can be measured.
const DEFAULT_CELL_W: u32 = 8;
const DEFAULT_CELL_H: u32 = 16;

/// Window size assumed when `TIOCGWINSZ` is unavailable.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// How long to wait for an XTWINOPS reply. Kept short: a terminal that answers
/// at all answers immediately, and this runs on every display.
const QUERY_TIMEOUT: Duration = Duration::from_millis(400);

/// Terminal dimensions needed to place an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// Window (or pane) width in cells.
    pub cols: u16,
    /// Window (or pane) height in cells.
    pub rows: u16,
    /// Cell width in pixels.
    pub cell_w: u32,
    /// Cell height in pixels.
    pub cell_h: u32,
    /// Whether the cell size was measured rather than assumed.
    pub cell_size_measured: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            cell_w: DEFAULT_CELL_W,
            cell_h: DEFAULT_CELL_H,
            cell_size_measured: false,
        }
    }
}

/// Resolve the terminal geometry, querying through `mux_stack` if needed.
pub fn detect(mux_stack: &[Mux]) -> Geometry {
    let winsize = platform::winsize();
    let (cols, rows) = match winsize {
        Some((c, r, _, _)) if c > 0 && r > 0 => (c, r),
        _ => (DEFAULT_COLS, DEFAULT_ROWS),
    };

    // 1. Pixel dimensions straight from the pty.
    if let Some((c, r, xpixel, ypixel)) = winsize {
        if let Some((cell_w, cell_h)) = cell_from_pixels(u32::from(xpixel), u32::from(ypixel), c, r)
        {
            return Geometry {
                cols,
                rows,
                cell_w,
                cell_h,
                cell_size_measured: true,
            };
        }
    }

    // 2. Ask tmux, which already knows the outer terminal's cell size.
    if mux_stack.iter().any(|m| matches!(m, Mux::Tmux(_))) {
        if let Some((cell_w, cell_h)) = tmux_cell_size() {
            return Geometry {
                cols,
                rows,
                cell_w,
                cell_h,
                cell_size_measured: true,
            };
        }
    }

    // 3/4. Query the outer terminal through the multiplexer stack.
    if let Some((w, h)) = query_cell_size(mux_stack, cols, rows) {
        return Geometry {
            cols,
            rows,
            cell_w: w,
            cell_h: h,
            cell_size_measured: true,
        };
    }

    // 5. Give up and assume a conventional cell.
    Geometry {
        cols,
        rows,
        ..Geometry::default()
    }
}

/// Derive a cell size from a pixel area and a cell count, rejecting the zeroes
/// that pseudo-terminals and multiplexers report.
fn cell_from_pixels(xpixel: u32, ypixel: u32, cols: u16, rows: u16) -> Option<(u32, u32)> {
    if xpixel == 0 || ypixel == 0 || cols == 0 || rows == 0 {
        return None;
    }
    let w = xpixel / u32::from(cols);
    let h = ypixel / u32::from(rows);
    if w == 0 || h == 0 { None } else { Some((w, h)) }
}

/// Ask tmux for the outer terminal's cell size (tmux 3.4+).
fn tmux_cell_size() -> Option<(u32, u32)> {
    std::env::var_os("TMUX")?;
    let out = std::process::Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "#{client_cell_width} #{client_cell_height}",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let mut parts = text.split_whitespace();
    let w: u32 = parts.next()?.parse().ok()?;
    let h: u32 = parts.next()?.parse().ok()?;
    if w == 0 || h == 0 { None } else { Some((w, h)) }
}

/// Query the terminal for its cell size via XTWINOPS.
#[cfg(unix)]
fn query_cell_size(mux_stack: &[Mux], cols: u16, rows: u16) -> Option<(u32, u32)> {
    let mut session = crate::terminal::query_session()?;

    // CSI 16 t -> CSI 6 ; height ; width t (cell size in pixels).
    let req = wrap_for_stack(b"\x1b[16t", mux_stack);
    if let Some(resp) = session.ask(&req, QUERY_TIMEOUT) {
        if let Some((h, w)) = parse_xtwinops(&resp, 6) {
            if w > 0 && h > 0 {
                return Some((w, h));
            }
        }
    }

    // CSI 14 t -> CSI 4 ; height ; width t (text area in pixels).
    let req = wrap_for_stack(b"\x1b[14t", mux_stack);
    if let Some(resp) = session.ask(&req, QUERY_TIMEOUT) {
        if let Some((h, w)) = parse_xtwinops(&resp, 4) {
            return cell_from_pixels(w, h, cols, rows);
        }
    }

    None
}

#[cfg(not(unix))]
fn query_cell_size(_mux_stack: &[Mux], _cols: u16, _rows: u16) -> Option<(u32, u32)> {
    None
}

/// Parse an XTWINOPS report `CSI <kind> ; <height> ; <width> t`.
///
/// Returns `(height, width)` in the report's own order.
fn parse_xtwinops(response: &[u8], kind: u32) -> Option<(u32, u32)> {
    let start = response.windows(2).position(|w| w == b"\x1b[")? + 2;
    let end = start + response[start..].iter().position(|&b| b == b't')?;
    let params = std::str::from_utf8(&response[start..end]).ok()?;

    let mut parts = params.split(';');
    let reported: u32 = parts.next()?.trim().parse().ok()?;
    if reported != kind {
        return None;
    }
    let height: u32 = parts.next()?.trim().parse().ok()?;
    let width: u32 = parts.next()?.trim().parse().ok()?;
    Some((height, width))
}

/// The cell rectangle an image of `px_w` x `px_h` pixels should occupy.
///
/// Images are shown at their natural size where they fit, and scaled down to
/// the window width otherwise -- the same thing a pager or a browser does, and
/// a hard requirement under a multiplexer, where a grid wider than the pane
/// would wrap and shear the image.
pub fn image_cells(px_w: u32, px_h: u32, geom: &Geometry, max_cells: u16) -> (u16, u16) {
    if px_w == 0 || px_h == 0 {
        return (0, 0);
    }

    let cell_w = f64::from(geom.cell_w.max(1));
    let cell_h = f64::from(geom.cell_h.max(1));
    let (px_w, px_h) = (f64::from(px_w), f64::from(px_h));

    let mut cols = (px_w / cell_w).ceil().max(1.0);
    let mut rows = (px_h / cell_h).ceil().max(1.0);

    // Fit to the window width, then to whatever the placeholder encoding can
    // address, preserving the aspect ratio at each step.
    let limit_cols = f64::from(geom.cols.max(1).min(max_cells));
    if cols > limit_cols {
        rows = (rows * limit_cols / cols).round().max(1.0);
        cols = limit_cols;
    }
    let limit_rows = f64::from(max_cells);
    if rows > limit_rows {
        cols = (cols * limit_rows / rows).round().max(1.0);
        rows = limit_rows;
    }

    (cols as u16, rows as u16)
}

/// Read `width` and `height` from a PNG's IHDR chunk.
pub fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if data.len() < 24 || !data.starts_with(SIGNATURE) || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

// ─── Platform I/O ───────────────────────────────────────────

#[cfg(unix)]
mod platform {
    use std::io::{self, IsTerminal};
    use std::os::unix::io::{AsRawFd, RawFd};

    /// `(cols, rows, xpixel, ypixel)` from `TIOCGWINSZ`.
    pub fn winsize() -> Option<(u16, u16, u16, u16)> {
        if io::stdout().is_terminal() {
            if let Some(ws) = ioctl_winsize(io::stdout().as_raw_fd()) {
                return Some(ws);
            }
        }
        let tty = std::fs::File::open("/dev/tty").ok()?;
        ioctl_winsize(tty.as_raw_fd())
    }

    fn ioctl_winsize(fd: RawFd) -> Option<(u16, u16, u16, u16)> {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 {
            return None;
        }
        Some((ws.ws_col, ws.ws_row, ws.ws_xpixel, ws.ws_ypixel))
    }
}

#[cfg(not(unix))]
mod platform {
    pub fn winsize() -> Option<(u16, u16, u16, u16)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(cols: u16, rows: u16, cell_w: u32, cell_h: u32) -> Geometry {
        Geometry {
            cols,
            rows,
            cell_w,
            cell_h,
            cell_size_measured: true,
        }
    }

    #[test]
    fn cell_size_from_pty_pixels() {
        assert_eq!(cell_from_pixels(800, 400, 100, 25), Some((8, 16)));
    }

    #[test]
    fn cell_size_rejects_multiplexer_zeroes() {
        assert_eq!(cell_from_pixels(0, 0, 100, 25), None);
        assert_eq!(cell_from_pixels(800, 0, 100, 25), None);
        assert_eq!(cell_from_pixels(800, 400, 0, 25), None);
        // Sub-pixel cells are nonsense, not a measurement.
        assert_eq!(cell_from_pixels(50, 400, 100, 25), None);
    }

    #[test]
    fn xtwinops_cell_size_report() {
        assert_eq!(parse_xtwinops(b"\x1b[6;16;8t", 6), Some((16, 8)));
    }

    #[test]
    fn xtwinops_text_area_report() {
        assert_eq!(parse_xtwinops(b"\x1b[4;1080;1920t", 4), Some((1080, 1920)));
    }

    #[test]
    fn xtwinops_wrong_kind_rejected() {
        // A CSI 18 t (size in cells) reply must not be read as pixels.
        assert_eq!(parse_xtwinops(b"\x1b[8;24;80t", 6), None);
        assert_eq!(parse_xtwinops(b"\x1b[6;16;8t", 4), None);
    }

    #[test]
    fn xtwinops_garbage_rejected() {
        assert_eq!(parse_xtwinops(b"", 6), None);
        assert_eq!(parse_xtwinops(b"\x1b[6;16t", 6), None);
        assert_eq!(parse_xtwinops(b"\x1b[6;abc;8t", 6), None);
        assert_eq!(parse_xtwinops(b"nonsense", 6), None);
    }

    #[test]
    fn xtwinops_tolerates_leading_noise() {
        assert_eq!(parse_xtwinops(b"junk\x1b[6;16;8t", 6), Some((16, 8)));
    }

    #[test]
    fn natural_size_when_it_fits() {
        let g = geom(80, 24, 8, 16);
        assert_eq!(image_cells(160, 160, &g, 297), (20, 10));
    }

    #[test]
    fn partial_cells_round_up() {
        let g = geom(80, 24, 8, 16);
        assert_eq!(image_cells(1, 1, &g, 297), (1, 1));
        assert_eq!(image_cells(9, 17, &g, 297), (2, 2));
    }

    #[test]
    fn wide_images_scale_down_to_the_window() {
        let g = geom(80, 24, 8, 16);
        // 1600x800 is 200x50 cells naturally; capped at 80 columns it keeps
        // its aspect ratio: 50 * 80/200 = 20 rows.
        assert_eq!(image_cells(1600, 800, &g, 297), (80, 20));
    }

    #[test]
    fn tall_images_are_not_capped_to_the_window_height() {
        let g = geom(80, 24, 8, 16);
        // Taller than the window is fine -- it scrolls, like any long output.
        let (cols, rows) = image_cells(80, 3200, &g, 297);
        assert_eq!((cols, rows), (10, 200));
    }

    #[test]
    fn dimensions_stay_within_the_encodable_range() {
        let g = geom(2000, 24, 8, 16);
        let (cols, rows) = image_cells(40_000, 80_000, &g, 297);
        assert!(cols <= 297 && rows <= 297, "got {cols}x{rows}");
        assert!(cols >= 1 && rows >= 1);
    }

    #[test]
    fn degenerate_images_produce_no_cells() {
        let g = geom(80, 24, 8, 16);
        assert_eq!(image_cells(0, 100, &g, 297), (0, 0));
        assert_eq!(image_cells(100, 0, &g, 297), (0, 0));
    }

    #[test]
    fn aspect_ratio_survives_an_unmeasurable_cell_size() {
        // Defaults must still produce a sane, non-zero rectangle.
        let g = Geometry::default();
        let (cols, rows) = image_cells(640, 640, &g, 297);
        assert_eq!((cols, rows), (80, 40));
    }

    #[test]
    fn png_dimensions_from_ihdr() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1920u32.to_be_bytes());
        png.extend_from_slice(&1080u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((1920, 1080)));
    }

    #[test]
    fn png_dimensions_rejects_non_png() {
        assert_eq!(png_dimensions(b""), None);
        assert_eq!(
            png_dimensions(b"\xff\xd8\xff not a png at all........"),
            None
        );
        let mut truncated = b"\x89PNG\r\n\x1a\n".to_vec();
        truncated.extend_from_slice(b"\x00\x00\x00\x0dIHDR");
        assert_eq!(png_dimensions(&truncated), None);
    }
}
