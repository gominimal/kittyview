// SPDX-License-Identifier: Apache-2.0

//! Unicode placeholders for kitty virtual image placements.
//!
//! A virtual placement is anchored to real text cells rather than to a screen
//! position: the terminal draws the image over cells containing `U+10EEEE`,
//! whose row/column within the image is encoded in combining diacritics and
//! whose image ID is encoded in the foreground colour. Because the anchor is
//! ordinary text, anything that manages text -- tmux, screen, a pager, the
//! shell's own scrollback -- moves and scrolls the image correctly without
//! knowing anything about the graphics protocol.

use std::fmt::Write as _;

/// The Unicode placeholder character.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Diacritics encoding row/column numbers, from kitty's
/// `gen/rowcolumn-diacritics.txt`. Index N is the diacritic for the number N.
#[rustfmt::skip]
pub const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F,
    0x0346, 0x034A, 0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357,
    0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484,
    0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597,
    0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611,
    0x0612, 0x0613, 0x0614, 0x0615, 0x0616, 0x0617, 0x0657, 0x0658,
    0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
    0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733,
    0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE,
    0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817, 0x0818, 0x0819,
    0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C,
    0x082D, 0x0951, 0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87,
    0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D,
    0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
    0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1,
    0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5, 0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9,
    0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1,
    0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7,
    0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA,
    0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2,
    0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D,
    0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5,
    0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7,
    0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23,
    0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187,
    0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243,
    0x1D244,
];

/// The largest row or column index that can be encoded.
pub const MAX_INDEX: usize = DIACRITICS.len() - 1;

/// The diacritic encoding `n`, or `None` if `n` exceeds the table.
pub fn diacritic(n: usize) -> Option<char> {
    DIACRITICS.get(n).and_then(|&c| char::from_u32(c))
}

/// How much of an image ID the placeholder cells can carry intact.
///
/// An ID is split across two carriers: its low bits ride in the cells'
/// foreground colour, and its top byte rides in a third combining diacritic.
/// The diacritic is ordinary text and survives anything that handles text, so
/// the only question is which foreground colour form gets through -- which
/// depends on what sits between us and the terminal.
///
/// Both spaces are used the same way: draw an index at random and hand it to
/// [`IdSpace::id_at`]. IDs are global to the terminal session and shared with
/// every other program drawing into it, and transmitting under an ID that is
/// already in use replaces that image and drops the placements drawing it --
/// blanking an image that may still be sitting in scrollback. Nothing
/// coordinates the namespace, so the size of the space is the whole defence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdSpace {
    /// Low byte only, carried by a 256-colour SGR: 65 280 IDs.
    ///
    /// Multiplexers relay 256-colour SGR verbatim, but rewrite truecolor when
    /// the outer terminal does not advertise it. A rewritten colour is a
    /// different image ID, so the placeholder cells would name an image that
    /// was never transmitted and nothing would be drawn at all.
    MuxSafe,
    /// Low 24 bits, carried by a truecolor SGR: 4 294 967 040 IDs.
    ///
    /// For output going straight to the terminal, where the colour arrives as
    /// written.
    Full,
}

impl IdSpace {
    /// The number of distinct IDs in this space. None of them is zero.
    pub const fn capacity(self) -> u32 {
        // Every value of the top byte, paired with every non-zero low half.
        self.low_span() * 256
    }

    /// The `n`th ID in this space, with `n` reduced modulo
    /// [`capacity`](IdSpace::capacity).
    pub const fn id_at(self, n: u32) -> u32 {
        let span = self.low_span();
        let n = n % self.capacity();
        // The low half is 1-based: an image ID of zero means "no image", and
        // it keeps the ID non-zero whatever the top byte is.
        ((n / span) << 24) | (n % span + 1)
    }

    /// How many non-zero values the foreground colour can carry.
    const fn low_span(self) -> u32 {
        match self {
            IdSpace::MuxSafe => 0xFF,
            IdSpace::Full => 0x00FF_FFFF,
        }
    }
}

