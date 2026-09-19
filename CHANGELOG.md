# Changelog

## upcoming

- **Slideshow mode**: giving kittyview more than one file now opens an interactive slideshow on the alternate screen -- Right/Space/n and Left/Backspace/p to navigate, Home/End for the ends, q/Esc/Ctrl-C to quit. Images are fitted to the window and centred, with a status line naming the file and position. On the Unicode-placeholder path, navigation never flashes a blank screen: an `a=T,U=1` transmission is invisible until placeholder cells reference it, so the next image's data streams out while the previous slide still shows, and the visible switch is a small delete-erase-grid burst. Every draw uses the same protocol commands as single-image display -- the transmit-then-place split (`a=t`/`a=p`) is deliberately avoided, having been found rendering blank in the wild where `a=T` works. The first draw orders the alternate-screen switch around its transmission per target: directly (and under Zellij) the switch comes first, because kitty-model terminals keep a separate image store per screen buffer and an image transmitted on the main screen does not exist on the alternate one; through tmux/screen passthrough the transmission comes first, because switching the pane's screen schedules a redraw during which tmux silently drops passthrough -- while the outer terminal, whose store holds the image, never switches screens at all. And because tmux drops passthrough around *any* pending redraw -- while `q=2` means nothing ever reports the loss -- every draw under a multiplexer is verified with the protocol's own reply channel (a silent-on-success placement probe chased by a DA barrier) and retried when the data did not arrive; losses are reported in a diagnostics line on exit. Under a multiplexer, slide changes rebuild by way of the pane's main screen (a brief flash), which field testing proved necessary independently of transmission loss: Ghostty through 1.3.1 renders placeholder images delivered as in-place pane updates blank -- with the data confirmed present in the terminal via the reply channel -- while a full repaint renders them correctly. The `r` key forces that rebuild by hand. Placeholder cells are bound to an explicit placement ID through their underline colour, so they resolve against exactly the slideshow's placement -- Ghostty through 1.3.1 otherwise picks among an image's virtual placements in hash order, and any stray placement could capture the cells. Each slide's data is deleted from the terminal's image store when it is replaced, so a long slideshow does not accumulate hundreds of megabytes in the terminal. Restoring the terminal is layered: one cleanup path for quits and errors, a panic hook that restores the screen before the panic message prints, and fatal signals are caught, cleaned up after, and re-raised so the exit status still reports them. Ctrl-Z suspends and resumes properly. Unix only for now: no native Windows terminal displays kitty graphics from a local process, so on Windows the slideshow refuses with an explanation instead.
- **Fuzzing** now also covers the parsers that face terminal-controlled input: the XTVERSION/DA2/XTWINOPS reply parsers, the kitty graphics capability answer, the response-completeness scanner, the slideshow's transmission-check verdict, and its raw-mode key decoder.
- Added `test/slideshow-test.sh` for human-in-the-loop testing of the slideshow: rendering, navigation, suspend/resume, and -- above all -- that the terminal is handed back untouched on every exit path.
- **macOS piped-input fix**: terminal queries now wait for replies with `select(2)` instead of `poll(2)`. On macOS, poll does not support devices -- its own manual page says so -- and a poll on `/dev/tty` returns immediately with POLLNVAL, which read as "no reply ever". With stdin piped (`curl ... | kittyview`), every in-band query silently timed out, detection fell back to environment variables, and the replies the terminal still sent could land as garbage on the shell prompt after exit. select has a working kernel path for `/dev/tty`, and is how fzf, less, and kitty's own tools wait on it.

