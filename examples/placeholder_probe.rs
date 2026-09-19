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
//! This is a debugging aid, not part of the kittyview CLI.

use kittyview::terminal::Mux;
use kittyview::{kitty, logo, placeholder};
use std::io::{self, Write};
use std::time::Duration;

const COLS: u16 = 16;
const ROWS: u16 = 8;
/// Where the positioned variants put the grid: away from column 1, like the
/// slideshow's centring does.
const ORIGIN_ROW: u16 = 3;
const ORIGIN_COL: u16 = 5;

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
        _ => {
            eprintln!(
                "usage: placeholder_probe \
                 <flowed|positioned|alt-flowed|alt-wait|alt-positioned|cycle>"
            );
            std::process::exit(2);
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
