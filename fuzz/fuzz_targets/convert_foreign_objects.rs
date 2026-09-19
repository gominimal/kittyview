// SPDX-License-Identifier: Apache-2.0

//! Fuzz the SVG <foreignObject> preprocessor, which parses untrusted XML and
//! rewrites embedded HTML into plain <text> elements (see SECURITY.md).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = kittyview::svg::convert_foreign_objects(data);
});