- **Zellij detection**: Zellij implements the kitty graphics protocol from version 0.45.0, and kittyview now recognises it, so `--force` is no longer needed to display an image in a Zellij pane. Zellij names itself and its version in its XTVERSION reply -- as the packed integer its own versioning uses, so 0.45.1 arrives as `Zellij(4501)` -- and it answers escape-sequence queries itself rather than forwarding them, so detection stops at Zellij instead of probing for a terminal beyond it.
- **Kitty graphics capability query**: where a name does not settle whether images can be drawn -- inside Zellij, under a multiplexer whose passthrough may not deliver, or in front of a terminal kittyview cannot identify -- kittyview now asks with the protocol's own query (`a=q`), along the same path the image will travel. A confirmed answer is authoritative, so terminals kittyview does not recognise by name no longer need `--force`. A negative answer never overrides a terminal that was recognised, since implementing the protocol and answering for it are not the same thing; it produces a warning instead.
- **Direct placement under Zellij**: Zellij rejects placements anchored to Unicode placeholder cells, and kittyview asks for no protocol replies, so the rejection was silent -- the image was replaced on screen by a rectangle of placeholder glyphs. `--placement auto` now resolves to direct placement whenever Zellij is in the stack, including under `--force`, which skips detection but still checks the environment for Zellij.
- Errors inside Zellij now name the actual problem: a version that predates the protocol, a host terminal that cannot display images, or the protocol switched off in the Zellij config.
- **Animation warning**: `--animate` now warns, on stderr, when the animation is headed somewhere it positively will not play -- Konsole and iTerm2, which ignore the kitty animation sequences; Ghostty before 1.4.0, which rejects them; and Zellij, which rejects them whatever terminal it runs in, including under `--force`, whose environment check already spots it. Every animation sequence goes out with replies suppressed (`q=2`), so the symptom was a static first frame with no explanation. There is no capability query for animation, so unrecognised terminals are given the benefit of the doubt, and WezTerm does not warn: it plays transmitted frames, ignoring only the animation-control sequence.
- **Release integrity**: the release workflow now checks that the pushed tag matches `Cargo.toml`'s version before anything is built, and fails the release if it does not. The v0.1.4 release shipped binaries that reported `0.1.3`, because the version was never bumped alongside the tag.
- Build provenance attestations are now produced by `actions/attest`, which GitHub recommends over the `actions/attest-build-provenance` wrapper it has become. Release artifacts remain verifiable; the README now documents the command, including the `--source-ref` pin that ties a check to a specific release rather than to any build from the repository.
- The release workflow can now be rehearsed from a manual run: the full publish path executes, attestation included, and the draft it produces is discarded at the end of the run so nothing publishable is left behind. Only a `v*` tag publishes.
- GitHub Actions pins updated, and two version comments corrected to name the release they actually point at.
- **Fuzzing**: cargo-fuzz targets now cover the two hand-written parsers that face untrusted input -- the SVG `<foreignObject>` preprocessor and the PNG IHDR reader. A weekly workflow runs them, uploads any crash reproducer, and files an issue on failure, since scheduled-run notifications otherwise reach only whoever last edited the cron line. To let the fuzz targets link against the internals, the binary's modules moved into a library crate; the CLI is unchanged, and the library is not a stable API.
- **CodeQL**: Rust static analysis now runs on pushes, pull requests, and a weekly schedule.
- **Provenance on the release page**: each release now carries its Sigstore provenance bundle (`kittyview-provenance.intoto.jsonl`) as an asset, so artifacts can be verified without querying GitHub's attestation store -- including fully offline, with a saved trusted root; SECURITY.md documents both commands. The release workflow also declares its empty top-level token permissions explicitly instead of leaving them implied.

## 0.1.5

- **Unicode placeholder placement**: images are now anchored to a grid of Unicode placeholder cells (kitty graphics `U=1`) instead of being positioned by the terminal. Placeholder cells are ordinary text, so images scroll, clip, and redraw with the surrounding output -- which is what makes them behave correctly inside multiplexers and pagers.
- **`--placement` flag**: choose how images are anchored (`auto`, `unicode`, `direct`). `auto` uses placeholders everywhere except Konsole and iTerm2, which do not implement them.
- **Terminal geometry detection**: the cell rectangle for a placement is sized from the terminal's cell size, resolved from `TIOCGWINSZ`, tmux's `client_cell_width`/`client_cell_height`, or an `XTWINOPS` query through the multiplexer stack, falling back to a conventional 8x16 cell. A wrong cell size only changes the image's size, never its aspect ratio.
- **tmux passthrough warning**: kittyview now warns when tmux's `allow-passthrough` is off, instead of appearing to do nothing.
- **Image ID collision resistance**: virtual placements now draw their kitty image ID at random from the whole space the placeholder cells can carry -- 65,280 IDs through a multiplexer, the full 32-bit range without one -- instead of 255 values derived from the process ID. Transmitting under an ID that is already in use replaces that image and drops the placements drawing it, which blanked earlier images still sitting in scrollback.

