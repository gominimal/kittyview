// SPDX-License-Identifier: Apache-2.0

//! Bisection probe for placeholder rendering: draws the built-in logo via
//! Unicode placeholders in one of several ways, to isolate which ingredient
//! of the slideshow's draw makes a terminal render blank cells.
//!
//! Run *inside* the terminal (and tmux pane) where the problem shows:
//!
//! ```sh
//! cargo build --example placeholder_probe
//! ./target/debug/examples/placeholder_probe flowed          # control
//! ./target/debug/examples/placeholder_probe positioned      # cursor-addressed rows
//! ./target/debug/examples/placeholder_probe alt-flowed      # alternate screen
//! ./target/debug/examples/placeholder_probe alt-positioned  # ~ the slideshow draw
//! ./target/debug/examples/placeholder_probe cycle           # the navigation step
//! ```
//!
//! Each variant shows the image for four seconds, then cleans up after
//! itself. Note which variants show the cat logo and which stay blank.
//!
//! A second group bisects the *repair* rather than the draw: when a slide
//! renders clipped or blank, which nudge completes it without re-sending
//! image data the terminal already holds?
//!
//! ```sh
//! ./target/debug/examples/placeholder_probe manual   # control: switch panes by hand
//! ./target/debug/examples/placeholder_probe refresh  # tmux refresh-client
//! ./target/debug/examples/placeholder_probe nudge    # one unrelated cell of traffic
//! ./target/debug/examples/placeholder_probe rewrite  # re-send the grid cells in place
//! ./target/debug/examples/placeholder_probe cycle-refresh  # navigation, repaired by refresh
//! ```
//!
//! Run `manual` first: it establishes that this draw reproduces the problem
//! at all, and that a pane switch repairs it. Without that the other three
//! prove nothing. Each shows the draw, waits, applies exactly one nudge,
//! and waits again, so a single run shows before and after.
//!
//! This is a debugging aid, not part of the kittyview CLI.

use kittyview::terminal::Mux;
use kittyview::{kitty, logo, placeholder};
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

const COLS: u16 = 16;
const ROWS: u16 = 8;
/// Where the positioned variants put the grid: away from column 1, like the
/// slideshow's centring does.
const ORIGIN_ROW: u16 = 3;
const ORIGIN_COL: u16 = 5;
/// How long the nudge variants leave the screen alone for one observation.
const OBSERVE: Duration = Duration::from_secs(5);
/// Longer, for the variant whose nudge is a pane switch made by hand.
const MANUAL_OBSERVE: Duration = Duration::from_secs(15);
/// How long to let a draw land before asking tmux to repaint, mirroring the
/// slideshow's `VERIFY_SETTLE`: tmux drops passthrough while a redraw is
/// pending, so the transmission must be clear of the repaint at both ends.
const REPAINT_SETTLE: Duration = Duration::from_millis(200);

