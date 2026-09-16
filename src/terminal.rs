// SPDX-License-Identifier: Apache-2.0

//! Terminal and multiplexer detection.
//!
//! Identifies the terminal emulator and any multiplexer layers (tmux, screen,
//! Zellij) using in-band escape sequence queries (XTVERSION, DA2) with an
//! environment variable fallback. Supports nested multiplexers (e.g.
//! tmux-in-tmux).
//!
//! Where a name does not settle whether images can be drawn, a kitty graphics
//! capability query asks the question directly, along the same path the image
//! will travel.

use std::io::{self, IsTerminal};
use std::time::{Duration, Instant};

// ─── Public types ───────────────────────────────────────────

/// A detected terminal emulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    Kitty(Option<String>),
    Ghostty(Option<String>),
    WezTerm(Option<String>),
    Foot(Option<String>),
    Konsole(Option<String>),
    ITerm2,
    Xterm(Option<String>),
    Mintty(Option<String>),
    Contour(Option<String>),
    Vte,
    Other(String),
    Unknown,
}

/// A detected multiplexer layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mux {
    Tmux(Option<String>),
    Screen(Option<String>),
    /// Zellij, which from 0.45.0 draws kitty graphics itself rather than
    /// passing them through to the terminal underneath.
    Zellij(Option<String>),
}

/// What the kitty graphics capability query came back with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsProbe {
    /// Answered `OK`: whatever is on the other end draws kitty graphics.
    Confirmed,
    /// Answered with an error. Zellij replies `ENOTSUPPORTED` here when the
    /// terminal *it* runs in cannot display images.
    Refused,
    /// Nothing came back, or only the barrier did.
    Unanswered,
    /// Never asked, because the terminal's own name already settled it.
    Skipped,
}

/// First Zellij release implementing the kitty graphics protocol.
const ZELLIJ_KITTY_GRAPHICS: (u32, u32, u32) = (0, 45, 0);

/// Whether a Zellij version implements the kitty graphics protocol.
///
/// A version we could not read is given the benefit of the doubt, since the
/// capability query has the final say either way.
pub fn zellij_implements_graphics(version: Option<&str>) -> bool {
    match version.and_then(semver_triple) {
        Some(v) => v >= ZELLIJ_KITTY_GRAPHICS,
        None => true,
    }
}

fn semver_triple(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Some((major, minor, patch))
}

/// Full terminal detection result.
#[derive(Debug, Clone)]
pub struct TerminalInfo {
    /// Multiplexer stack, innermost first.
    /// E.g. for tmux-in-tmux: `[Tmux, Tmux]` where [0] is the session we're
    /// directly inside and [1] is the outer one.
    pub mux_stack: Vec<Mux>,
    pub terminal: Terminal,
    /// What the capability query answered, where one was asked.
    pub graphics: GraphicsProbe,
}

impl TerminalInfo {
    /// A result resting on identity alone, with no capability query behind it.
    pub fn unprobed(mux_stack: Vec<Mux>, terminal: Terminal) -> Self {
        Self {
            mux_stack,
            terminal,
            graphics: GraphicsProbe::Skipped,
        }
    }

    /// Whether Zellij is one of the multiplexer layers.
    pub fn in_zellij(&self) -> bool {
        self.mux_stack.iter().any(|m| matches!(m, Mux::Zellij(_)))
    }

    /// The version Zellij reported, when we are inside one that named itself.
    pub fn zellij_version(&self) -> Option<&str> {
        self.mux_stack.iter().find_map(|m| match m {
            Mux::Zellij(v) => v.as_deref(),
            _ => None,
        })
    }

    /// Whether this terminal is one we expect to answer the capability query,
    /// and can therefore read silence from as a bad sign. Konsole and iTerm2
    /// implement the protocol without necessarily answering for it.
    pub fn answers_capability_query(&self) -> bool {
        matches!(
            self.terminal,
            Terminal::Kitty(_) | Terminal::Ghostty(_) | Terminal::WezTerm(_)
        )
    }

    /// Whether the kitty graphics protocol works here.
    ///
    /// A confirmed capability query settles it: whatever is on the other end
    /// answered for itself, which is worth more than recognising a name. A
    /// query that came back negative never overrides a name, though -- a
    /// terminal can implement the protocol without answering for it, and a
    /// multiplexer can swallow the reply to a query it forwarded perfectly
    /// well.
    ///
    /// Zellij is the exception, and gets no benefit of the doubt: it is what
    /// receives the escape sequences, so only it can say whether they will be
    /// drawn. The host terminal's name -- which leaks into panes through
    /// inherited environment variables -- answers a question nobody asked.
    pub fn supports_kitty_graphics(&self) -> bool {
        if self.graphics == GraphicsProbe::Confirmed {
            return true;
        }
        if self.in_zellij() {
            return false;
        }
        matches!(
            self.terminal,
            Terminal::Kitty(_)
                | Terminal::Ghostty(_)
                | Terminal::WezTerm(_)
                | Terminal::Konsole(_)
                | Terminal::ITerm2
        )
    }

    /// Whether the detected terminal can display images anchored to Unicode
    /// placeholder cells (kitty graphics `U=1`).
    ///
    /// Terminals we could not identify are given the benefit of the doubt:
    /// detection is skipped entirely under `--force` and an explicit
    /// `--passthrough`, and placeholders are what make images behave the same
    /// way everywhere.
    pub fn supports_unicode_placeholders(&self) -> bool {
        // Zellij rejects a placement carrying `U=1` outright, and kittyview
        // asks for no reply, so the rejection is silent: the placeholder cells
        // that follow arrive as ordinary text and are drawn as literal
        // glyphs, replacing the image with a rectangle of U+10EEEE.
        if self.in_zellij() {
            return false;
        }
        !matches!(self.terminal, Terminal::Konsole(_) | Terminal::ITerm2)
    }

    /// Whether output needs DCS passthrough wrapping.
    #[allow(dead_code)]
    pub fn needs_passthrough(&self) -> bool {
        self.mux_stack
            .iter()
            .any(|m| matches!(m, Mux::Tmux(_) | Mux::Screen(_)))
    }
}

impl std::fmt::Display for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Terminal::Kitty(v) => write!(f, "kitty{}", ver(v)),
            Terminal::Ghostty(v) => write!(f, "Ghostty{}", ver(v)),
            Terminal::WezTerm(v) => write!(f, "WezTerm{}", ver(v)),
            Terminal::Foot(v) => write!(f, "foot{}", ver(v)),
            Terminal::Konsole(v) => write!(f, "Konsole{}", ver(v)),
            Terminal::ITerm2 => write!(f, "iTerm2"),
            Terminal::Xterm(v) => write!(f, "xterm{}", ver(v)),
            Terminal::Mintty(v) => write!(f, "mintty{}", ver(v)),
            Terminal::Contour(v) => write!(f, "contour{}", ver(v)),
            Terminal::Vte => write!(f, "VTE"),
            Terminal::Other(s) => write!(f, "{s}"),
            Terminal::Unknown => write!(f, "unknown"),
        }
    }
}

impl std::fmt::Display for Mux {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mux::Tmux(v) => write!(f, "tmux{}", ver(v)),
            Mux::Screen(v) => write!(f, "screen{}", ver(v)),
            Mux::Zellij(v) => write!(f, "Zellij{}", ver(v)),
        }
    }
}