## 0.1.4

- **Terminal multiplexer passthrough**: kittyview now auto-detects tmux and GNU screen via in-band terminal queries (XTVERSION, DA2) and automatically wraps kitty graphics sequences in DCS passthrough envelopes. Requires `set -g allow-passthrough on` in tmux.conf.
- **Nested multiplexer support**: detects and wraps through multiple multiplexer layers (e.g. tmux-in-tmux, tmux-in-screen). Each layer is probed recursively up to 4 levels deep.
- **`--passthrough` flag**: manually specify the multiplexer stack (`auto`, `off`, or comma-separated layers like `tmux`, `tmux,tmux`, `tmux,screen`).
- **In-band terminal detection**: terminal identity is now detected via XTVERSION/DA2 escape sequence queries through stdin (or `/dev/tty` when stdin is piped), replacing the previous env-var-only approach. Env vars serve as fallback when no terminal I/O is available.
- **Font fallback for SVG text**: sans-serif, serif, and monospace font family mappings now try a list of common fonts (Liberation, DejaVu, Helvetica, Arial, etc.) instead of hardcoding Liberation fonts. Fixes blank text on systems without Liberation fonts installed (e.g. macOS).
- Fixed `<br/>` (self-closing, no space) not producing line breaks in foreignObject text extraction.
- Added `test/visual-test.sh` for human-in-the-loop visual regression testing of SVG text rendering.

## 0.1.3

- SVG `<foreignObject>` support: text labels in SVGs generated by mermaid-cli, draw.io, and D3.js now render correctly. Embedded HTML is converted to native SVG `<text>` elements before rendering. Structural HTML (tables, lists, nested divs) is preserved as readable text with row/cell separation.
- SVG text inherits fill color and font-family from the document's stylesheet.
- SVG `<foreignObject>` elements inside `<switch>` with an existing `<text>` fallback are left untouched.
- SVG `<foreignObject>` x/y positioning attributes are respected.
- Expanded HTML entity decoding: numeric character references (`&#NNN;`, `&#xHHH;`) and common named entities (`&nbsp;`, `&mdash;`, `&rarr;`, etc.) are now decoded in foreignObject text.

## 0.1.2

- Animated GIF playback via `--animate` flag (kitty animation protocol)
- Animated logo variant with speech bubble (`kittyview --animate logo`)
- Built-in kitten logo now has normal and happy (^_^) expressions

## 0.1.1

- Stdin support: pipe images directly (`cat photo.jpg | kittyview`). Auto-detected when stdin is not a TTY.
- SVG stdin input resolves relative resource paths from the current working directory.

## 0.1.0

Initial release.

- Display images in kitty-compatible terminals (kitty, Ghostty, WezTerm, Konsole, iTerm2)
- Supported raster formats: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, PNM, TGA, QOI, Farbfeld, HDR
- SVG rendering with full text support via resvg (pure Rust)
- Terminal auto-detection via environment variables with `--force` override
- `png` subcommand for format conversion and debugging
- `completions` subcommand for bash, zsh, fish, PowerShell, and elvish
- SVG external resource sandboxing (`--svg-resources none|cwd|tree|any`)
- Oversized SVG downscaling (max 8192x8192) to prevent memory exhaustion
- Crash-safe buffered protocol output
- Built-in kitten logo (`kittyview logo`)
- Cross-platform: Linux (amd64, aarch64), macOS (Intel, Apple Silicon), Windows (amd64, aarch64)
- Pure Rust -- no C dependencies