fn main() -> io::Result<()> {
    let variant = std::env::args().nth(1).unwrap_or_default();
    // Wrap for tmux exactly when running inside one, so the probe also
    // works directly in the terminal for comparison.
    let mux: Vec<Mux> = if std::env::var_os("TMUX").is_some() {
        vec![Mux::Tmux(None)]
    } else {
        vec![]
    };
    let png = logo::generate_logo_png();
    let image_id = kitty::pick_image_id(&mux);

    // Give a freshly created pane time to finish attaching: tmux forwards
    // passthrough only to an attached, visible client, and output written
    // in the first instants of a pane's life can beat the attach.
    std::thread::sleep(Duration::from_millis(500));

    let mut out = io::stdout().lock();
    match variant.as_str() {
        // Newline-flowed grid at the cursor on the main screen: the same
        // shape as single-image display, the known-good control.
        "flowed" => {
            kitty::display_png(
                &png,
                &mut out,
                &mux,
                kitty::Placement::Virtual {
                    image_id,
                    cols: COLS,
                    rows: ROWS,
                },
            )?;
            hold(&mut out)?;
            cleanup(&mut out, &mux, image_id, false)
        }
        // Same, but each grid row is cursor-positioned away from column 1.
        "positioned" => {
            transmit_and_position(&mut out, &mux, &png, image_id)?;
            hold(&mut out)?;
            write!(out, "\x1b[{};1H", ORIGIN_ROW + ROWS)?;
            cleanup(&mut out, &mux, image_id, false)
        }
        // Flowed grid, but on the alternate screen after an erase.
        "alt-flowed" => {
            out.write_all(b"\x1b[?1049h\x1b[H\x1b[J")?;
            kitty::display_png(
                &png,
                &mut out,
                &mux,
                kitty::Placement::Virtual {
                    image_id,
                    cols: COLS,
                    rows: ROWS,
                },
            )?;
            hold(&mut out)?;
            cleanup(&mut out, &mux, image_id, true)
        }
        // Alternate screen, but with a pause between entering it and
        // transmitting: discriminates "the mode switch races the
        // passthrough" from "the alternate screen blocks it outright".
        "alt-wait" => {
            out.write_all(b"\x1b[?1049h\x1b[H\x1b[J")?;
            out.flush()?;
            std::thread::sleep(Duration::from_millis(1500));
            transmit_and_position(&mut out, &mux, &png, image_id)?;
            hold(&mut out)?;
            cleanup(&mut out, &mux, image_id, true)
        }
        // Alternate screen + erase + positioned rows + dim status line:
        // everything the slideshow's first draw does.
        "alt-positioned" => {
            out.write_all(b"\x1b[?1049h\x1b[H\x1b[J")?;
            transmit_and_position(&mut out, &mux, &png, image_id)?;
            write!(
                out,
                "\x1b[{};1H\x1b[K\x1b[2m probe \x1b[0m",
                ORIGIN_ROW + ROWS + 2
            )?;
            hold(&mut out)?;
            cleanup(&mut out, &mux, image_id, true)
        }
        // The navigation step in isolation: slide A, then -- exactly like
        // the slideshow -- transmit slide B, delete A, erase, draw B's
        // grid; then the same again back to a fresh copy of A. Both
        // "slides" are the logo, drawn at different origins so it is
        // visible which draw is on screen.
        "cycle" => {
            let id_a = image_id;
            let id_b = kitty::pick_image_id(&mux);
            let id_c = kitty::pick_image_id(&mux);
            out.write_all(b"\x1b[?1049h\x1b[H\x1b[J")?;
            out.flush()?;
            std::thread::sleep(Duration::from_millis(1500));

            transmit_and_position(&mut out, &mux, &png, id_a)?;
            status(&mut out, "cycle: slide A (top-left)")?;
            hold(&mut out)?;

            // "Next": new data first, then delete the old, erase, redraw.
            kitty::transmit_virtual(&png, &mut out, &mux, id_b, 1, COLS, ROWS)?;
            kitty::delete_image(&mut out, &mux, id_a, true)?;
            out.write_all(b"\x1b[H\x1b[J")?;
            position_grid_at(&mut out, id_b, ORIGIN_ROW, ORIGIN_COL + 20)?;
            status(&mut out, "cycle: slide B (shifted right)")?;
            hold(&mut out)?;

            // "Prev": same dance back.
            kitty::transmit_virtual(&png, &mut out, &mux, id_c, 1, COLS, ROWS)?;
            kitty::delete_image(&mut out, &mux, id_b, true)?;
            out.write_all(b"\x1b[H\x1b[J")?;
            position_grid_at(&mut out, id_c, ORIGIN_ROW, ORIGIN_COL)?;
            status(&mut out, "cycle: slide C (top-left again)")?;
            hold(&mut out)?;

            cleanup(&mut out, &mux, id_c, true)
        }
        // Navigation with a tmux repaint standing in for the alternate-
        // screen cycle: three slides, each drawn by the slideshow's own
        // dance -- transmit the new data, delete the old image, erase,
        // write the new grid -- and then repaired with `refresh-client`
        // rather than by leaving and re-entering the alternate screen.
        //
        // This is what `refresh` does not settle. That variant repaints a
        // first draw, where nothing has been deleted and no data is in
        // flight; a slide change repaints while an image has just been
        // dropped from the terminal's store and another has just arrived
        // through passthrough. If every slide here renders, the cycle in
        // `slideshow::show` -- and its flash, and its LEAVE_SETTLE -- can
        // be replaced by a repaint.
        "cycle-refresh" => {
            if mux.is_empty() {
                eprintln!("cycle-refresh needs tmux: $TMUX is not set");
                std::process::exit(2);
            }
            let ids = [
                image_id,
                kitty::pick_image_id(&mux),
                kitty::pick_image_id(&mux),
            ];
            out.write_all(b"\x1b[?1049h\x1b[H\x1b[J")?;
            out.flush()?;
            std::thread::sleep(Duration::from_millis(1500));

            let mut previous: Option<u32> = None;
            for (n, &id) in ids.iter().enumerate() {
                // New data first: an `a=T,U=1` transmission is invisible
                // until cells reference it, so it can stream out while the
                // previous slide is still on screen.
                kitty::transmit_virtual(&png, &mut out, &mux, id, 1, COLS, ROWS)?;
                if let Some(old) = previous {
                    kitty::delete_image(&mut out, &mux, old, true)?;
                }
                out.write_all(b"\x1b[H\x1b[J")?;
                // Shift each slide right, so which draw is on screen is
                // visible at a glance.
                position_grid_at(&mut out, id, ORIGIN_ROW, ORIGIN_COL + 20 * n as u16)?;
                status(
                    &mut out,
                    &format!(
                        "cycle-refresh: slide {} of {} -- repaint in progress",
                        n + 1,
                        ids.len()
                    ),
                )?;
                out.flush()?;

                std::thread::sleep(REPAINT_SETTLE);
                let _ = Command::new("tmux")
                    .arg("refresh-client")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                // Let the repaint finish before the next transmission goes
                // out through passthrough.
                std::thread::sleep(REPAINT_SETTLE);
                std::thread::sleep(OBSERVE);
                previous = Some(id);
            }

            cleanup(&mut out, &mux, ids[ids.len() - 1], true)?;
            eprintln!(
                "cycle-refresh: all three slides rendered => a repaint is enough for slide \
                 changes too, and the alternate-screen cycle can go. Any slide blank or \
                 clipped => the cycle is doing something the repaint does not, and \
                 refresh-client can only be added to it, not swapped for it."
            );
            Ok(())
        }
        // The repaint bisection. All four draw the same known-problematic
        // shape -- alternate screen, transmit, positioned grid -- hold it
        // long enough to see whether it came out wrong, apply exactly one
        // nudge, and hold again. Nothing is written after the nudge: any
        // further output is itself terminal traffic and would confound the
        // result, which is also why the caption goes up front.
        "manual" | "refresh" | "nudge" | "rewrite" => {
            if variant == "refresh" && mux.is_empty() {
                eprintln!("refresh needs tmux: $TMUX is not set");
                std::process::exit(2);
            }
            out.write_all(b"\x1b[?1049h\x1b[H\x1b[J")?;
            out.flush()?;
            std::thread::sleep(Duration::from_millis(1500));

            transmit_and_position(&mut out, &mux, &png, image_id)?;
            status(&mut out, &caption(&variant))?;
            out.flush()?;
            std::thread::sleep(if variant == "manual" {
                MANUAL_OBSERVE
            } else {
                OBSERVE
            });

            match variant.as_str() {
                // What switching panes does: repaint every visible pane
                // out of tmux's own grid. No image data is re-sent -- the
                // terminal already holds it, and the placeholder cells are
                // ordinary text in tmux's grid. They survive the repaint
                // because their image ID rides a 256-colour SGR, which a
                // multiplexer relays verbatim (see `IdSpace::MuxSafe`).
                "refresh" => {
                    let _ = Command::new("tmux")
                        .arg("refresh-client")
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                // One cell of traffic, far from the grid, reaching the
                // terminal through tmux like any other pane output. It
                // changes nothing about the image or the cells drawing
                // it, so it can only repair a frame the terminal itself
                // failed to paint.
                "nudge" => {
                    write!(out, "\x1b[{};1H\x1b[2m.\x1b[0m", ORIGIN_ROW + ROWS + 4)?;
                    out.flush()?;
                }
                // The placeholder cells re-sent exactly as they were, as
                // an in-place pane update.
                "rewrite" => {
                    position_grid_at(&mut out, image_id, ORIGIN_ROW, ORIGIN_COL)?;
                    out.flush()?;
                }
                // "manual": the nudge is the pane switch, done by hand
                // during the wait that just passed.
                _ => {}
            }

            std::thread::sleep(OBSERVE);
            cleanup(&mut out, &mux, image_id, true)?;
            eprintln!("{}", verdict(&variant));
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: placeholder_probe \
                 <flowed|positioned|alt-flowed|alt-wait|alt-positioned|cycle\
                 |manual|refresh|nudge|rewrite|cycle-refresh>"
            );
            std::process::exit(2);
        }
    }
}