fn ver(v: &Option<String>) -> String {
    match v {
        Some(s) => format!(" {s}"),
        None => String::new(),
    }
}

// ─── DCS wrapping (shared with kitty.rs) ────────────────────

/// Wrap data for a single multiplexer layer.
///
/// - **tmux**: double every ESC, wrap in `ESC P tmux ; … ESC \`
/// - **screen**: prefix with `ESC P` (inner ST terminates the DCS)
/// - **Zellij**: return unchanged (transparent)
pub fn wrap_for_mux(data: &[u8], mux: &Mux) -> Vec<u8> {
    match mux {
        Mux::Tmux(_) => {
            let mut out = Vec::with_capacity(data.len() * 2 + 10);
            out.extend_from_slice(b"\x1bPtmux;");
            for &byte in data {
                if byte == 0x1b {
                    out.push(0x1b);
                    out.push(0x1b);
                } else {
                    out.push(byte);
                }
            }
            out.extend_from_slice(b"\x1b\\");
            out
        }
        Mux::Screen(_) => {
            let mut out = Vec::with_capacity(data.len() + 2);
            out.extend_from_slice(b"\x1bP");
            out.extend_from_slice(data);
            out
        }
        Mux::Zellij(_) => data.to_vec(),
    }
}

/// Wrap data through an entire mux stack (innermost first).
///
/// Each layer wraps the output of the previous layer. The innermost mux
/// is applied first, then the next layer wraps that result, and so on.
pub fn wrap_for_stack(data: &[u8], stack: &[Mux]) -> Vec<u8> {
    let mut result = data.to_vec();
    for mux in stack {
        result = wrap_for_mux(&result, mux);
    }
    result
}

// ─── Escape sequence queries ────────────────────────────────

/// XTVERSION query: `ESC [ > 0 q`
const XTVERSION: &[u8] = b"\x1b[>0q";

/// DA2 (Secondary Device Attributes) query: `ESC [ > c`
const DA2: &[u8] = b"\x1b[>c";

/// Kitty graphics capability query: a query-only (`a=q`) transmission of a
/// 1x1 RGB image, followed by a primary DA.
///
/// A terminal implementing the protocol answers `ESC _ G i=31;OK ESC \`; one
/// that does not ignores the APC sequence entirely. That is why the DA goes
/// out in the same write: every terminal answers it, so the negative case
/// resolves on a reply rather than on the timeout.
const KITTY_GRAPHICS_QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// Maximum nesting depth to probe (bounds detection time).
const MAX_MUX_DEPTH: usize = 4;

/// How long to wait for a reply that arrives after the barrier.
const LATE_REPLY_TIMEOUT: Duration = Duration::from_millis(300);

// ─── Response parsers ───────────────────────────────────────

/// Parsed DA2 response parameters.
struct Da2Info {
    pp: u32,
    pv: u32,
}

/// Parse an XTVERSION response: `ESC P > | name(version) ST`
///
/// Returns the terminal (or mux) identified by the name string.
fn parse_xtversion(response: &[u8]) -> Option<(Terminal, Option<Mux>)> {
    // Find the DCS payload start marker ">|"
    let marker = response.windows(2).position(|w| w == b">|")?;
    let payload_start = marker + 2;

    // Find ST terminator: ESC \ or 0x9C
    let payload_end = find_st(response, payload_start)?;
    let name = std::str::from_utf8(&response[payload_start..payload_end]).ok()?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    Some(identify_xtversion(name))
}

/// Identify terminal/mux from an XTVERSION name string.
fn identify_xtversion(name: &str) -> (Terminal, Option<Mux>) {
    let lower = name.to_ascii_lowercase();

    if lower.starts_with("tmux") {
        let ver = extract_version_space(name);
        return (Terminal::Unknown, Some(Mux::Tmux(ver)));
    }
    if lower.starts_with("zellij") {
        let version = extract_version_parens(name).and_then(|v| decode_zellij_version(&v));
        return (Terminal::Unknown, Some(Mux::Zellij(version)));
    }
    if lower.starts_with("kitty") {
        return (Terminal::Kitty(extract_version_parens(name)), None);
    }
    if lower.starts_with("ghostty") {
        return (Terminal::Ghostty(extract_version_parens(name)), None);
    }
    if lower.starts_with("wezterm") {
        return (Terminal::WezTerm(extract_version_space(name)), None);
    }
    if lower.starts_with("foot") {
        return (Terminal::Foot(extract_version_parens(name)), None);
    }
    if lower.starts_with("xterm") {
        return (Terminal::Xterm(extract_version_parens(name)), None);
    }
    if lower.starts_with("contour") {
        return (Terminal::Contour(extract_version_parens(name)), None);
    }
    if lower.starts_with("mintty") {
        return (Terminal::Mintty(extract_version_space(name)), None);
    }

    (Terminal::Other(name.to_string()), None)
}

/// Decode the version out of Zellij's XTVERSION reply.
///
/// Zellij reports the packed integer its own `version_number()` produces --
/// two decimal digits per semver component, most significant first -- so
/// 0.45.1 arrives as `Zellij(4501)`. A dotted version is passed through
/// unchanged, in case a later release reports one.
fn decode_zellij_version(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.contains('.') {
        return Some(raw.to_string());
    }
    let packed: u32 = raw.parse().ok()?;
    Some(format!(
        "{}.{}.{}",
        packed / 10_000,
        (packed / 100) % 100,
        packed % 100
    ))
}

/// Extract version from "name(version)" format.
fn extract_version_parens(name: &str) -> Option<String> {
    let start = name.find('(')?;
    let end = name.find(')')?;
    if end > start + 1 {
        Some(name[start + 1..end].to_string())
    } else {
        None
    }
}

/// Extract version from "name version" format (everything after first space).
fn extract_version_space(name: &str) -> Option<String> {
    let pos = name.find(' ')?;
    let ver = name[pos + 1..].trim();
    if ver.is_empty() {
        None
    } else {
        Some(ver.to_string())
    }
}

