# Dependency audit notes

Review date: 2026-09-16

## JavaScript

`pnpm audit --audit-level=low` reported no known vulnerabilities. Production
dependencies use MIT or Apache-2.0/MIT licenses. Playwright is a pinned
development-only dependency used by browser regression tests.

## Rust

RustSec `cargo audit` reported no vulnerability advisories. It reported seven
warnings:

- `RUSTSEC-2024-0370` (`proc-macro-error`) and `RUSTSEC-2024-0429` (`glib`) are
  pulled through Tauri's Linux GTK dependency graph and are absent from
  `aarch64-apple-darwin`. They are not ignored in CI, so they become blockers
  if they enter the macOS graph.
- `RUSTSEC-2025-0075`, `RUSTSEC-2025-0080`, `RUSTSEC-2025-0081`,
  `RUSTSEC-2025-0098`, and `RUSTSEC-2025-0100` are unmaintained `unic-*`
  transitive crates pulled by Tauri's `urlpattern` dependency. They are
  present in the macOS dependency graph but are maintenance warnings, not
  vulnerability advisories.

`deny.toml` limits the audited graph to the two macOS targets and keeps only
the five `unic-*` warning IDs as explicit, reasoned exceptions. New advisories,
unknown registries, unknown Git dependencies, or unapproved licenses fail CI.
Remove an exception when the upstream Tauri dependency graph no longer
contains it.

The Rust dependency metadata contains no missing or unreviewed license
expressions. The only LGPL-containing expressions are multi-license
`r-efi` packages that also offer MIT/Apache-2.0.
