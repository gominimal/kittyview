// SPDX-License-Identifier: Apache-2.0

//! Interactive slideshow over a list of image files.
//!
//! Runs on the alternate screen buffer (DECSET 1049), reading keys from the
//! terminal in raw mode: Right/Down/Space/n advance, Left/Up/Backspace/p go back,
//! Home/End jump to the ends, and q/Esc/Ctrl-C leave. Every kitty-graphics
//! terminal implements mode 1049, so entering it -- and leaving it on every
//! exit path -- is what keeps the user's scrollback exactly as it was.
//!
//! Slides drawn on placeholder cells switch without flicker: an `a=T,U=1`
//! transmission is invisible until placeholder cells reference it, so the
//! image data streams out while the previous slide is still on screen, and
//! the visible switch is a small burst -- delete the old image, erase the
//! text, write the new grid. `CSI 2J` is never used between slides:
//! terminals delete image data on a clear-screen, including data that was
//! just transmitted and not yet revealed.
//!
//! Restoring the terminal is layered. A normal exit and an error both walk
//! the same cleanup; a panic is caught by a hook that restores the terminal
//! before the message prints, so it lands readable on the main screen; and
//! fatal signals set a flag the event loop acts on -- cleanup first, then
//! the signal is re-raised with its default disposition so the exit status
//! still says what happened. Ctrl-C arrives as a plain byte in raw mode and
//! is treated as a quit key, and Ctrl-Z suspends via the classic dance:
//! restore, stop, and on resume re-enter raw mode and redraw.

use crate::geometry::{self, Geometry};
use crate::kitty::{self, Placement};
use crate::placeholder;
use crate::terminal::Mux;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

/// Decoded frames of one slide: `(png_bytes, delay_ms)` pairs, exactly what
/// the display functions in [`crate::kitty`] take.
pub type Frames = Vec<(Vec<u8>, u32)>;

// ─── Keys ───────────────────────────────────────────────────

/// A keypress the slideshow reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Next,
    Prev,
    First,
    Last,
    Quit,
    Suspend,
    /// Redraw the current slide from scratch (the `r` key).
    Redraw,
    /// Anything unrecognised; the show goes on.
    Other,
}

/// How long a lone ESC byte may sit alone before it is the Esc key rather
/// than the start of a sequence. Real tools sit between 10ms (tmux) and
/// 100ms (vim); a complete sequence arrives in well under 10ms locally,
/// and 50ms leaves headroom for SSH.
const ESC_TIMEOUT: Duration = Duration::from_millis(50);

/// Longest CSI parameter run the decoder will consume before giving up.
const MAX_CSI_LEN: usize = 16;

/// A source of raw input bytes with a timeout, so the decoder can be fed by
/// the terminal in production and by a script in tests.
pub trait ByteSource {
    /// The next byte, or `None` if the timeout passed (or the wait was
    /// interrupted by a signal).
    fn next_byte(&mut self, timeout: Duration) -> Option<u8>;
}

/// Read one key, waiting up to `timeout` for it to begin.
///
/// Returns `None` when no input arrived, which is the event loop's cue to
/// look at its signal flags.
pub fn read_key(src: &mut impl ByteSource, timeout: Duration) -> Option<Key> {
    let byte = src.next_byte(timeout)?;
    Some(match byte {
        // Raw mode delivers Ctrl-C and Ctrl-D as plain bytes -- nothing
        // else will stop the show, so these must count as quit keys.
        0x03 | 0x04 | b'q' | b'Q' => Key::Quit,
        // Ctrl-Z, likewise a plain byte with ISIG off.
        0x1a => Key::Suspend,
        b' ' | b'n' | b'N' => Key::Next,
        0x08 | 0x7f | b'p' | b'P' => Key::Prev,
        b'r' | b'R' => Key::Redraw,
        0x1b => escape_key(src),
        _ => Key::Other,
    })
}

/// Decode what follows an ESC byte.
fn escape_key(src: &mut impl ByteSource) -> Key {
    match src.next_byte(ESC_TIMEOUT) {
        // Nothing followed: the Esc key itself.
        None => Key::Quit,
        Some(b'[') => csi_key(src),
        // SS3 prefix: application cursor mode arrows, ESC O A..F.
        Some(b'O') => match src.next_byte(ESC_TIMEOUT) {
            Some(final_byte) => function_key(final_byte, &[]),
            None => Key::Other,
        },
        // Alt+something; not ours.
        Some(_) => Key::Other,
    }
}

/// Decode a CSI sequence: parameter bytes up to a final byte in `@`..`~`.
fn csi_key(src: &mut impl ByteSource) -> Key {
    let mut params = Vec::new();
    for _ in 0..MAX_CSI_LEN {
        match src.next_byte(ESC_TIMEOUT) {
            Some(byte @ 0x40..=0x7e) => return function_key(byte, &params),
            Some(byte) => params.push(byte),
            None => return Key::Other,
        }
    }
    Key::Other
}

/// Map a CSI/SS3 final byte (and any parameters) to a key.
fn function_key(final_byte: u8, params: &[u8]) -> Key {
    match final_byte {
        // Left/Right are the canonical pair; Up/Down navigate too, not
        // least because Mac keyboards hide Home/End and PgUp/PgDn behind
        // the Fn layer and reaching for the bare arrows is natural.
        b'C' | b'B' => Key::Next,
        b'D' | b'A' => Key::Prev,
        b'H' => Key::First,
        b'F' => Key::Last,
        // The tilde family: CSI <num> ~.
        b'~' => match params.split(|&b| b == b';').next().unwrap_or(&[]) {
            b"5" => Key::Prev,         // PgUp
            b"6" => Key::Next,         // PgDn
            b"1" | b"7" => Key::First, // Home variants
            b"4" | b"8" => Key::Last,  // End variants
            _ => Key::Other,
        },
        _ => Key::Other,
    }
}

// ─── Slide state ────────────────────────────────────────────

/// Which slide is showing, and where the keys can take it.
///
/// Navigation clamps at the ends rather than wrapping: pressing Right on
/// the last slide holds still, which the status line's `n/m` makes visible.
pub struct SlideList {
    count: usize,
    index: usize,
}

impl SlideList {
    pub fn new(count: usize) -> Self {
        Self {
            count: count.max(1),
            index: 0,
        }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Move according to `key`; reports whether the slide changed.
    pub fn apply(&mut self, key: Key) -> bool {
        let target = match key {
            Key::Next => (self.index + 1).min(self.count - 1),
            Key::Prev => self.index.saturating_sub(1),
            Key::First => 0,
            Key::Last => self.count - 1,
            _ => self.index,
        };
        let changed = target != self.index;
        self.index = target;
        changed
    }
}

// ─── Decoded-frame cache ────────────────────────────────────

/// Decoded slides, kept close to the current one.
///
/// Decoding -- an SVG render especially -- is the slow step, so the current
/// slide's neighbours stay decoded for instant back-and-forth. Failures are
/// cached too: retrying a broken file on every keypress buys nothing, and
/// eviction retries it anyway once the user has moved away and back.
#[derive(Default)]
pub struct SlideCache {
    slots: HashMap<usize, Result<Frames, String>>,
}

impl SlideCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The decoded slide at `index`, loading it on first request.
    pub fn get_or_load(
        &mut self,
        index: usize,
        load: impl FnOnce() -> Result<Frames, String>,
    ) -> &Result<Frames, String> {
        self.slots.entry(index).or_insert_with(load)
    }

