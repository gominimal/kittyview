// SPDX-License-Identifier: Apache-2.0

//! Fuzz the hand-written PNG IHDR parser used for geometry calculations.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = kittyview::geometry::png_dimensions(data);
});