/// SGR sequence selecting `image_id`'s low 24 bits as the foreground colour.
///
/// IDs that fit in a byte use the 256-colour form, which multiplexers relay
/// verbatim; larger IDs need the 24-bit form, which tmux may degrade when the
/// outer terminal lacks truecolor. [`IdSpace`] is how callers choose which of
/// those they are relying on.
fn fg_sgr(image_id: u32) -> String {
    let low = image_id & 0x00FF_FFFF;
    if low <= 0xFF {
        format!("\x1b[38;5;{low}m")
    } else {
        format!(
            "\x1b[38;2;{};{};{}m",
            (low >> 16) & 0xFF,
            (low >> 8) & 0xFF,
            low & 0xFF
        )
    }
}

/// Write the placeholder grid for `image_id` covering `cols` x `rows` cells.
///
/// Rows are separated by newlines; no trailing newline is written, so the
/// caller decides how the line the image ends on is terminated. The grid is
/// plain text and must NOT be wrapped in multiplexer passthrough -- the
/// multiplexer has to see these cells to be able to scroll them.
pub fn write_grid(buf: &mut String, image_id: u32, cols: u16, rows: u16) {
    let cols = (cols as usize).min(MAX_INDEX + 1);
    let rows = (rows as usize).min(MAX_INDEX + 1);
    if cols == 0 || rows == 0 {
        return;
    }

    let fg = fg_sgr(image_id);
    // The most significant byte of the ID rides in a third diacritic, since
    // the foreground colour only carries 24 bits.
    let msb = ((image_id >> 24) & 0xFF) as usize;
    let msb_diacritic = if msb == 0 { None } else { diacritic(msb) };

    for row in 0..rows {
        if row > 0 {
            buf.push('\n');
        }
        buf.push_str(&fg);
        let Some(row_d) = diacritic(row) else { break };
        for col in 0..cols {
            let Some(col_d) = diacritic(col) else { break };
            buf.push(PLACEHOLDER);
            buf.push(row_d);
            buf.push(col_d);
            if let Some(d) = msb_diacritic {
                buf.push(d);
            }
        }
        // Restore the default foreground so following text is unaffected.
        let _ = write!(buf, "\x1b[39m");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritic_table_matches_kitty_spec() {
        assert_eq!(DIACRITICS.len(), 297);
        // Values quoted in the kitty protocol documentation.
        assert_eq!(diacritic(0), Some('\u{0305}'));
        assert_eq!(diacritic(1), Some('\u{030D}'));
        assert_eq!(diacritic(2), Some('\u{030E}'));
        assert_eq!(diacritic(296), Some('\u{1D244}'));
        assert_eq!(diacritic(297), None);
    }

    #[test]
    fn diacritics_are_unique() {
        let mut sorted = DIACRITICS;
        sorted.sort_unstable();
        let mut deduped = sorted.to_vec();
        deduped.dedup();
        assert_eq!(deduped.len(), DIACRITICS.len());
    }

    #[test]
    fn eight_bit_ids_use_256_colour_form() {
        assert_eq!(fg_sgr(42), "\x1b[38;5;42m");
        assert_eq!(fg_sgr(255), "\x1b[38;5;255m");
    }

    #[test]
    fn larger_ids_use_truecolor_form() {
        assert_eq!(fg_sgr(0x010203), "\x1b[38;2;1;2;3m");
        // The most significant byte is carried by a diacritic, not the colour.
        assert_eq!(fg_sgr(0x02_000042), "\x1b[38;5;66m");
    }

    #[test]
    fn id_space_capacities() {
        assert_eq!(IdSpace::MuxSafe.capacity(), 255 * 256);
        assert_eq!(IdSpace::Full.capacity(), 0x00FF_FFFF * 256);
    }

    #[test]
    fn mux_safe_ids_all_use_the_256_colour_form() {
        // The whole point of the narrow space: whatever index we land on, the
        // colour carrying it is one a multiplexer relays untouched.
        for n in 0..IdSpace::MuxSafe.capacity() {
            let id = IdSpace::MuxSafe.id_at(n);
            assert!(
                fg_sgr(id).starts_with("\x1b[38;5;"),
                "index {n} gave {id:#x}"
            );
        }
    }

    #[test]
    fn mux_safe_space_is_exhausted_before_an_id_repeats() {
        let capacity = IdSpace::MuxSafe.capacity();
        let ids: std::collections::HashSet<u32> =
            (0..capacity).map(|n| IdSpace::MuxSafe.id_at(n)).collect();
        assert_eq!(ids.len(), capacity as usize);
    }

    #[test]
    fn full_space_ids_are_distinct() {
        // Walking 4.29e9 indices would be silly; a uniform stride across the
        // space reaches every top byte instead.
        let stride = IdSpace::Full.capacity() / 100_000;
        let ids: std::collections::HashSet<u32> = (0..100_000)
            .map(|i| IdSpace::Full.id_at(i * stride))
            .collect();
        assert_eq!(ids.len(), 100_000);
    }

    #[test]
    fn ids_are_never_zero_and_always_encodable() {
        for space in [IdSpace::MuxSafe, IdSpace::Full] {
            let capacity = space.capacity();
            for n in [0, 1, 254, 255, 256, capacity - 1, capacity, capacity + 7] {
                let id = space.id_at(n);
                assert_ne!(id, 0, "{space:?} at index {n}");
                // The top byte has to name a diacritic or it cannot be sent.
                let top = ((id >> 24) & 0xFF) as usize;
                assert!(diacritic(top).is_some(), "{space:?} top byte {top}");
            }
        }
    }

    #[test]
    fn id_at_wraps_at_capacity() {
        for space in [IdSpace::MuxSafe, IdSpace::Full] {
            assert_eq!(space.id_at(space.capacity()), space.id_at(0));
            assert_eq!(space.id_at(space.capacity() + 9), space.id_at(9));
        }
    }

    #[test]
    fn grid_matches_documented_example() {
        // The kitty docs' 2x2 placeholder for image ID 42.
        let mut buf = String::new();
        write_grid(&mut buf, 42, 2, 2);
        let expected = "\x1b[38;5;42m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}\x1b[39m\n\
                        \x1b[38;5;42m\u{10EEEE}\u{030D}\u{0305}\u{10EEEE}\u{030D}\u{030D}\x1b[39m";
        assert_eq!(buf, expected);
    }

    #[test]
    fn grid_encodes_msb_as_third_diacritic() {
        // ID 33554474 = 42 + (2 << 24), also from the kitty docs.
        let mut buf = String::new();
        write_grid(&mut buf, 33_554_474, 2, 1);
        let expected = "\x1b[38;5;42m\
                        \u{10EEEE}\u{0305}\u{0305}\u{030E}\
                        \u{10EEEE}\u{0305}\u{030D}\u{030E}\x1b[39m";
        assert_eq!(buf, expected);
    }

    #[test]
    fn grid_has_no_trailing_newline() {
        let mut buf = String::new();
        write_grid(&mut buf, 1, 3, 4);
        assert!(!buf.ends_with('\n'));
        assert_eq!(buf.matches('\n').count(), 3, "rows - 1 separators");
    }

    #[test]
    fn grid_cell_count_matches_dimensions() {
        let mut buf = String::new();
        write_grid(&mut buf, 7, 5, 3);
        assert_eq!(buf.matches(PLACEHOLDER).count(), 15);
    }

    #[test]
    fn grid_resets_foreground_each_row() {
        let mut buf = String::new();
        write_grid(&mut buf, 9, 2, 3);
        assert_eq!(buf.matches("\x1b[39m").count(), 3);
        assert!(buf.ends_with("\x1b[39m"));
    }

    #[test]
    fn zero_dimensions_emit_nothing() {
        let mut buf = String::new();
        write_grid(&mut buf, 1, 0, 5);
        write_grid(&mut buf, 1, 5, 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn oversized_dimensions_are_clamped_to_the_table() {
        let mut buf = String::new();
        write_grid(&mut buf, 1, 5000, 1);
        assert_eq!(buf.matches(PLACEHOLDER).count(), MAX_INDEX + 1);
    }
}
