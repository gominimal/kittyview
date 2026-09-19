// SPDX-License-Identifier: Apache-2.0

//! Fuzz the hand-written parsers for terminal replies.
//!
//! Everything here parses bytes the terminal (or a multiplexer between us
//! and it) sent back: XTVERSION and DA2 identification replies, XTWINOPS
//! geometry reports, kitty graphics capability answers, the
//! response-completeness scanner that decides when to stop reading, and
//! the slideshow's transmission-check verdict. A hostile or broken
//! terminal controls all of it.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = kittyview::terminal::parse_xtversion(data);
    let _ = kittyview::terminal::parse_da2(data);
    let _ = kittyview::terminal::parse_graphics_probe(data);
    let _ = kittyview::terminal::is_response_complete(data);
    // The two report kinds geometry detection actually asks for.
    let _ = kittyview::geometry::parse_xtwinops(data, 6);
    let _ = kittyview::geometry::parse_xtwinops(data, 4);
    let _ = kittyview::slideshow::verify_reply_says_arrived(Some(data));
});