/// Find the String Terminator (ST) starting from `from`.
/// ST is either ESC `\` (two bytes: 0x1B 0x5C) or the 8-bit form 0x9C.
fn find_st(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < data.len() {
        if data[i] == 0x9c {
            return Some(i);
        }
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a DA2 response: `ESC [ > Pp ; Pv ; Pc c`
fn parse_da2(response: &[u8]) -> Option<Da2Info> {
    let start = response.windows(3).position(|w| w == b"\x1b[>")?;
    let params_start = start + 3;

    let end = response[params_start..].iter().position(|&b| b == b'c')?;
    let params_str = std::str::from_utf8(&response[params_start..params_start + end]).ok()?;

    let parts: Vec<&str> = params_str.split(';').collect();
    if parts.len() < 2 {
        return None;
    }

    let pp = parts[0].trim().parse().ok()?;
    let pv = parts[1].trim().parse().ok()?;
    Some(Da2Info { pp, pv })
}

/// Map DA2 parameters to a terminal/mux identity.
fn terminal_from_da2(info: &Da2Info) -> (Terminal, Option<Mux>) {
    match info.pp {
        84 => (Terminal::Unknown, Some(Mux::Tmux(None))),
        83 => (Terminal::Unknown, Some(Mux::Screen(None))),
        1 if info.pv >= 4000 => (Terminal::Kitty(Some(format!("{}", info.pv / 1000))), None),
        41 => (Terminal::Xterm(Some(info.pv.to_string())), None),
        65 => (Terminal::Vte, None),
        77 => (Terminal::Mintty(None), None),
        _ => (Terminal::Unknown, None),
    }
}

// ─── In-band detection (querier abstraction) ────────────────

/// Trait for sending terminal queries and receiving responses.
/// Allows mocking in tests.
trait TerminalQuerier {
    fn query(&mut self, request: &[u8], timeout: Duration) -> Option<Vec<u8>>;

    /// Read again without sending anything, for the case where one query
    /// draws two replies and the first to arrive is not the interesting one.
    fn read_more(&mut self, _timeout: Duration) -> Option<Vec<u8>> {
        None
    }

    /// Discard anything the terminal sent that we did not read, so it does
    /// not surface as input to whatever runs next.
    fn discard_pending(&mut self) {}
}

/// Read the verdict out of a reply to [`KITTY_GRAPHICS_QUERY`].
///
/// The reply is an APC sequence whose payload is `keys;status`, where the
/// status is `OK` or an error code such as `ENOTSUPPORTED`. Anything else --
/// the bare DA barrier, nothing at all -- means the question went unanswered.
fn parse_graphics_probe(response: &[u8]) -> GraphicsProbe {
    let Some(start) = response.windows(3).position(|w| w == b"\x1b_G") else {
        return GraphicsProbe::Unanswered;
    };
    let payload_start = start + 3;
    let Some(payload_end) = find_st(response, payload_start) else {
        return GraphicsProbe::Unanswered;
    };
    let payload = &response[payload_start..payload_end];
    let Some(separator) = payload.iter().position(|&b| b == b';') else {
        return GraphicsProbe::Unanswered;
    };
    if payload[separator + 1..].starts_with(b"OK") {
        GraphicsProbe::Confirmed
    } else {
        GraphicsProbe::Refused
    }
}

/// Ask whether kitty graphics can be drawn, through the whole mux stack.
fn probe_graphics(q: &mut impl TerminalQuerier, stack: &[Mux], timeout: Duration) -> GraphicsProbe {
    let request = wrap_for_stack(KITTY_GRAPHICS_QUERY, stack);
    let mut verdict = q
        .query(&request, timeout)
        .map_or(GraphicsProbe::Unanswered, |r| parse_graphics_probe(&r));

    // A multiplexer can answer the DA barrier itself, out of the terminal's
    // way and ahead of it, which ends the read before the reply we are
    // actually waiting for has arrived. Take one more look before concluding
    // that nothing is coming.
    if verdict == GraphicsProbe::Unanswered && !stack.is_empty() {
        if let Some(late) = q.read_more(LATE_REPLY_TIMEOUT) {
            verdict = parse_graphics_probe(&late);
        }
    }

    // Whatever of the reply we did not read -- the barrier, usually -- would
    // otherwise land on the shell prompt after we exit.
    q.discard_pending();
    verdict
}

/// Run the capability query wherever its answer could change the outcome.
///
/// A terminal identified by name, with nothing wrapped around it, has already
/// answered the question; asking again could only confirm what we know or
/// return a negative we would not act on. Everywhere else -- inside Zellij,
/// under a multiplexer whose passthrough may or may not deliver, in front of
/// a terminal we cannot name -- the query travels the exact path the image
/// will travel, and tests what no name can.
fn probe_if_inconclusive(
    q: &mut impl TerminalQuerier,
    stack: &[Mux],
    terminal: &Terminal,
    timeout: Duration,
) -> GraphicsProbe {
    let settled = TerminalInfo::unprobed(vec![], terminal.clone());
    if stack.is_empty() && settled.supports_kitty_graphics() {
        return GraphicsProbe::Skipped;
    }
    probe_graphics(q, stack, timeout)
}

/// Identify what's at the current layer: send XTVERSION, fall back to DA2.
/// Returns (terminal, optional mux detected at this layer).
fn identify_layer(
    q: &mut impl TerminalQuerier,
    query_bytes: &[u8],
    da2_bytes: &[u8],
    timeout: Duration,
) -> Option<(Terminal, Option<Mux>)> {
    // Try XTVERSION
    if let Some(resp) = q.query(query_bytes, timeout) {
        if let Some(result) = parse_xtversion(&resp) {
            return Some(result);
        }
    }
    // Try DA2
    if let Some(resp) = q.query(da2_bytes, timeout) {
        if let Some(info) = parse_da2(&resp) {
            return Some(terminal_from_da2(&info));
        }
    }
    None
}

/// Run the full in-band detection algorithm using a querier.
/// Recursively probes through mux layers to find the outer terminal.
fn detect_inband(q: &mut impl TerminalQuerier) -> TerminalInfo {
    let timeout = Duration::from_millis(1500);
    let passthrough_timeout = Duration::from_millis(2000);

    // Identify the immediate layer (no passthrough wrapping)
    let Some((terminal, mux)) = identify_layer(q, XTVERSION, DA2, timeout) else {
        return TerminalInfo {
            mux_stack: vec![],
            terminal: Terminal::Unknown,
            graphics: probe_graphics(q, &[], timeout),
        };
    };

    let Some(first_mux) = mux else {
        // Direct terminal, no mux
        let graphics = probe_if_inconclusive(q, &[], &terminal, timeout);
        return TerminalInfo {
            mux_stack: vec![],
            terminal,
            graphics,
        };
    };

    // We're inside at least one mux. Probe outward recursively.
    let mut stack = vec![first_mux];
    if !ends_at_zellij(&stack) {
        probe_outer(q, &mut stack, passthrough_timeout);
    }

    // Zellij answers escape-sequence queries itself instead of forwarding
    // them, so there is nothing outside it to discover: it is the terminal we
    // are drawing into, whatever it is running in.
    let terminal = if ends_at_zellij(&stack) {
        Terminal::Unknown
    } else {
        // Query through the full discovered stack to identify the outer terminal.
        let xt = wrap_for_stack(XTVERSION, &stack);
        let da = wrap_for_stack(DA2, &stack);
        if let Some((term, mux)) = identify_layer(q, &xt, &da, passthrough_timeout) {
            if let Some(outer_mux) = mux {
                // Even more nesting beyond what probe_outer found.
                if stack.len() < MAX_MUX_DEPTH {
                    stack.push(outer_mux);
                }
                Terminal::Unknown
            } else {
                term
            }
        } else {
            Terminal::Unknown
        }
    };

    let graphics = probe_if_inconclusive(q, &stack, &terminal, passthrough_timeout);
    TerminalInfo {
        mux_stack: stack,
        terminal,
        graphics,
    }
}

/// Whether the outermost layer found so far is Zellij, which is as far out as
/// escape-sequence queries reach.
fn ends_at_zellij(stack: &[Mux]) -> bool {
    matches!(stack.last(), Some(Mux::Zellij(_)))
}

/// Recursively probe through mux layers to discover nesting.
/// `stack` already contains the first detected mux. We wrap queries through
/// the current stack and check if the response reveals another mux layer.
fn probe_outer(q: &mut impl TerminalQuerier, stack: &mut Vec<Mux>, timeout: Duration) {
    while stack.len() < MAX_MUX_DEPTH {
        let xt = wrap_for_stack(XTVERSION, stack);
        let da = wrap_for_stack(DA2, stack);

        let Some((_, mux)) = identify_layer(q, &xt, &da, timeout) else {
            break;
        };

        match mux {
            Some(next_mux) => {
                stack.push(next_mux);
                if ends_at_zellij(stack) {
                    break; // Zellij answers for itself — nothing beyond it
                }
            }
            None => break, // Found a terminal, not a mux — done
        }
    }
}

// ─── Env var fallback ───────────────────────────────────────

/// Detect terminal and mux from environment variables alone.
pub fn detect_from_env() -> TerminalInfo {
    TerminalInfo::unprobed(detect_mux_env(), detect_terminal_env())
}

/// The Zellij layer, if the environment says we are inside one.
///
/// For the paths that skip detection: Zellij needs no passthrough wrapping,
/// but it does reject Unicode placeholders, so knowing it is there still
/// decides how the image is anchored.
pub fn zellij_from_env() -> Vec<Mux> {
    if std::env::var_os("ZELLIJ").is_some() {
        vec![Mux::Zellij(None)]
    } else {
        vec![]
    }
}

fn detect_mux_env() -> Vec<Mux> {
    // Env vars can only detect one layer (the innermost mux sets the var).
    if std::env::var_os("TMUX").is_some() {
        return vec![Mux::Tmux(None)];
    }
    if std::env::var_os("STY").is_some() {
        return vec![Mux::Screen(None)];
    }
    if std::env::var_os("ZELLIJ").is_some() {
        return vec![Mux::Zellij(None)];
    }
    vec![]
}

fn detect_terminal_env() -> Terminal {
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return Terminal::Kitty(None);
    }
    if std::env::var_os("KONSOLE_VERSION").is_some() {
        return Terminal::Konsole(None);
    }
    if let Some(prog) = std::env::var_os("TERM_PROGRAM") {
        let prog = prog.to_string_lossy();
        match prog.as_ref() {
            "kitty" => return Terminal::Kitty(None),
            "WezTerm" => return Terminal::WezTerm(None),
            "ghostty" | "Ghostty" => return Terminal::Ghostty(None),
            "iTerm.app" | "iTerm2" => return Terminal::ITerm2,
            _ => {}
        }
    }
    if let Some(term) = std::env::var_os("TERM") {
        let term = term.to_string_lossy().to_ascii_lowercase();
        if term.contains("kitty") {
            return Terminal::Kitty(None);
        }
        if term.contains("ghostty") {
            return Terminal::Ghostty(None);
        }
        if term.contains("wezterm") {
            return Terminal::WezTerm(None);
        }
    }
    Terminal::Unknown
}

// ─── Platform I/O: Unix ─────────────────────────────────────

#[cfg(unix)]
pub(crate) mod tty {
    use super::*;
    use std::os::unix::io::{AsRawFd, RawFd};

    /// RAII guard that restores termios settings on drop.
    struct RawModeGuard {
        fd: RawFd,
        original: libc::termios,
    }

    impl Drop for RawModeGuard {
        fn drop(&mut self) {
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
            }
        }
    }

    /// A session for querying the terminal via escape sequences.
    pub struct QuerySession {
        read_fd: RawFd,
        write_fd: RawFd,
        _tty_file: Option<std::fs::File>,
        _guard: RawModeGuard,
    }

    impl QuerySession {
        /// Open a query session.
        ///
        /// Prefers stdin/stdout when stdin is a terminal. Falls back to
        /// `/dev/tty` when stdin is redirected (e.g. piped image data).
        pub fn open() -> io::Result<Self> {
            if io::stdin().is_terminal() {
                Self::from_stdin()
            } else {
                Self::from_dev_tty()
            }
        }

        fn from_stdin() -> io::Result<Self> {
            let read_fd = io::stdin().as_raw_fd();
            let write_fd = io::stdout().as_raw_fd();
            let guard = enter_raw_mode(read_fd)?;
            Ok(Self {
                read_fd,
                write_fd,
                _tty_file: None,
                _guard: guard,
            })
        }

        fn from_dev_tty() -> io::Result<Self> {
            let tty = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")?;
            let fd = tty.as_raw_fd();
            let guard = enter_raw_mode(fd)?;
            Ok(Self {
                read_fd: fd,
                write_fd: fd,
                _tty_file: Some(tty),
                _guard: guard,
            })
        }

        /// Drain any pending bytes from the read fd.
        fn discard_pending(&self) {
            let mut byte = [0u8];
            loop {
                if !poll_fd(self.read_fd, Duration::ZERO).unwrap_or(false) {
                    break;
                }
                let ret = unsafe { libc::read(self.read_fd, byte.as_mut_ptr().cast(), 1) };
                if ret <= 0 {
                    break;
                }
            }
        }

        /// Read bytes until the response looks complete or the deadline passes.
        fn read_response(&self, timeout: Duration) -> Option<Vec<u8>> {
            let deadline = Instant::now() + timeout;
            let mut buf = Vec::with_capacity(128);

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                if !poll_fd(self.read_fd, remaining).unwrap_or(false) {
                    break;
                }
                let mut byte = [0u8];
                let ret = unsafe { libc::read(self.read_fd, byte.as_mut_ptr().cast(), 1) };
                if ret <= 0 {
                    break;
                }
                buf.push(byte[0]);
                if is_response_complete(&buf) {
                    break;
                }
            }

            if buf.is_empty() { None } else { Some(buf) }
        }
    }

    impl QuerySession {
        /// Send an arbitrary query and read the response, if any.
        ///
        /// Exposed so callers outside detection (e.g. geometry probing) can
        /// reuse one raw-mode session for several queries.
        pub fn ask(&mut self, request: &[u8], timeout: Duration) -> Option<Vec<u8>> {
            <Self as TerminalQuerier>::query(self, request, timeout)
        }
    }

    impl TerminalQuerier for QuerySession {
        fn query(&mut self, request: &[u8], timeout: Duration) -> Option<Vec<u8>> {
            self.discard_pending();
            let ret = unsafe { libc::write(self.write_fd, request.as_ptr().cast(), request.len()) };
            if ret < 0 {
                return None;
            }
            self.read_response(timeout)
        }

        fn read_more(&mut self, timeout: Duration) -> Option<Vec<u8>> {
            self.read_response(timeout)
        }

        fn discard_pending(&mut self) {
            QuerySession::discard_pending(self);
        }
    }

    fn enter_raw_mode(fd: RawFd) -> io::Result<RawModeGuard> {
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(RawModeGuard { fd, original })
    }

    fn poll_fd(fd: RawFd, timeout: Duration) -> io::Result<bool> {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(err);
        }
        Ok(ret > 0 && (pfd.revents & libc::POLLIN) != 0)
    }

    /// Detect using in-band queries, falling back to env vars.
    pub fn detect() -> TerminalInfo {
        match QuerySession::open() {
            Ok(mut session) => {
                let info = detect_inband(&mut session);
                // If in-band detection found mux(es) but not the terminal,
                // try env vars for the terminal identity.
                if info.terminal == Terminal::Unknown {
                    let env = detect_from_env();
                    TerminalInfo {
                        mux_stack: if info.mux_stack.is_empty() {
                            env.mux_stack
                        } else {
                            info.mux_stack
                        },
                        terminal: if env.terminal != Terminal::Unknown {
                            env.terminal
                        } else {
                            info.terminal
                        },
                        graphics: info.graphics,
                    }
                } else if info.mux_stack.is_empty() {
                    // In-band found terminal but no mux — check env for mux
                    let env_mux = detect_mux_env();
                    TerminalInfo {
                        mux_stack: env_mux,
                        terminal: info.terminal,
                        graphics: info.graphics,
                    }
                } else {
                    info
                }
            }
            Err(_) => detect_from_env(),
        }
    }
}

