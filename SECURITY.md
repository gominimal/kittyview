# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in kittyview, please report it through [GitHub's private vulnerability reporting](../../security/advisories/new).

Do **not** open a public issue for security vulnerabilities.

## Scope

Security-relevant areas of kittyview include:

- **SVG rendering**: SVGs are a rich format that can reference external files, embed scripts, and contain deeply nested structures. kittyview uses [resvg](https://github.com/linebender/resvg) (pure Rust, no scripting support) and defaults to blocking external file access (`--svg-resources none`).
- **SVG foreignObject conversion**: SVGs containing `<foreignObject>` elements (common in mermaid-cli, draw.io, and D3.js output) are preprocessed before rendering. Embedded HTML is stripped to plain text and replaced with native SVG `<text>` elements. No HTML is interpreted or executed -- all markup is discarded and only text content is preserved. Entity decoding is limited to a fixed set of named entities and numeric character references.
- **Image decoding**: Malformed images could trigger bugs in decoder libraries. All decoders are pure Rust (no C code).
- **Terminal escape sequences**: Malformed output could corrupt terminal state. kittyview buffers all protocol output before writing and validates terminal support before emitting. kittyview also *reads* from the terminal -- capability query replies, geometry reports, the slideshow's transmission checks, and keyboard input in raw mode. Everything read is treated as untrusted: reads are time-bounded, and replies and keys go through pure-Rust slice parsers that discard what they do not recognise (the key decoder additionally caps how many bytes one sequence may consume).
- **Unsafe code**: every `unsafe` block in kittyview is terminal-control FFI to libc -- termios raw mode, `select(2)`, `TIOCGWINSZ`, reads and writes on terminal file descriptors, and the slideshow's signal handling (`sigaction`, `kill`) -- or zero-initialisation of the C structs those calls fill. Signal handlers do nothing but store an atomic flag, keeping them async-signal-safe. No `unsafe` code touches untrusted data: images, SVGs, and terminal replies are parsed entirely in safe Rust, with the input-file parsers fuzzed (see `fuzz/`).

## Verifying release artifacts

Release binaries are built and attested with [SLSA build provenance](https://slsa.dev/provenance/)
by the release workflow. To verify that an asset came from the release it
claims to -- see [Verifying downloads](README.md#verifying-downloads) for why
`--source-ref` is what gives a passing check that meaning:

```sh
gh attestation verify kittyview-linux-amd64.tar.gz \
  --repo gominimal/kittyview \
  --source-ref refs/tags/v0.1.5 \
  --deny-self-hosted-runners
```

Substitute the tag of the release you downloaded.

Each release also includes the provenance bundle itself
(`kittyview-provenance.intoto.jsonl`) as an asset, so verification does not
have to query GitHub's attestation store. Fully offline verification also
needs a copy of the Sigstore trusted root, saved while still online:

```sh
gh attestation trusted-root > trusted_root.jsonl
```

```sh
gh attestation verify kittyview-linux-amd64.tar.gz \
  --repo gominimal/kittyview \
  --source-ref refs/tags/v0.1.5 \
  --deny-self-hosted-runners \
  --bundle kittyview-provenance.intoto.jsonl \
  --custom-trusted-root trusted_root.jsonl
```

## Supported versions

| Version | Supported |
|---------|-----------|
| latest  | Yes       |