/// What the nudge variants put on screen before the wait, since nothing may
/// be written afterwards.
fn caption(variant: &str) -> String {
    match variant {
        "manual" => format!(
            "manual: switch to another pane and back within {}s -- does the image complete?",
            MANUAL_OBSERVE.as_secs()
        ),
        "refresh" => format!(
            "refresh: `tmux refresh-client` fires in {}s",
            OBSERVE.as_secs()
        ),
        "nudge" => format!(
            "nudge: one unrelated cell is written in {}s",
            OBSERVE.as_secs()
        ),
        _ => format!(
            "rewrite: the grid cells are re-sent in {}s",
            OBSERVE.as_secs()
        ),
    }
}

/// How to read what just happened, printed once the screen is restored.
fn verdict(variant: &str) -> &'static str {
    match variant {
        "manual" => {
            "manual: if the image was wrong and the pane switch completed it, this draw \
             reproduces the bug and the other three variants mean something. If it looked \
             right all along, the probe is not reproducing what the slideshow hits."
        }
        "refresh" => {
            "refresh: completed => a full tmux client redraw is the fix, and the slideshow \
             can call refresh-client after each draw instead of cycling the alternate \
             screen -- no flash, no LEAVE_SETTLE."
        }
        "nudge" => {
            "nudge: completed => the terminal painted a stale frame; any trivial traffic \
             after a draw is enough, and no tmux involvement is needed."
        }
        _ => {
            "rewrite: completed => re-sending the placeholder cells in place is enough, \
             which is the cheapest fix of the three."
        }
    }
}