/// Check whether a response buffer contains a complete terminal response.
fn is_response_complete(buf: &[u8]) -> bool {
    let len = buf.len();
    if len < 2 {
        return false;
    }

    // DCS / XTVERSION response ends with ESC \ (ST)
    if buf[len - 2] == 0x1b && buf[len - 1] == b'\\' {
        return true;
    }

    // 8-bit ST
    if buf[len - 1] == 0x9c {
        return true;
    }

    // Device attributes: DA2 (ESC [ > ... c), or the DA1 report (ESC [ ? ... c)
    // that serves as the barrier on the capability query.
    if buf[len - 1] == b'c' && len >= 4 {
        return buf.windows(3).any(|w| w == b"\x1b[>" || w == b"\x1b[?");
    }

    // XTWINOPS report: ESC [ Ps ; Ps ; Ps t
    if buf[len - 1] == b't' && len >= 6 {
        return buf.windows(2).any(|w| w == b"\x1b[");
    }

    false
}

// ─── Public API ─────────────────────────────────────────────

/// Open a session for sending escape-sequence queries to the terminal.
///
/// Returns `None` when no terminal is reachable (or on non-Unix platforms,
/// where in-band querying is not implemented).
#[cfg(unix)]
pub fn query_session() -> Option<tty::QuerySession> {
    tty::QuerySession::open().ok()
}