    /// Drop everything further than one slide from `index`.
    pub fn retain_near(&mut self, index: usize) {
        self.slots.retain(|&i, _| i.abs_diff(index) <= 1);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.slots.len()
    }
}

// ─── Screen sequences ───────────────────────────────────────

/// Enter the slideshow screen: alternate buffer (which clears it), cursor
/// hidden.
pub fn enter_sequence() -> &'static [u8] {
    b"\x1b[?1049h\x1b[?25l"
}

/// Leave the slideshow screen, deleting the images it made first.
///
/// Ordering matters: the deletes must go out while the alternate screen --
/// whose image store they address -- is still active. `d=A` covers direct
/// placements but never virtual ones, so the current image is also deleted
/// by ID. The trailing text restores attributes, screen and cursor even on
/// terminals where mode 1049 is a no-op (GNU screen without `altscreen on`).
pub fn restore_sequence(mux_stack: &[Mux], image_id: Option<u32>) -> Vec<u8> {
    let mut seq = Vec::new();
    if let Some(id) = image_id {
        let _ = kitty::delete_image(&mut seq, mux_stack, id, true);
    }
    let _ = kitty::delete_all(&mut seq, mux_stack);
    seq.extend_from_slice(b"\x1b[0m\x1b[?1049l\x1b[?25h");
    seq
}

/// Move the cursor to a 1-based `(row, col)`.
fn cursor_to(row: u16, col: u16) -> String {
    format!("\x1b[{row};{col}H")
}

/// Placement ID every slide is displayed under. The placeholder cells
/// repeat it in their underline colour, so they resolve against exactly
/// this placement -- never against a stray one like the arrival check's.
pub const SLIDE_PLACEMENT_ID: u32 = 1;

/// Placement ID used by the arrival check, distinct from
/// [`SLIDE_PLACEMENT_ID`] so the probe can never capture the cells.
const VERIFY_PLACEMENT_ID: u32 = 424_242;

/// Build the query that asks whether the terminal holds image `id`'s data.
///
/// A multiplexer can silently drop passthrough sequences, and `q=2` means
/// the terminal never says what arrived -- so after a draw the slideshow
/// asks. A virtual placement attempt with `q=1` stays silent on success and
/// answers with an error when the image does not exist; the placement is
/// deleted (by its placement ID, leaving the real one alone) in the same
/// breath, and a primary DA query follows as the barrier every terminal
/// answers.
pub fn verify_request(mux_stack: &[Mux], image_id: u32) -> Vec<u8> {
    let inner = format!(
        "\x1b_Ga=p,i={image_id},U=1,c=1,r=1,p={VERIFY_PLACEMENT_ID},q=1;\x1b\\\
         \x1b_Ga=d,d=i,i={image_id},p={VERIFY_PLACEMENT_ID},q=2;\x1b\\\
         \x1b[c"
    );
    crate::terminal::wrap_for_stack(inner.as_bytes(), mux_stack)
}

/// Read the verdict out of a reply to [`verify_request`].
///
/// `q=1` keeps success silent, so any graphics APC in the reply is an error
/// report: the image is not there. A reply without one -- the bare DA
/// barrier -- means the data arrived. No reply at all is read as arrived,
/// so a terminal that ignores the query entirely never causes retry loops.
pub fn verify_reply_says_arrived(reply: Option<&[u8]>) -> bool {
    match reply {
        Some(bytes) => !bytes.windows(3).any(|w| w == b"\x1b_G"),
        None => true,
    }
}

/// The status line: ` current/count  filename`.
pub fn status_text(index: usize, count: usize, path: &Path) -> String {
    let name = match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    };
    format!(" {}/{}  {}", index + 1, count, name)
}

// ─── Renderer ───────────────────────────────────────────────

/// Draws slides into a byte buffer; the event loop writes the buffer to the
/// terminal in one go.
///
/// Every draw uses the same commands the single-image path does -- `a=T`,
/// with or without `U=1` -- and nothing else. The protocol's
/// transmit-then-place split (`a=t` + `a=p`) is deliberately not used: it
/// is spec-canonical, but an implementation was found in the wild that
/// renders the split form blank while `a=T` works, and an image parked
/// between `a=t` and `a=p` is first in line for quota eviction in both
/// kitty and Ghostty. Placeholder-based drawing loses nothing -- an
/// `a=T,U=1` transmission is invisible until placeholder cells reference
/// it -- and the direct path accepts a brief clear instead.
pub struct Renderer<'a> {
    pub mux_stack: &'a [Mux],
    /// Whether images are anchored to Unicode placeholder cells, decided
    /// the same way as for single-image display.
    pub use_placeholders: bool,
    pub geom: Geometry,
}

