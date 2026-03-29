# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in kittyview, please report it through [GitHub's private vulnerability reporting](../../security/advisories/new).

Do **not** open a public issue for security vulnerabilities.

## Scope

Security-relevant areas of kittyview include:

- **SVG rendering**: SVGs are a rich format that can reference external files, embed scripts, and contain deeply nested structures. kittyview uses [resvg](https://github.com/linebender/resvg) (pure Rust, no scripting support) and defaults to blocking external file access (`--svg-resources none`).
- **Image decoding**: Malformed images could trigger bugs in decoder libraries. All decoders are pure Rust (no C code).
- **Terminal escape sequences**: Malformed output could corrupt terminal state. kittyview buffers all protocol output before writing and validates terminal support before emitting.

## Supported versions

| Version | Supported |
|---------|-----------|
| latest  | Yes       |