/// Write a dim status line below the grid area.
fn status(out: &mut impl Write, text: &str) -> io::Result<()> {
    write!(
        out,
        "\x1b[{};1H\x1b[K\x1b[2m {text} \x1b[0m",
        ORIGIN_ROW + ROWS + 2
    )
}

/// Write `image_id`'s grid rows at an explicit origin.
fn position_grid_at(out: &mut impl Write, image_id: u32, row: u16, col: u16) -> io::Result<()> {
    let mut grid = String::new();
    for r in 0..ROWS {
        grid.push_str(&format!("\x1b[{};{}H", row + r, col));
        placeholder::write_row(&mut grid, image_id, Some(1), COLS, r);
    }
    out.write_all(grid.as_bytes())
}

/// Transmit with `a=T,U=1` and write the grid rows via cursor addressing,
/// the way the slideshow does.
fn transmit_and_position(
    out: &mut impl Write,
    mux: &[Mux],
    png: &[u8],
    image_id: u32,
) -> io::Result<()> {
    kitty::transmit_virtual(png, out, mux, image_id, 1, COLS, ROWS)?;
    position_grid_at(out, image_id, ORIGIN_ROW, ORIGIN_COL)
}

/// Flush, then leave the image on screen long enough to look at.
fn hold(out: &mut impl Write) -> io::Result<()> {
    out.flush()?;
    std::thread::sleep(Duration::from_secs(4));
    Ok(())
}

/// Delete the probe's image and put the screen back.
fn cleanup(out: &mut impl Write, mux: &[Mux], image_id: u32, alt: bool) -> io::Result<()> {
    kitty::delete_image(out, mux, image_id, true)?;
    if alt {
        out.write_all(b"\x1b[?1049l")?;
    }
    out.write_all(b"\n")?;
    out.flush()
}