impl Renderer<'_> {
    /// Rows available for the image; the bottom row belongs to the status
    /// line.
    fn avail_rows(&self) -> u16 {
        self.geom.rows.saturating_sub(1).max(1)
    }

    /// Draw one slide, replacing the image `prev_id`, and return the ID the
    /// new image is on screen under (`None` if nothing was placed).
    ///
    /// `prelude` is how the alternate screen is entered on the first draw,
    /// and where it goes relative to the transmission depends on who sits
    /// between us and the terminal -- both orders are load-bearing, in
    /// opposite directions:
    ///
    /// - Direct (and under Zellij, which receives the switch itself):
    ///   prelude first. Kitty-model terminals keep a separate image store
    ///   per screen buffer, so an image transmitted on the main screen
    ///   does not exist on the alternate screen, and cells drawn there
    ///   would reference nothing.
    /// - Through tmux/screen passthrough: transmission first. Switching
    ///   the pane's screen schedules a full redraw, and until it settles
    ///   tmux silently drops passthrough, so a transmission sent just
    ///   after `?1049h` never reaches the terminal -- while the outer
    ///   terminal, whose store holds the image, never switches screens at
    ///   all.
    pub fn draw(
        &self,
        out: &mut impl Write,
        frames: &Frames,
        status: &str,
        prev_id: Option<u32>,
        prelude: &[u8],
    ) -> io::Result<Option<u32>> {
        let Some((png, _)) = frames.first() else {
            self.draw_message(out, "empty image", status, prev_id, prelude)?;
            return Ok(None);
        };
        let animated = frames.len() > 1;

        let max_cells = u16::try_from(placeholder::MAX_INDEX + 1).unwrap_or(u16::MAX);
        let rect = geometry::png_dimensions(png)
            .map(|(w, h)| {
                geometry::image_cells_within(w, h, &self.geom, max_cells, self.avail_rows())
            })
            .filter(|&(c, r)| c > 0 && r > 0);
        // No readable dimensions means no placeholder grid and no centring;
        // direct placement at the origin still shows the image.
        let virtual_grid = if self.use_placeholders { rect } else { None };

        let (origin_row, origin_col) = match rect {
            Some((cols, rows)) => (
                (self.avail_rows() - rows) / 2 + 1,
                self.geom.cols.saturating_sub(cols) / 2 + 1,
            ),
            None => (1, 1),
        };

        let image_id = kitty::pick_image_id(self.mux_stack);
        let mut buf = Vec::new();

        // Whether a passthrough layer separates us from the terminal; see
        // the method comment for how it decides the prelude's position.
        let transmit_first = self
            .mux_stack
            .iter()
            .any(|m| matches!(m, Mux::Tmux(_) | Mux::Screen(_)));
        if !transmit_first {
            buf.extend_from_slice(prelude);
        }

        // Phase one, virtual placements only: the new image's data goes out
        // while the old slide still shows. Nothing addressed to a virtual
        // placement displays until placeholder cells reference it --
        // animation frames and loop start included -- so the whole
        // transmission is invisible and the visible switch below stays a
        // small burst.
        if let Some((cols, rows)) = virtual_grid {
            if animated {
                kitty::transmit_animation_virtual(
                    frames,
                    &mut buf,
                    self.mux_stack,
                    image_id,
                    SLIDE_PLACEMENT_ID,
                    cols,
                    rows,
                )?;
            } else {
                kitty::transmit_virtual(
                    png,
                    &mut buf,
                    self.mux_stack,
                    image_id,
                    SLIDE_PLACEMENT_ID,
                    cols,
                    rows,
                )?;
            }
        }

        if transmit_first {
            buf.extend_from_slice(prelude);
        }

        // The visible switch: out with the old, in with the new.
        if let Some(prev) = prev_id {
            kitty::delete_image(&mut buf, self.mux_stack, prev, true)?;
        }
        // Erase text from the top -- deliberately not CSI 2J, which deletes
        // image data too, the just-transmitted (and still unplaced) image
        // included.
        buf.extend_from_slice(b"\x1b[H\x1b[J");

        if let Some((cols, rows)) = virtual_grid {
            // The data and placement are already in the terminal; the grid
            // cells are what makes the image appear.
            let mut grid = String::new();
            for row in 0..rows {
                grid.push_str(&cursor_to(origin_row + row, origin_col));
                placeholder::write_row(&mut grid, image_id, Some(SLIDE_PLACEMENT_ID), cols, row);
            }
            buf.extend_from_slice(grid.as_bytes());
        } else if animated {
            // A direct animation displays as it streams; it cannot precede
            // the prelude on direct terminals (per-screen image stores) and
            // is not worth a special path under passthrough, where direct
            // placement is already documented as degraded.
            buf.extend_from_slice(cursor_to(origin_row, origin_col).as_bytes());
            kitty::display_animation(frames, &mut buf, self.mux_stack, Placement::Direct)?;
        } else {
            // Direct placement draws at the cursor as it streams, so the
            // screen shows the erase until the data has arrived -- the
            // price of staying on the one command every implementation
            // handles. Direct is the minority path: Zellij, Konsole,
            // iTerm2, and images whose dimensions could not be read.
            buf.extend_from_slice(cursor_to(origin_row, origin_col).as_bytes());
            kitty::display_png_with_id(png, &mut buf, self.mux_stack, image_id)?;
        }

        self.write_status(&mut buf, status);
        out.write_all(&buf)?;
        out.flush()?;

        // A direct animation transmits under the fixed animation ID; that
        // is the ID the next slide must delete.
        let effective_id = if animated && virtual_grid.is_none() {
            kitty::DEFAULT_ANIMATION_ID
        } else {
            image_id
        };
        Ok(Some(effective_id))
    }

    /// Replace the screen with a message -- a slide that failed to load.
    pub fn draw_message(
        &self,
        out: &mut impl Write,
        message: &str,
        status: &str,
        prev_id: Option<u32>,
        prelude: &[u8],
    ) -> io::Result<()> {
        let mut buf = Vec::new();
        buf.extend_from_slice(prelude);
        if let Some(prev) = prev_id {
            kitty::delete_image(&mut buf, self.mux_stack, prev, true)?;
        }
        buf.extend_from_slice(b"\x1b[H\x1b[J");
        let row = self.avail_rows() / 2 + 1;
        let width = u16::try_from(message.chars().count()).unwrap_or(u16::MAX);
        let col = self.geom.cols.saturating_sub(width) / 2 + 1;
        buf.extend_from_slice(cursor_to(row, col).as_bytes());
        buf.extend_from_slice(message.as_bytes());
        self.write_status(&mut buf, status);
        out.write_all(&buf)?;
        out.flush()
    }

    /// The status line on the bottom row, dimmed, truncated to the width.
    fn write_status(&self, buf: &mut Vec<u8>, status: &str) {
        let row = self.geom.rows.max(1);
        let line: String = status.chars().take(self.geom.cols as usize).collect();
        let _ = write!(buf, "\x1b[{row};1H\x1b[K\x1b[2m{line}\x1b[0m");
    }
}

// ─── The interactive session (Unix only) ────────────────────

