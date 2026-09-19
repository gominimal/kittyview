// SPDX-License-Identifier: Apache-2.0

//! Fuzz the slideshow's key decoder.
//!
//! In raw mode every byte the terminal delivers -- keys, mouse noise,
//! stray reply fragments -- flows through `read_key`. The decoder must
//! never panic and never read past what the source hands it, whatever the
//! byte stream looks like.

#![no_main]

use kittyview::slideshow::{ByteSource, read_key};
use libfuzzer_sys::fuzz_target;
use std::time::Duration;

/// Hands out the fuzz input one byte at a time; exhaustion is a timeout.
struct Feed<'a>(&'a [u8]);

impl ByteSource for Feed<'_> {
    fn next_byte(&mut self, _timeout: Duration) -> Option<u8> {
        let (&byte, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(byte)
    }
}

fuzz_target!(|data: &[u8]| {
    let mut source = Feed(data);
    // Drain the whole stream: every byte must decode as some key or be
    // discarded, and the loop must terminate once the input runs out.
    while read_key(&mut source, Duration::ZERO).is_some() {}
});