/// Detect the terminal emulator and any multiplexer layers.
///
/// Uses in-band escape sequence queries when a terminal is available,
/// falling back to environment variables.
pub fn detect() -> TerminalInfo {
    #[cfg(unix)]
    {
        tty::detect()
    }
    #[cfg(not(unix))]
    {
        detect_from_env()
    }
}

// ─── Tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Parser tests ────────────────────────────────────────

    #[test]
    fn xtversion_kitty() {
        let resp = b"\x1bP>|kitty(0.26.5)\x1b\\";
        let (term, mux) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Kitty(Some("0.26.5".into())));
        assert!(mux.is_none());
    }

    #[test]
    fn xtversion_tmux() {
        let resp = b"\x1bP>|tmux 3.3a\x1b\\";
        let (term, mux) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Unknown);
        assert_eq!(mux, Some(Mux::Tmux(Some("3.3a".into()))));
    }

    #[test]
    fn xtversion_ghostty() {
        let resp = b"\x1bP>|Ghostty(1.0.0)\x1b\\";
        let (term, mux) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Ghostty(Some("1.0.0".into())));
        assert!(mux.is_none());
    }

    #[test]
    fn xtversion_wezterm() {
        let resp = b"\x1bP>|WezTerm 20230408-112425-69ae8472\x1b\\";
        let (term, _) = parse_xtversion(resp).unwrap();
        assert_eq!(
            term,
            Terminal::WezTerm(Some("20230408-112425-69ae8472".into()))
        );
    }

    #[test]
    fn xtversion_foot() {
        let resp = b"\x1bP>|foot(1.13.1)\x1b\\";
        let (term, _) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Foot(Some("1.13.1".into())));
    }

    #[test]
    fn xtversion_xterm() {
        let resp = b"\x1bP>|xterm(351)\x1b\\";
        let (term, _) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Xterm(Some("351".into())));
    }

    #[test]
    fn xtversion_mintty() {
        let resp = b"\x1bP>|mintty 3.6.4\x1b\\";
        let (term, _) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Mintty(Some("3.6.4".into())));
    }

    #[test]
    fn xtversion_contour() {
        let resp = b"\x1bP>|contour(0.3.12)\x1b\\";
        let (term, _) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Contour(Some("0.3.12".into())));
    }

    #[test]
    fn xtversion_8bit_st() {
        let resp = b"\x1bP>|foot(1.13.1)\x9c";
        let (term, _) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Foot(Some("1.13.1".into())));
    }

    #[test]
    fn xtversion_unknown_terminal() {
        let resp = b"\x1bP>|SomeFutureTerminal 2.0\x1b\\";
        let (term, mux) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Other("SomeFutureTerminal 2.0".into()));
        assert!(mux.is_none());
    }

    #[test]
    fn xtversion_garbage() {
        assert!(parse_xtversion(b"not a valid response").is_none());
        assert!(parse_xtversion(b"").is_none());
        assert!(parse_xtversion(b"\x1bP>|\x1b\\").is_none());
    }

    #[test]
    fn da2_tmux() {
        let resp = b"\x1b[>84;0;0c";
        let info = parse_da2(resp).unwrap();
        assert_eq!(info.pp, 84);
        let (_, mux) = terminal_from_da2(&info);
        assert!(matches!(mux, Some(Mux::Tmux(_))));
    }

    #[test]
    fn da2_screen() {
        let resp = b"\x1b[>83;40801;0c";
        let info = parse_da2(resp).unwrap();
        let (_, mux) = terminal_from_da2(&info);
        assert!(matches!(mux, Some(Mux::Screen(_))));
    }

    #[test]
    fn da2_kitty() {
        let resp = b"\x1b[>1;4000;29c";
        let info = parse_da2(resp).unwrap();
        let (term, mux) = terminal_from_da2(&info);
        assert!(matches!(term, Terminal::Kitty(_)));
        assert!(mux.is_none());
    }

    #[test]
    fn da2_xterm() {
        let resp = b"\x1b[>41;351;0c";
        let info = parse_da2(resp).unwrap();
        let (term, _) = terminal_from_da2(&info);
        assert!(matches!(term, Terminal::Xterm(_)));
    }

    #[test]
    fn da2_vte() {
        let resp = b"\x1b[>65;6700;1c";
        let info = parse_da2(resp).unwrap();
        let (term, _) = terminal_from_da2(&info);
        assert_eq!(term, Terminal::Vte);
    }

    #[test]
    fn da2_mintty() {
        let resp = b"\x1b[>77;30105;0c";
        let info = parse_da2(resp).unwrap();
        let (term, _) = terminal_from_da2(&info);
        assert!(matches!(term, Terminal::Mintty(_)));
    }

    #[test]
    fn da2_garbage() {
        assert!(parse_da2(b"not a valid response").is_none());
        assert!(parse_da2(b"").is_none());
        assert!(parse_da2(b"\x1b[>c").is_none());
    }

    // ── Response completeness ───────────────────────────────

    #[test]
    fn response_complete_xtversion() {
        assert!(is_response_complete(b"\x1bP>|kitty(0.26.5)\x1b\\"));
    }

    #[test]
    fn response_complete_8bit_st() {
        assert!(is_response_complete(b"\x1bP>|foot(1.0)\x9c"));
    }

    #[test]
    fn response_complete_da2() {
        assert!(is_response_complete(b"\x1b[>1;4000;29c"));
    }

    #[test]
    fn response_incomplete() {
        assert!(!is_response_complete(b"\x1bP>|kit"));
        assert!(!is_response_complete(b"\x1b[>1;40"));
        assert!(!is_response_complete(b""));
        assert!(!is_response_complete(b"\x1b"));
    }

    // ── Wrapping ────────────────────────────────────────────

    #[test]
    fn wrap_for_mux_tmux() {
        let data = b"\x1b[>0q";
        let wrapped = wrap_for_mux(data, &Mux::Tmux(None));
        // ESC P tmux ; ESC ESC [ > 0 q ESC \
        assert_eq!(wrapped, b"\x1bPtmux;\x1b\x1b[>0q\x1b\\");
    }

    #[test]
    fn wrap_for_mux_screen() {
        let data = b"\x1b[>0q";
        let wrapped = wrap_for_mux(data, &Mux::Screen(None));
        assert_eq!(wrapped, b"\x1bP\x1b[>0q");
    }

    #[test]
    fn wrap_for_stack_double_tmux() {
        let data = b"\x1b[>0q";
        let stack = vec![Mux::Tmux(None), Mux::Tmux(None)];
        let wrapped = wrap_for_stack(data, &stack);
        // Inner wrap: ESC doubled once
        // Outer wrap: all ESCs in the inner result doubled again
        // Original ESC -> 2 ESC (inner) -> 4 ESC (outer)
        let inner = wrap_for_mux(data, &Mux::Tmux(None));
        let expected = wrap_for_mux(&inner, &Mux::Tmux(None));
        assert_eq!(wrapped, expected);
    }

    #[test]
    fn wrap_for_stack_tmux_in_screen() {
        let data = b"\x1b[>0q";
        let stack = vec![Mux::Tmux(None), Mux::Screen(None)];
        let wrapped = wrap_for_stack(data, &stack);
        let inner = wrap_for_mux(data, &Mux::Tmux(None));
        let expected = wrap_for_mux(&inner, &Mux::Screen(None));
        assert_eq!(wrapped, expected);
    }

    #[test]
    fn wrap_for_stack_empty() {
        let data = b"\x1b[>0q";
        let wrapped = wrap_for_stack(data, &[]);
        assert_eq!(wrapped, data);
    }

    #[test]
    fn wrap_for_mux_zellij_transparent() {
        let data = b"\x1b[>0q";
        let wrapped = wrap_for_mux(data, &Mux::Zellij(None));
        assert_eq!(wrapped, data);
    }

    // ── Mock querier for detection algorithm ────────────────

    struct MockQuerier {
        responses: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    }

    impl MockQuerier {
        fn new(responses: Vec<(Vec<u8>, Option<Vec<u8>>)>) -> Self {
            Self { responses }
        }
    }

    impl TerminalQuerier for MockQuerier {
        fn query(&mut self, request: &[u8], _timeout: Duration) -> Option<Vec<u8>> {
            for (req, resp) in &self.responses {
                if req == request {
                    return resp.clone();
                }
            }
            None
        }
    }

    #[test]
    fn detect_direct_kitty_via_xtversion() {
        let mut q = MockQuerier::new(vec![(
            XTVERSION.to_vec(),
            Some(b"\x1bP>|kitty(0.35.0)\x1b\\".to_vec()),
        )]);
        let info = detect_inband(&mut q);
        assert!(matches!(info.terminal, Terminal::Kitty(_)));
        assert!(info.mux_stack.is_empty());
    }

    #[test]
    fn detect_tmux_with_kitty_outer() {
        // XTVERSION returns tmux, then XTVERSION-through-tmux returns tmux again
        // (probe_outer loop), then XTVERSION-through-2-tmux returns kitty
        // Actually: XTVERSION returns tmux. Then probe_outer wraps XTVERSION
        // through [tmux] and gets kitty (no more mux). Then detect_inband
        // wraps through full stack and gets kitty.
        let xt_through_tmux = wrap_for_stack(XTVERSION, &[Mux::Tmux(None)]);
        let mut q = MockQuerier::new(vec![
            (XTVERSION.to_vec(), Some(b"\x1bP>|tmux 3.3a\x1b\\".to_vec())),
            // probe_outer: XTVERSION through 1 tmux -> kitty (terminal, not mux)
            (
                xt_through_tmux.clone(),
                Some(b"\x1bP>|kitty(0.35.0)\x1b\\".to_vec()),
            ),
            // detect_inband final query through full stack -> kitty
            (
                xt_through_tmux,
                Some(b"\x1bP>|kitty(0.35.0)\x1b\\".to_vec()),
            ),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.mux_stack.len(), 1);
        assert!(matches!(info.mux_stack[0], Mux::Tmux(_)));
        assert!(matches!(info.terminal, Terminal::Kitty(_)));
    }

    #[test]
    fn detect_nested_tmux_with_kitty_outer() {
        let xt_1 = wrap_for_stack(XTVERSION, &[Mux::Tmux(None)]);
        let xt_2 = wrap_for_stack(XTVERSION, &[Mux::Tmux(None), Mux::Tmux(None)]);
        let mut q = MockQuerier::new(vec![
            // Layer 0: XTVERSION -> tmux
            (XTVERSION.to_vec(), Some(b"\x1bP>|tmux 3.4\x1b\\".to_vec())),
            // probe_outer through 1 tmux -> another tmux
            (xt_1.clone(), Some(b"\x1bP>|tmux 3.3a\x1b\\".to_vec())),
            // probe_outer through 2 tmux -> kitty (terminal)
            (xt_2.clone(), Some(b"\x1bP>|kitty(0.35.0)\x1b\\".to_vec())),
            // detect_inband final query through full stack -> kitty
            (xt_2, Some(b"\x1bP>|kitty(0.35.0)\x1b\\".to_vec())),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.mux_stack.len(), 2);
        assert!(matches!(info.mux_stack[0], Mux::Tmux(_)));
        assert!(matches!(info.mux_stack[1], Mux::Tmux(_)));
        assert!(matches!(info.terminal, Terminal::Kitty(_)));
    }

    #[test]
    fn detect_tmux_in_screen_with_ghostty_outer() {
        let xt_tmux = wrap_for_stack(XTVERSION, &[Mux::Tmux(None)]);
        let da_tmux = wrap_for_stack(DA2, &[Mux::Tmux(None)]);
        let xt_both = wrap_for_stack(XTVERSION, &[Mux::Tmux(None), Mux::Screen(None)]);

        let mut q = MockQuerier::new(vec![
            // XTVERSION -> tmux (innermost)
            (XTVERSION.to_vec(), Some(b"\x1bP>|tmux 3.4\x1b\\".to_vec())),
            // Through tmux: XTVERSION times out, DA2 -> screen
            (xt_tmux.clone(), None),
            (da_tmux.clone(), Some(b"\x1b[>83;40801;0c".to_vec())),
            // Through tmux+screen: XTVERSION -> Ghostty
            (
                xt_both.clone(),
                Some(b"\x1bP>|Ghostty(1.0.0)\x1b\\".to_vec()),
            ),
            // Final query through stack -> Ghostty
            (xt_both, Some(b"\x1bP>|Ghostty(1.0.0)\x1b\\".to_vec())),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.mux_stack.len(), 2);
        assert!(matches!(info.mux_stack[0], Mux::Tmux(_)));
        assert!(matches!(info.mux_stack[1], Mux::Screen(_)));
        assert!(matches!(info.terminal, Terminal::Ghostty(_)));
    }

    #[test]
    fn detect_screen_via_da2() {
        let xt_screen = wrap_for_stack(XTVERSION, &[Mux::Screen(None)]);

        let mut q = MockQuerier::new(vec![
            (XTVERSION.to_vec(), None),
            (DA2.to_vec(), Some(b"\x1b[>83;40801;0c".to_vec())),
            // Through screen -> kitty
            (
                xt_screen.clone(),
                Some(b"\x1bP>|kitty(0.30.0)\x1b\\".to_vec()),
            ),
            // Final query
            (xt_screen, Some(b"\x1bP>|kitty(0.30.0)\x1b\\".to_vec())),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.mux_stack.len(), 1);
        assert!(matches!(info.mux_stack[0], Mux::Screen(_)));
        assert!(matches!(info.terminal, Terminal::Kitty(_)));
    }

    #[test]
    fn detect_direct_kitty_via_da2() {
        let mut q = MockQuerier::new(vec![
            (XTVERSION.to_vec(), None),
            (DA2.to_vec(), Some(b"\x1b[>1;4000;29c".to_vec())),
        ]);
        let info = detect_inband(&mut q);
        assert!(matches!(info.terminal, Terminal::Kitty(_)));
        assert!(info.mux_stack.is_empty());
    }

    #[test]
    fn detect_both_timeout_returns_unknown() {
        let mut q = MockQuerier::new(vec![(XTVERSION.to_vec(), None), (DA2.to_vec(), None)]);
        let info = detect_inband(&mut q);
        assert_eq!(info.terminal, Terminal::Unknown);
        assert!(info.mux_stack.is_empty());
    }

    #[test]
    fn detect_tmux_outer_unknown_falls_through() {
        let xt_tmux = wrap_for_stack(XTVERSION, &[Mux::Tmux(None)]);
        let da_tmux = wrap_for_stack(DA2, &[Mux::Tmux(None)]);
        let mut q = MockQuerier::new(vec![
            (XTVERSION.to_vec(), Some(b"\x1bP>|tmux 3.4\x1b\\".to_vec())),
            // probe_outer and final query all time out
            (xt_tmux, None),
            (da_tmux, None),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.mux_stack.len(), 1);
        assert!(matches!(info.mux_stack[0], Mux::Tmux(_)));
        assert_eq!(info.terminal, Terminal::Unknown);
    }

    // ── TerminalInfo methods ────────────────────────────────

    #[test]
    fn kitty_supports_graphics() {
        let info = TerminalInfo::unprobed(vec![], Terminal::Kitty(None));
        assert!(info.supports_kitty_graphics());
    }

    #[test]
    fn unknown_does_not_support_graphics() {
        let info = TerminalInfo::unprobed(vec![], Terminal::Unknown);
        assert!(!info.supports_kitty_graphics());
    }

    #[test]
    fn tmux_needs_passthrough() {
        let info = TerminalInfo::unprobed(vec![Mux::Tmux(None)], Terminal::Kitty(None));
        assert!(info.needs_passthrough());
    }

    #[test]
    fn nested_tmux_needs_passthrough() {
        let info = TerminalInfo::unprobed(
            vec![Mux::Tmux(None), Mux::Tmux(None)],
            Terminal::Kitty(None),
        );
        assert!(info.needs_passthrough());
    }

    #[test]
    fn zellij_does_not_need_passthrough() {
        let info = TerminalInfo::unprobed(vec![Mux::Zellij(None)], Terminal::Kitty(None));
        assert!(!info.needs_passthrough());
    }

    #[test]
    fn no_mux_does_not_need_passthrough() {
        let info = TerminalInfo::unprobed(vec![], Terminal::Kitty(None));
        assert!(!info.needs_passthrough());
    }

    // ── Zellij identity ─────────────────────────────────────

    #[test]
    fn xtversion_zellij() {
        // Zellij packs its version into an integer: 0.45.1 -> 4501.
        let resp = b"\x1bP>|Zellij(4501)\x1b\\";
        let (term, mux) = parse_xtversion(resp).unwrap();
        assert_eq!(term, Terminal::Unknown);
        assert_eq!(mux, Some(Mux::Zellij(Some("0.45.1".into()))));
    }

    #[test]
    fn zellij_version_decoding() {
        assert_eq!(decode_zellij_version("4501").as_deref(), Some("0.45.1"));
        assert_eq!(decode_zellij_version("4500").as_deref(), Some("0.45.0"));
        assert_eq!(decode_zellij_version("4400").as_deref(), Some("0.44.0"));
        assert_eq!(decode_zellij_version("10203").as_deref(), Some("1.2.3"));
        // A dotted version, should a later release report one, is left alone.
        assert_eq!(decode_zellij_version("0.46.0").as_deref(), Some("0.46.0"));
        assert_eq!(decode_zellij_version(""), None);
        assert_eq!(decode_zellij_version("not-a-version"), None);
    }

    #[test]
    fn zellij_graphics_version_threshold() {
        assert!(!zellij_implements_graphics(Some("0.44.0")));
        assert!(!zellij_implements_graphics(Some("0.42.2")));
        assert!(zellij_implements_graphics(Some("0.45.0")));
        assert!(zellij_implements_graphics(Some("0.45.1")));
        assert!(zellij_implements_graphics(Some("1.0.0")));
        // An unreadable version defers to the capability query.
        assert!(zellij_implements_graphics(None));
    }

    #[test]
    fn zellij_rejects_unicode_placeholders() {
        let info =
            TerminalInfo::unprobed(vec![Mux::Zellij(Some("0.45.1".into()))], Terminal::Unknown);
        assert!(!info.supports_unicode_placeholders());
    }

    #[test]
    fn zellij_needs_a_confirmed_query_not_an_inherited_env_var() {
        // TERM_PROGRAM and friends leak into Zellij panes, so the host
        // terminal's name can survive into detection. It does not mean the
        // Zellij in between will draw anything.
        let named = TerminalInfo::unprobed(
            vec![Mux::Zellij(Some("0.45.1".into()))],
            Terminal::Ghostty(None),
        );
        assert!(!named.supports_kitty_graphics());

        let confirmed = TerminalInfo {
            mux_stack: vec![Mux::Zellij(Some("0.45.1".into()))],
            terminal: Terminal::Unknown,
            graphics: GraphicsProbe::Confirmed,
        };
        assert!(confirmed.supports_kitty_graphics());
    }

    #[test]
    fn refused_query_does_not_override_a_named_terminal() {
        // Under a multiplexer the reply can go missing on a path that
        // nonetheless carries the image, and not every terminal answers.
        let info = TerminalInfo {
            mux_stack: vec![Mux::Tmux(None)],
            terminal: Terminal::Kitty(None),
            graphics: GraphicsProbe::Unanswered,
        };
        assert!(info.supports_kitty_graphics());
    }

    // ── Capability query ────────────────────────────────────

    #[test]
    fn graphics_probe_ok() {
        assert_eq!(
            parse_graphics_probe(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;52c"),
            GraphicsProbe::Confirmed
        );
    }

    #[test]
    fn graphics_probe_enotsupported() {
        let resp = b"\x1b_Gi=31;ENOTSUPPORTED:kitty graphics not supported by host terminal\x1b\\";
        assert_eq!(parse_graphics_probe(resp), GraphicsProbe::Refused);
    }

    #[test]
    fn graphics_probe_barrier_only() {
        assert_eq!(
            parse_graphics_probe(b"\x1b[?62;52c"),
            GraphicsProbe::Unanswered
        );
        assert_eq!(parse_graphics_probe(b""), GraphicsProbe::Unanswered);
        assert_eq!(
            parse_graphics_probe(b"\x1b_Gi=31;OK"),
            GraphicsProbe::Unanswered
        );
    }

    #[test]
    fn response_complete_da1() {
        assert!(is_response_complete(b"\x1b[?62;4;52c"));
        assert!(!is_response_complete(b"\x1b[?62;4"));
    }

    // ── Detection through Zellij ────────────────────────────

    #[test]
    fn detect_zellij_answers_for_itself() {
        let mut q = MockQuerier::new(vec![
            (
                XTVERSION.to_vec(),
                Some(b"\x1bP>|Zellij(4501)\x1b\\".to_vec()),
            ),
            (
                KITTY_GRAPHICS_QUERY.to_vec(),
                Some(b"\x1b_Gi=31;OK\x1b\\".to_vec()),
            ),
        ]);
        let info = detect_inband(&mut q);
        // One layer only: Zellij answers queries itself, so probing outward
        // would just hear from Zellij again.
        assert_eq!(info.mux_stack, vec![Mux::Zellij(Some("0.45.1".into()))]);
        assert_eq!(info.terminal, Terminal::Unknown);
        assert_eq!(info.graphics, GraphicsProbe::Confirmed);
        assert!(info.supports_kitty_graphics());
        assert!(!info.supports_unicode_placeholders());
        assert!(!info.needs_passthrough());
    }

    #[test]
    fn detect_zellij_whose_host_cannot_draw() {
        let mut q =
            MockQuerier::new(vec![
            (XTVERSION.to_vec(), Some(b"\x1bP>|Zellij(4501)\x1b\\".to_vec())),
            (
                KITTY_GRAPHICS_QUERY.to_vec(),
                Some(
                    b"\x1b_Gi=31;ENOTSUPPORTED:kitty graphics not supported by host terminal\x1b\\"
                        .to_vec(),
                ),
            ),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.graphics, GraphicsProbe::Refused);
        assert!(!info.supports_kitty_graphics());
    }

    #[test]
    fn detect_zellij_too_old_for_graphics() {
        let mut q = MockQuerier::new(vec![(
            XTVERSION.to_vec(),
            Some(b"\x1bP>|Zellij(4400)\x1b\\".to_vec()),
        )]);
        let info = detect_inband(&mut q);
        assert_eq!(info.zellij_version(), Some("0.44.0"));
        assert!(!zellij_implements_graphics(info.zellij_version()));
        assert_eq!(info.graphics, GraphicsProbe::Unanswered);
        assert!(!info.supports_kitty_graphics());
    }

    #[test]
    fn detect_zellij_inside_tmux() {
        let xt_tmux = wrap_for_stack(XTVERSION, &[Mux::Tmux(None)]);
        let probe = wrap_for_stack(
            KITTY_GRAPHICS_QUERY,
            &[Mux::Tmux(None), Mux::Zellij(Some("0.45.1".into()))],
        );
        let mut q = MockQuerier::new(vec![
            (XTVERSION.to_vec(), Some(b"\x1bP>|tmux 3.4\x1b\\".to_vec())),
            (xt_tmux, Some(b"\x1bP>|Zellij(4501)\x1b\\".to_vec())),
            (probe, Some(b"\x1b_Gi=31;OK\x1b\\".to_vec())),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.mux_stack.len(), 2);
        assert!(matches!(info.mux_stack[0], Mux::Tmux(_)));
        assert!(matches!(info.mux_stack[1], Mux::Zellij(_)));
        assert_eq!(info.terminal, Terminal::Unknown);
        assert_eq!(info.graphics, GraphicsProbe::Confirmed);
    }

    // ── When the query is asked at all ──────────────────────

    #[test]
    fn named_terminal_is_not_asked_to_confirm_itself() {
        let mut q = MockQuerier::new(vec![(
            XTVERSION.to_vec(),
            Some(b"\x1bP>|kitty(0.35.0)\x1b\\".to_vec()),
        )]);
        let info = detect_inband(&mut q);
        assert_eq!(info.graphics, GraphicsProbe::Skipped);
        assert!(info.supports_kitty_graphics());
    }

    #[test]
    fn unnamed_terminal_can_still_confirm_itself() {
        let mut q = MockQuerier::new(vec![
            (
                XTVERSION.to_vec(),
                Some(b"\x1bP>|SomeFutureTerminal 2.0\x1b\\".to_vec()),
            ),
            (
                KITTY_GRAPHICS_QUERY.to_vec(),
                Some(b"\x1b_Gi=31;OK\x1b\\".to_vec()),
            ),
        ]);
        let info = detect_inband(&mut q);
        assert!(matches!(info.terminal, Terminal::Other(_)));
        assert_eq!(info.graphics, GraphicsProbe::Confirmed);
        assert!(info.supports_kitty_graphics());
    }

    #[test]
    fn silent_terminal_can_still_confirm_itself() {
        let mut q = MockQuerier::new(vec![
            (XTVERSION.to_vec(), None),
            (DA2.to_vec(), None),
            (
                KITTY_GRAPHICS_QUERY.to_vec(),
                Some(b"\x1b_Gi=31;OK\x1b\\".to_vec()),
            ),
        ]);
        let info = detect_inband(&mut q);
        assert_eq!(info.terminal, Terminal::Unknown);
        assert!(info.supports_kitty_graphics());
    }

    #[test]
    fn late_reply_after_a_locally_answered_barrier() {
        // screen answers the DA barrier itself, ahead of the reply it
        // forwarded, which ends the read before the answer arrives.
        struct LateReplyQuerier;
        impl TerminalQuerier for LateReplyQuerier {
            fn query(&mut self, request: &[u8], _timeout: Duration) -> Option<Vec<u8>> {
                if request == XTVERSION {
                    return Some(b"\x1bP>|Zellij(4501)\x1b\\".to_vec());
                }
                Some(b"\x1b[?1;2c".to_vec())
            }
            fn read_more(&mut self, _timeout: Duration) -> Option<Vec<u8>> {
                Some(b"\x1b_Gi=31;OK\x1b\\".to_vec())
            }
        }
        let mut q = LateReplyQuerier;
        let info = detect_inband(&mut q);
        assert_eq!(info.graphics, GraphicsProbe::Confirmed);
    }
}