#[cfg(unix)]
pub use unix::run;

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::terminal::{self, tty};
    use std::path::PathBuf;
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    /// How long one wait for input lasts before the event loop looks at its
    /// signal flags again. A signal interrupts the wait immediately (the
    /// handlers are installed without SA_RESTART), so this only bounds how
    /// stale a flag can get on a completely idle terminal.
    const INPUT_TICK: Duration = Duration::from_millis(250);

    /// The fatal signal waiting to be re-raised, if any.
    static FATAL_SIGNAL: AtomicI32 = AtomicI32::new(0);
    /// The window changed size (SIGWINCH).
    static RESIZED: AtomicBool = AtomicBool::new(false);
    /// An external SIGTSTP asked us to suspend.
    static SUSPEND_ASKED: AtomicBool = AtomicBool::new(false);

    // Handlers only set flags: everything else -- restore, redraw, re-raise
    // -- happens in normal context, where nothing is async-signal-unsafe.
    extern "C" fn note_fatal(sig: libc::c_int) {
        FATAL_SIGNAL.store(sig, Ordering::Relaxed);
    }
    extern "C" fn note_resize(_: libc::c_int) {
        RESIZED.store(true, Ordering::Relaxed);
    }
    extern "C" fn note_suspend(_: libc::c_int) {
        SUSPEND_ASKED.store(true, Ordering::Relaxed);
    }

    impl ByteSource for tty::QuerySession {
        fn next_byte(&mut self, timeout: Duration) -> Option<u8> {
            self.read_byte(timeout)
        }
    }

    /// Installed signal handlers, put back the way they were on drop.
    struct SignalGuard {
        saved: Vec<(libc::c_int, libc::sigaction)>,
    }

    impl SignalGuard {
        fn install() -> Self {
            let handlers: [(libc::c_int, extern "C" fn(libc::c_int)); 5] = [
                (libc::SIGINT, note_fatal),
                (libc::SIGTERM, note_fatal),
                (libc::SIGHUP, note_fatal),
                (libc::SIGWINCH, note_resize),
                (libc::SIGTSTP, note_suspend),
            ];
            let mut saved = Vec::with_capacity(handlers.len());
            for (sig, handler) in handlers {
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                action.sa_sigaction = handler as libc::sighandler_t;
                // No SA_RESTART: the signal must interrupt the select()
                // wait so the event loop sees the flag now, not at the
                // next keypress.
                unsafe { libc::sigemptyset(&mut action.sa_mask) };
                let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
                if unsafe { libc::sigaction(sig, &action, &mut old) } == 0 {
                    saved.push((sig, old));
                }
            }
            Self { saved }
        }
    }

    impl Drop for SignalGuard {
        fn drop(&mut self) {
            for (sig, old) in &self.saved {
                unsafe {
                    libc::sigaction(*sig, old, std::ptr::null_mut());
                }
            }
        }
    }

    /// Re-deliver a signal with its default disposition, so the process
    /// exits the way the sender intended and the parent's wait status says
    /// so.
    fn reraise(sig: libc::c_int) {
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    /// Suspend the process the way less does: restore the terminal, stop,
    /// and on resume re-enter raw mode. The images were deleted and the
    /// alternate screen left on the way out; the caller re-enters it with
    /// the next draw's prelude and redraws.
    fn suspend_self(
        session: &mut tty::QuerySession,
        mux_stack: &[Mux],
        current_id: &mut Option<u32>,
    ) {
        let _ = session.write_bytes(&restore_sequence(mux_stack, current_id.take()));
        session.restore_termios();
        unsafe {
            libc::signal(libc::SIGTSTP, libc::SIG_DFL);
            libc::raise(libc::SIGTSTP);
            // Stopped here until SIGCONT.
            libc::signal(
                libc::SIGTSTP,
                note_suspend as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
        let _ = session.reenter_raw();
        // The window may have been resized while we were stopped.
        RESIZED.store(true, Ordering::Relaxed);
    }

    /// How long to wait for the arrival check's answer. Generous: it rides
    /// the same round trip as a keypress over ssh.
    const VERIFY_TIMEOUT: Duration = Duration::from_millis(600);
    /// How long to let the multiplexer settle after a draw before asking.
    /// The draw's own erase and grid schedule a client redraw, and tmux
    /// drops passthrough -- the question included -- while one is pending;
    /// asking immediately would lose the question itself.
    const VERIFY_SETTLE: Duration = Duration::from_millis(200);
    /// Pause before retrying a draw whose transmission was dropped.
    const RETRY_PAUSE: Duration = Duration::from_millis(150);
    /// Pause after leaving the alternate screen, so the multiplexer's
    /// redraw settles before the data goes out.
    const LEAVE_SETTLE: Duration = Duration::from_millis(250);
    /// Draw attempts per slide under a multiplexer.
    const MAX_ATTEMPTS: u32 = 3;

    /// The state a draw reads and advances: where we are in the list, what
    /// is decoded, what is on screen, and whether the next draw still owes
    /// the alternate-screen entry -- plus the tallies for the diagnostics
    /// line reported when the show ends.
    struct ShowState {
        slides: SlideList,
        cache: SlideCache,
        current_id: Option<u32>,
        pending_enter: bool,
        /// The next draw must rebuild from the main screen (the `r` key).
        force_cycle: bool,
        /// Whether the opening draw has happened; every draw after it
        /// rebuilds from the main screen when under a multiplexer.
        first_draw_done: bool,
        draws: u32,
        verified_ok: u32,
        verify_unanswered: u32,
        lost_transmissions: u32,
        fallback_draws: u32,
        unrecovered: u32,
    }

    /// Load (through the cache) and draw the slide at the current index,
    /// verifying under a multiplexer that the image data survived the trip
    /// and retrying when it did not.
    ///
    /// Under a multiplexer, every draw after the first rebuilds from the
    /// pane's main screen -- leave the alternate screen, let the redraw
    /// settle, transmit, re-enter -- at the cost of a brief flash of the
    /// underlying pane. Field-tested on Ghostty 1.3.1 + tmux: it is the
    /// one draw shape that reliably ends in a rendered image there, where
    /// in-place draws end blank even when the protocol's own arrival
    /// check reports the data present. Each draw is still verified over
    /// the reply channel and retried on a reported loss, and the `r` key
    /// forces a rebuild by hand.
    ///
    /// When `state.pending_enter` is set, the draw carries the
    /// alternate-screen entry in its prelude (and clears the flag once
    /// written).
    fn show(
        session: &mut tty::QuerySession,
        renderer: &Renderer,
        state: &mut ShowState,
        load: &mut impl FnMut(&Path) -> Result<Frames, String>,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        state.draws += 1;
        let attempts = if renderer.mux_stack.is_empty() {
            1
        } else {
            MAX_ATTEMPTS
        };
        for attempt in 0..attempts {
            if attempt > 0 {
                std::thread::sleep(RETRY_PAUSE);
            }
            let cycle = !renderer.mux_stack.is_empty()
                && (state.first_draw_done || state.force_cycle || attempt + 1 == MAX_ATTEMPTS);
            if cycle {
                if attempt + 1 == MAX_ATTEMPTS {
                    state.fallback_draws += 1;
                }
                if !state.pending_enter {
                    session
                        .write_bytes(b"\x1b[?1049l")
                        .map_err(|e| format!("Failed to write to terminal: {e}"))?;
                }
                std::thread::sleep(LEAVE_SETTLE);
                state.pending_enter = true;
            }

            let prelude: &[u8] = if state.pending_enter {
                enter_sequence()
            } else {
                b""
            };
            let path = &paths[state.slides.index()];
            let status = status_text(state.slides.index(), state.slides.count(), path);
            let mut buf = Vec::new();
            let (drawn, animated) =
                match state.cache.get_or_load(state.slides.index(), || load(path)) {
                    Ok(frames) => (
                        renderer
                            .draw(&mut buf, frames, &status, state.current_id, prelude)
                            .map_err(|e| format!("Failed to render slide: {e}"))?,
                        frames.len() > 1,
                    ),
                    Err(message) => {
                        renderer
                            .draw_message(&mut buf, message, &status, state.current_id, prelude)
                            .map_err(|e| format!("Failed to render slide: {e}"))?;
                        (None, false)
                    }
                };
            session
                .write_bytes(&buf)
                .map_err(|e| format!("Failed to write to terminal: {e}"))?;
            state.pending_enter = false;
            state.current_id = drawn;
            // Keep the emergency restore current: whatever is on screen
            // now is what a panic must delete.
            session.arm_emergency_restore(restore_sequence(renderer.mux_stack, state.current_id));

            // Nothing to verify without a multiplexer, without an image,
            // or for animations (whose arrival has no cheap check).
            let Some(image_id) = drawn else {
                state.force_cycle = false;
                return Ok(());
            };
            if renderer.mux_stack.is_empty() || animated {
                state.force_cycle = false;
                return Ok(());
            }
            // The answer window doubles as input: a key pressed during it
            // is discarded with the reply -- worth it under a multiplexer.
            std::thread::sleep(VERIFY_SETTLE);
            let request = verify_request(renderer.mux_stack, image_id);
            match session.ask(&request, VERIFY_TIMEOUT) {
                None => {
                    state.verify_unanswered += 1;
                    state.force_cycle = false;
                    return Ok(());
                }
                Some(reply) if verify_reply_says_arrived(Some(&reply)) => {
                    state.verified_ok += 1;
                    state.force_cycle = false;
                    return Ok(());
                }
                Some(_) => state.lost_transmissions += 1,
            }
        }
        state.unrecovered += 1;
        state.force_cycle = false;
        Ok(())
    }

    /// Run the slideshow. Returns when the user quits, an error makes the
    /// terminal unusable, or a fatal signal arrives (which is re-raised
    /// after cleanup, so this then never actually returns).
    pub fn run(
        paths: &[PathBuf],
        mux_stack: &[Mux],
        use_placeholders: bool,
        mut load: impl FnMut(&Path) -> Result<Frames, String>,
    ) -> Result<(), String> {
        // Geometry first: it can run escape-sequence queries, which need
        // the terminal to themselves before the slideshow owns it.
        let geom = geometry::detect(mux_stack);

        let mut session =
            terminal::query_session().ok_or("slideshow mode needs a terminal to read keys from")?;

        // The panic hook restores the terminal before the panic message
        // prints, so the message lands readable on the main screen. It
        // stays installed for the life of the process; when nothing is
        // armed it does nothing.
        static HOOK: Once = Once::new();
        HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                tty::run_emergency_restore();
                previous(info);
            }));
        });

        FATAL_SIGNAL.store(0, Ordering::Relaxed);
        RESIZED.store(false, Ordering::Relaxed);
        SUSPEND_ASKED.store(false, Ordering::Relaxed);
        let signals = SignalGuard::install();

        session.arm_emergency_restore(restore_sequence(mux_stack, None));

        let mut renderer = Renderer {
            mux_stack,
            use_placeholders,
            geom,
        };
        let mut state = ShowState {
            slides: SlideList::new(paths.len()),
            cache: SlideCache::new(),
            current_id: None,
            // The alternate screen is entered by the first draw itself,
            // ordered around its image transmission per target -- see
            // Renderer::draw for why each order is load-bearing.
            pending_enter: true,
            force_cycle: false,
            first_draw_done: false,
            draws: 0,
            verified_ok: 0,
            verify_unanswered: 0,
            lost_transmissions: 0,
            fallback_draws: 0,
            unrecovered: 0,
        };

        // The event loop proper, wrapped so that every exit path below
        // walks the same cleanup.
        let outcome = (|| -> Result<Option<libc::c_int>, String> {
            show(&mut session, &renderer, &mut state, &mut load, paths)?;
            state.first_draw_done = true;
            loop {
                let sig = FATAL_SIGNAL.swap(0, Ordering::Relaxed);
                if sig != 0 {
                    return Ok(Some(sig));
                }
                if SUSPEND_ASKED.swap(false, Ordering::Relaxed) {
                    suspend_self(&mut session, mux_stack, &mut state.current_id);
                    state.pending_enter = true;
                }
                if RESIZED.swap(false, Ordering::Relaxed) {
                    renderer.geom = geometry::refresh_window_size(&renderer.geom);
                    show(&mut session, &renderer, &mut state, &mut load, paths)?;
                }
                match read_key(&mut session, INPUT_TICK) {
                    None => continue,
                    Some(Key::Quit) => return Ok(None),
                    Some(Key::Suspend) => {
                        suspend_self(&mut session, mux_stack, &mut state.current_id);
                        state.pending_enter = true;
                        // The redraw happens via the RESIZED flag above.
                    }
                    Some(Key::Redraw) => {
                        state.force_cycle = true;
                        show(&mut session, &renderer, &mut state, &mut load, paths)?;
                    }
                    Some(Key::Other) => {}
                    Some(key) => {
                        if state.slides.apply(key) {
                            state.cache.retain_near(state.slides.index());
                            show(&mut session, &renderer, &mut state, &mut load, paths)?;
                        }
                    }
                }
            }
        })();

        // The one cleanup path: images deleted, alternate screen left,
        // cursor shown, termios restored -- in that order, then the
        // emergency copy disarmed because it is no longer needed.
        let _ = session.write_bytes(&restore_sequence(mux_stack, state.current_id));
        tty::disarm_emergency_restore();
        session.restore_termios();
        drop(signals);

        // Transmission report, now that stderr lands on the main screen
        // again. This is the diagnostic for "the image never appeared": it
        // says whether the data was dropped in transit (a multiplexer
        // problem, retried here) or arrived and simply was not drawn (a
        // terminal problem).
        if !mux_stack.is_empty() {
            eprintln!(
                "Slideshow diagnostics (multiplexer): {} draw(s); transmissions: \
                 {} confirmed arrived, {} checks unanswered, {} dropped and retried, \
                 {} never arrived.",
                state.draws,
                state.verified_ok,
                state.verify_unanswered,
                state.lost_transmissions,
                state.unrecovered,
            );
        }

        match outcome {
            Ok(Some(sig)) => {
                reraise(sig);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── key decoding ────────────────────────────────────────

    /// A scripted byte source: `None` entries model a timeout.
    struct Script(std::collections::VecDeque<Option<u8>>);

    impl Script {
        fn new(bytes: &[Option<u8>]) -> Self {
            Self(bytes.iter().copied().collect())
        }

        fn of(bytes: &[u8]) -> Self {
            Self(bytes.iter().map(|&b| Some(b)).collect())
        }
    }

    impl ByteSource for Script {
        fn next_byte(&mut self, _timeout: Duration) -> Option<u8> {
            self.0.pop_front().flatten()
        }
    }

    fn key_of(bytes: &[u8]) -> Option<Key> {
        read_key(&mut Script::of(bytes), Duration::ZERO)
    }

    #[test]
    fn quit_keys() {
        for bytes in [b"q".as_slice(), b"Q", b"\x03", b"\x04"] {
            assert_eq!(key_of(bytes), Some(Key::Quit), "{bytes:?}");
        }
    }

    #[test]
    fn a_lone_escape_quits() {
        let mut src = Script::new(&[Some(0x1b), None]);
        assert_eq!(read_key(&mut src, Duration::ZERO), Some(Key::Quit));
    }

    #[test]
    fn arrow_keys_in_both_encodings() {
        // CSI (normal mode) and SS3 (application cursor mode).
        assert_eq!(key_of(b"\x1b[C"), Some(Key::Next));
        assert_eq!(key_of(b"\x1b[D"), Some(Key::Prev));
        assert_eq!(key_of(b"\x1bOC"), Some(Key::Next));
        assert_eq!(key_of(b"\x1bOD"), Some(Key::Prev));
        // Up/Down navigate too.
        assert_eq!(key_of(b"\x1b[B"), Some(Key::Next));
        assert_eq!(key_of(b"\x1b[A"), Some(Key::Prev));
        assert_eq!(key_of(b"\x1bOB"), Some(Key::Next));
        assert_eq!(key_of(b"\x1bOA"), Some(Key::Prev));
    }

    #[test]
    fn plain_navigation_keys() {
        assert_eq!(key_of(b" "), Some(Key::Next));
        assert_eq!(key_of(b"n"), Some(Key::Next));
        assert_eq!(key_of(b"\x7f"), Some(Key::Prev)); // Backspace as DEL
        assert_eq!(key_of(b"\x08"), Some(Key::Prev)); // Backspace as BS
        assert_eq!(key_of(b"p"), Some(Key::Prev));
    }

    #[test]
    fn home_and_end_jump_to_the_ends() {
        assert_eq!(key_of(b"\x1b[H"), Some(Key::First));
        assert_eq!(key_of(b"\x1b[F"), Some(Key::Last));
        assert_eq!(key_of(b"\x1bOH"), Some(Key::First));
        assert_eq!(key_of(b"\x1bOF"), Some(Key::Last));
        assert_eq!(key_of(b"\x1b[1~"), Some(Key::First));
        assert_eq!(key_of(b"\x1b[4~"), Some(Key::Last));
    }

    #[test]
    fn page_keys_navigate() {
        assert_eq!(key_of(b"\x1b[5~"), Some(Key::Prev));
        assert_eq!(key_of(b"\x1b[6~"), Some(Key::Next));
    }

    #[test]
    fn modified_arrows_still_navigate() {
        // Ctrl+Right arrives as CSI 1;5C -- parameters before the final
        // byte must not confuse the decoder.
        assert_eq!(key_of(b"\x1b[1;5C"), Some(Key::Next));
    }

    #[test]
    fn ctrl_z_asks_to_suspend() {
        assert_eq!(key_of(b"\x1a"), Some(Key::Suspend));
    }

    #[test]
    fn r_asks_for_a_hard_redraw() {
        assert_eq!(key_of(b"r"), Some(Key::Redraw));
        assert_eq!(key_of(b"R"), Some(Key::Redraw));
    }

    #[test]
    fn unrecognised_input_is_ignored_not_fatal() {
        assert_eq!(key_of(b"x"), Some(Key::Other));
        assert_eq!(key_of(b"\x1bx"), Some(Key::Other)); // Alt+x
        assert_eq!(key_of(b"\x1b[Z"), Some(Key::Other)); // Shift+Tab
        assert_eq!(key_of(b"\x1b[15~"), Some(Key::Other)); // F5
    }

    #[test]
    fn an_unterminated_csi_gives_up() {
        let bytes: Vec<Option<u8>> = std::iter::once(Some(0x1b))
            .chain(std::iter::once(Some(b'[')))
            .chain(std::iter::repeat_n(Some(b'1'), MAX_CSI_LEN + 4))
            .collect();
        let mut src = Script::new(&bytes);
        assert_eq!(read_key(&mut src, Duration::ZERO), Some(Key::Other));
    }

    #[test]
    fn no_input_reads_as_no_key() {
        let mut src = Script::new(&[None]);
        assert_eq!(read_key(&mut src, Duration::ZERO), None);
    }

    // ── slide list ──────────────────────────────────────────

    #[test]
    fn navigation_clamps_at_both_ends() {
        let mut slides = SlideList::new(3);
        assert!(!slides.apply(Key::Prev), "already at the first slide");
        assert!(slides.apply(Key::Next));
        assert!(slides.apply(Key::Next));
        assert_eq!(slides.index(), 2);
        assert!(!slides.apply(Key::Next), "already at the last slide");
    }

    #[test]
    fn home_and_end_move_to_the_ends() {
        let mut slides = SlideList::new(5);
        assert!(slides.apply(Key::Last));
        assert_eq!(slides.index(), 4);
        assert!(slides.apply(Key::First));
        assert_eq!(slides.index(), 0);
        assert!(!slides.apply(Key::First), "no move, no redraw");
    }

    #[test]
    fn non_navigation_keys_do_not_move() {
        let mut slides = SlideList::new(3);
        for key in [Key::Quit, Key::Suspend, Key::Redraw, Key::Other] {
            assert!(!slides.apply(key));
        }
        assert_eq!(slides.index(), 0);
    }

    // ── cache ───────────────────────────────────────────────

    #[test]
    fn slides_are_decoded_once() {
        let mut cache = SlideCache::new();
        let mut loads = 0;
        for _ in 0..3 {
            let _ = cache.get_or_load(0, || {
                loads += 1;
                Ok(vec![(vec![1], 0)])
            });
        }
        assert_eq!(loads, 1);
    }

    #[test]
    fn failures_are_cached_too() {
        let mut cache = SlideCache::new();
        let mut loads = 0;
        for _ in 0..2 {
            let _ = cache.get_or_load(0, || {
                loads += 1;
                Err("broken".into())
            });
        }
        assert_eq!(loads, 1);
    }

    #[test]
    fn eviction_keeps_only_the_neighbourhood() {
        let mut cache = SlideCache::new();
        for i in 0..6 {
            let _ = cache.get_or_load(i, || Ok(vec![(vec![1], 0)]));
        }
        cache.retain_near(4);
        assert_eq!(cache.len(), 3, "slides 3, 4 and 5 remain");
    }

    #[test]
    fn eviction_at_the_first_slide_does_not_underflow() {
        let mut cache = SlideCache::new();
        for i in 0..4 {
            let _ = cache.get_or_load(i, || Ok(vec![(vec![1], 0)]));
        }
        cache.retain_near(0);
        assert_eq!(cache.len(), 2, "slides 0 and 1 remain");
    }

    // ── screen sequences ────────────────────────────────────

    #[test]
    fn enter_switches_screen_and_hides_cursor() {
        let seq = enter_sequence();
        assert!(seq.starts_with(b"\x1b[?1049h"));
        assert!(seq.ends_with(b"\x1b[?25l"));
    }

    #[test]
    fn restore_deletes_images_before_leaving_the_screen() {
        let seq = restore_sequence(&[], Some(42));
        let text = String::from_utf8(seq).unwrap();
        let delete_current = text.find("a=d,d=I,i=42").unwrap();
        let delete_all = text.find("a=d,d=A").unwrap();
        let leave = text.find("\x1b[?1049l").unwrap();
        assert!(delete_current < delete_all && delete_all < leave);
        assert!(text.ends_with("\x1b[?25h"), "the cursor comes back last");
    }

    #[test]
    fn restore_without_an_image_still_cleans_up() {
        let text = String::from_utf8(restore_sequence(&[], None)).unwrap();
        assert!(!text.contains("d=I"));
        assert!(text.contains("a=d,d=A"));
        assert!(text.contains("\x1b[?1049l"));
    }

    #[test]
    fn restore_wraps_deletes_for_the_multiplexer_but_not_the_csis() {
        let stack = [Mux::Tmux(None)];
        let seq = restore_sequence(&stack, Some(7));
        let text = String::from_utf8_lossy(&seq);
        // The APC deletes need passthrough to reach the terminal...
        assert!(text.contains("\x1bPtmux;"));
        // ...but the alternate screen is tmux's own to manage, so its CSIs
        // must arrive unwrapped.
        assert!(text.contains("\x1b[?1049l"));
        assert!(!text.contains("\x1b\x1b[?1049l"));
    }

    // ── transmission verification ───────────────────────────

    #[test]
    fn verify_request_probes_and_cleans_up_after_itself() {
        let req = String::from_utf8(verify_request(&[], 42)).unwrap();
        // The probe placement is silent on success, loud on a missing
        // image...
        assert!(req.contains("a=p,i=42,U=1,c=1,r=1,p=424242,q=1"));
        // ...is deleted by its own placement ID, leaving the real
        // placement alone...
        assert!(req.contains("a=d,d=i,i=42,p=424242,q=2"));
        // ...and rides with the DA barrier every terminal answers.
        assert!(req.ends_with("\x1b[c"));
        let place = req.find("a=p").unwrap();
        let delete = req.find("a=d").unwrap();
        assert!(place < delete);
    }

    #[test]
    fn verify_request_is_passthrough_wrapped_as_one_unit() {
        let req = verify_request(&[Mux::Tmux(None)], 7);
        let wraps = req.windows(7).filter(|w| *w == b"\x1bPtmux;").count();
        // One envelope: the DA barrier must reach the outer terminal too,
        // or the multiplexer answers it early and the check lies.
        assert_eq!(wraps, 1);
    }

    #[test]
    fn verify_reply_reads_silence_as_arrival() {
        // A bare DA reply: no error, the image is there.
        assert!(verify_reply_says_arrived(Some(b"\x1b[?62;c")));
        // No reply at all: benefit of the doubt, never a retry loop.
        assert!(verify_reply_says_arrived(None));
        // Any graphics APC is an error report (q=1 silenced success):
        // the transmission was dropped.
        assert!(!verify_reply_says_arrived(Some(
            b"\x1b_Gi=42;ENOENT:image not found\x1b\\"
        )));
    }

    // ── status line ─────────────────────────────────────────

    #[test]
    fn status_names_the_position_and_file() {
        let status = status_text(2, 7, &PathBuf::from("/photos/cat.png"));
        assert_eq!(status, " 3/7  cat.png");
    }

    // ── renderer ────────────────────────────────────────────

    /// The 24 bytes of PNG header [`geometry::png_dimensions`] reads.
    fn png_stub(width: u32, height: u32) -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png
    }

    fn test_geometry() -> Geometry {
        Geometry {
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 16,
            cell_size_measured: true,
        }
    }

    fn renderer(use_placeholders: bool) -> Renderer<'static> {
        Renderer {
            mux_stack: &[],
            use_placeholders,
            geom: test_geometry(),
        }
    }

    fn draw_to_string(r: &Renderer, frames: &Frames, prev: Option<u32>) -> (String, Option<u32>) {
        let mut out = Vec::new();
        let id = r.draw(&mut out, frames, " 1/2  a.png", prev, b"").unwrap();
        (String::from_utf8_lossy(&out).into_owned(), id)
    }

    #[test]
    fn virtual_slides_transmit_before_the_screen_is_touched() {
        // 160x160 px in 8x16 cells is 20x10; centred in 80x23 that is
        // row 7, column 31.
        let frames = vec![(png_stub(160, 160), 0)];
        let (out, id) = draw_to_string(&renderer(true), &frames, None);
        let id = id.unwrap();

        let transmit = out
            .find(&format!("a=T,f=100,i={id},U=1,c=20,r=10,p=1,q=2"))
            .unwrap();
        let erase = out.find("\x1b[H\x1b[J").unwrap();
        let grid = out.find(crate::placeholder::PLACEHOLDER).unwrap();
        assert!(transmit < erase, "data goes out while the old slide shows");
        assert!(erase < grid, "the cells that reveal the image come last");
        assert!(out.contains("\x1b[7;31H"), "grid rows are centred");
        assert_eq!(
            out.matches(crate::placeholder::PLACEHOLDER).count(),
            20 * 10
        );
    }

    #[test]
    fn direct_terminals_enter_the_screen_before_transmitting() {
        // Kitty-model terminals keep a separate image store per screen
        // buffer: an image transmitted on the main screen does not exist
        // on the alternate screen, so the switch must come first.
        let frames = vec![(png_stub(160, 160), 0)];
        let mut out = Vec::new();
        renderer(true)
            .draw(&mut out, &frames, " 1/2  a.png", None, enter_sequence())
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        let enter = text.find("\x1b[?1049h").unwrap();
        let transmit = text.find("a=T,f=100").unwrap();
        let erase = text.find("\x1b[H\x1b[J").unwrap();
        assert!(
            enter < transmit && transmit < erase,
            "switch screens, then transmit, then draw"
        );
    }

    #[test]
    fn passthrough_stacks_transmit_before_entering_the_screen() {
        // tmux schedules a full pane redraw on the alternate-screen switch
        // and silently drops passthrough until it settles, so through a
        // passthrough stack the transmission must come first -- the outer
        // terminal, whose store holds the image, never switches screens.
        let frames = vec![(png_stub(160, 160), 0)];
        let stack = [Mux::Tmux(None)];
        let r = Renderer {
            mux_stack: &stack,
            use_placeholders: true,
            geom: test_geometry(),
        };
        let mut out = Vec::new();
        r.draw(&mut out, &frames, " 1/2  a.png", None, enter_sequence())
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        let transmit = text.find("a=T,f=100").unwrap();
        let enter = text.find("\x1b[?1049h").unwrap();
        let erase = text.find("\x1b[H\x1b[J").unwrap();
        assert!(
            transmit < enter && enter < erase,
            "transmit, then switch screens, then draw"
        );
    }

    #[test]
    fn animated_virtual_slides_follow_the_same_prelude_ordering() {
        // Frames and loop start address the image, not the screen, so the
        // whole animation rides in phase one -- before the screen switch
        // under passthrough, after it on direct terminals.
        let frames = vec![(png_stub(160, 160), 100), (png_stub(160, 160), 100)];

        let stack = [Mux::Tmux(None)];
        let r = Renderer {
            mux_stack: &stack,
            use_placeholders: true,
            geom: test_geometry(),
        };
        let mut out = Vec::new();
        r.draw(&mut out, &frames, " 1/2  a.png", None, enter_sequence())
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        let loop_start = text.find("a=a,").unwrap();
        let enter = text.find("\x1b[?1049h").unwrap();
        assert!(
            loop_start < enter,
            "the whole animation precedes the screen switch under passthrough"
        );

        let mut out = Vec::new();
        renderer(true)
            .draw(&mut out, &frames, " 1/2  a.png", None, enter_sequence())
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        let enter = text.find("\x1b[?1049h").unwrap();
        let base = text.find("a=T,f=100").unwrap();
        assert!(
            enter < base,
            "on direct terminals the screen switch still comes first"
        );
    }

    #[test]
    fn animated_virtual_grids_are_centred_and_bound() {
        // With the animation transmitted separately from its grid, the
        // rows are cursor-positioned like static slides: centred, and
        // bound to the slide placement ID.
        let frames = vec![(png_stub(160, 160), 100), (png_stub(160, 160), 100)];
        let (out, _) = draw_to_string(&renderer(true), &frames, None);
        assert!(out.contains("\x1b[7;31H"), "grid rows are centred");
        assert_eq!(out.matches("\x1b[58;5;1m").count(), 10, "one per row");
    }

    #[test]
    fn grid_cells_are_bound_to_the_slide_placement_id() {
        // Ghostty through 1.3.1 resolves unbound cells against a
        // hash-random virtual placement of the image; the underline colour
        // pins them to ours, exactly.
        let frames = vec![(png_stub(160, 160), 0)];
        let (out, _) = draw_to_string(&renderer(true), &frames, None);
        assert_eq!(out.matches("\x1b[58;5;1m").count(), 10, "one per row");
        assert_eq!(out.matches("\x1b[59m").count(), 10, "and one reset each");
    }

    #[test]
    fn slides_never_use_the_transmit_then_place_split() {
        // The split (`a=t` + `a=p`) is spec-canonical but was found in the
        // wild rendering blank where `a=T` works, and a parked `a=t` image
        // is first in line for quota eviction. Only the single command the
        // ordinary display path uses is trusted.
        let frames = vec![(png_stub(160, 160), 0)];
        for r in [renderer(true), renderer(false)] {
            let (out, _) = draw_to_string(&r, &frames, None);
            assert!(!out.contains("a=t,"), "no bare transmit");
            assert!(!out.contains("a=p,"), "no separate put");
        }
    }

    #[test]
    fn slides_never_clear_with_2j() {
        // CSI 2J deletes image data -- the just-transmitted slide included.
        let frames = vec![(png_stub(160, 160), 0)];
        for r in [renderer(true), renderer(false)] {
            let (out, _) = draw_to_string(&r, &frames, Some(3));
            assert!(!out.contains("\x1b[2J"));
        }
    }

    #[test]
    fn the_previous_slide_is_deleted_with_its_data() {
        let frames = vec![(png_stub(160, 160), 0)];
        let (out, _) = draw_to_string(&renderer(true), &frames, Some(7));
        let transmit = out.find("a=T,f=100").unwrap();
        let delete = out.find("a=d,d=I,i=7,q=2").unwrap();
        assert!(
            transmit < delete,
            "the old slide stays visible until the new data has arrived"
        );
    }

    #[test]
    fn direct_placement_centres_and_displays_under_a_deletable_id() {
        let frames = vec![(png_stub(160, 160), 0)];
        let (out, id) = draw_to_string(&renderer(false), &frames, None);
        let id = id.unwrap();
        let erase = out.find("\x1b[H\x1b[J").unwrap();
        let cursor = out.find("\x1b[7;31H").unwrap();
        let draw = out.find(&format!("a=T,f=100,i={id},q=2")).unwrap();
        assert!(erase < cursor && cursor < draw, "erase, position, draw");
        assert!(!out.contains("U=1"));
    }

    #[test]
    fn animations_use_the_animation_path() {
        // The image ID is drawn at random by design, so the strongest
        // available assertion ties the reported ID to every part of the
        // sequence that must reference it: the base frame, the extra
        // frame, and the loop-start control -- which is exactly what the
        // next slide's delete depends on. DELAY_MS is a frame delay, not
        // an ID.
        const DELAY_MS: u32 = 100;
        let frames = vec![
            (png_stub(160, 160), DELAY_MS),
            (png_stub(160, 160), DELAY_MS),
        ];
        let (out, id) = draw_to_string(&renderer(true), &frames, None);
        assert!(
            !out.contains("a=t,"),
            "no transmit-then-place for animations"
        );
        let id = id.expect("an animation is placed under an ID");
        assert_ne!(id, kitty::DEFAULT_ANIMATION_ID);
        assert!(
            out.contains(&format!("a=T,f=100,i={id},U=1")),
            "the base frame is transmitted under the reported ID"
        );
        assert!(
            out.contains(&format!("a=f,i={id},r=2,z={DELAY_MS}")),
            "the second frame references it with its delay"
        );
        assert!(
            out.contains(&format!("a=a,i={id},r=1,z={DELAY_MS},s=3,v=1")),
            "and the loop is started on it"
        );
    }

    #[test]
    fn a_direct_animation_reports_the_fixed_animation_id() {
        let frames = vec![(png_stub(16, 16), 100), (png_stub(16, 16), 100)];
        let (_, id) = draw_to_string(&renderer(false), &frames, None);
        assert_eq!(id, Some(kitty::DEFAULT_ANIMATION_ID));
    }

    #[test]
    fn the_status_line_sits_on_the_bottom_row() {
        let frames = vec![(png_stub(160, 160), 0)];
        let (out, _) = draw_to_string(&renderer(true), &frames, None);
        assert!(out.contains("\x1b[24;1H\x1b[K\x1b[2m 1/2  a.png\x1b[0m"));
    }

    #[test]
    fn oversized_images_fit_the_screen_not_just_the_width() {
        // 640x6400 px is 80x400 cells naturally; it must land within the
        // 23 rows above the status line: 80 * 23/400 rounds to 5 columns.
        let frames = vec![(png_stub(640, 6400), 0)];
        let (out, _) = draw_to_string(&renderer(true), &frames, None);
        assert!(
            out.contains(",c=5,r=23,p=1,q=2"),
            "fitted to 23 rows: {out}"
        );
    }

    #[test]
    fn unreadable_dimensions_fall_back_to_direct_placement() {
        let frames = vec![(b"not a png".to_vec(), 0)];
        let (out, id) = draw_to_string(&renderer(true), &frames, None);
        assert!(id.is_some());
        assert!(!out.contains("U=1"), "no grid without known dimensions");
        assert!(out.contains("\x1b[1;1H"));
    }

    #[test]
    fn empty_frames_draw_a_message_not_a_crash() {
        let frames: Frames = vec![];
        let mut out = Vec::new();
        let id = renderer(true)
            .draw(&mut out, &frames, " 1/1  x", Some(9), b"")
            .unwrap();
        assert_eq!(id, None);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("empty image"));
        assert!(text.contains("a=d,d=I,i=9"), "the old slide is removed");
    }

    #[test]
    fn a_message_slide_still_enters_the_screen() {
        // A slideshow whose first file fails to load must still switch to
        // the alternate screen before erasing anything: the prelude leads,
        // since a message has no transmission to order around.
        let mut out = Vec::new();
        renderer(true)
            .draw_message(&mut out, "Error: no", " 1/1  x", None, enter_sequence())
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        let enter = text.find("\x1b[?1049h").unwrap();
        let erase = text.find("\x1b[H\x1b[J").unwrap();
        assert!(enter < erase, "switch screens before erasing");
    }

    #[test]
    fn messages_are_centred_and_replace_the_image() {
        let mut out = Vec::new();
        renderer(true)
            .draw_message(&mut out, "Error: no such file", " 2/3  b.png", Some(5), b"")
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("a=d,d=I,i=5"));
        assert!(text.contains("Error: no such file"));
        assert!(text.contains(" 2/3  b.png"));
    }

    #[test]
    fn the_status_line_is_truncated_to_the_window() {
        let mut r = renderer(true);
        r.geom.cols = 10;
        let mut out = Vec::new();
        r.draw_message(&mut out, "x", " 1/1  a-very-long-name.png", None, b"")
            .unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("\x1b[2m 1/1  a-ve\x1b[0m"));
    }
}
