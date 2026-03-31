# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in kittyview, please report it through [GitHub's private vulnerability reporting](../../security/advisories/new).

Do **not** open a public issue for security vulnerabilities.

## Scope

Security-relevant areas of kittyview include:

- **SVG rendering**: SVGs are a rich format that can reference external files, embed scripts, and contain deeply nested structures. kittyview uses [resvg](https://github.com/linebender/resvg) (pure Rust, no scripting support) and defaults to blocking external file access (`--svg-resources none`).
- **SVG foreignObject conversion**: SVGs containing `<foreignObject>` elements (common in mermaid-cli, draw.io, and D3.js output) are preprocessed before rendering. Embedded HTML is stripped to plain text and replaced with native SVG `<text>` elements. No HTML is interpreted or executed -- all markup is discarded and only text content is preserved. Entity decoding is limited to a fixed set of named entities and numeric character references.
- **Image decoding**: Malformed images could trigger bugs in decoder libraries. All decoders are pure Rust (no C code).
- **Terminal escape sequences**: Malformed output could corrupt terminal state. kittyview buffers all protocol output before writing and validates terminal support before emitting.

## Supported versions

| Version | Supported |
|---------|-----------|
| latest  | Yes       |
