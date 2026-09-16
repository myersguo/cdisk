# Release checklist

## Source release

- Confirm `git status --short` contains only intended source and documentation.
- Confirm generated output, `release/`, local app data, `.env*`, signing keys,
  certificates, and provisioning profiles are ignored.
- Run `scripts/check-release-tree.sh` over the complete staged tree and enable
  GitHub secret scanning with push protection.
- Run `pnpm install --frozen-lockfile --ignore-scripts`, `pnpm run verify`,
  browser regressions, `pnpm audit --audit-level=low`, and RustSec
  `cargo audit`.
- Review RustSec warnings as well as vulnerabilities. Record target-specific
  transitive warnings instead of silently suppressing them.
- Confirm `LICENSE`, `PRIVACY.md`, and `SECURITY.md` match the release.
- Enable GitHub Private Vulnerability Reporting, Dependabot alerts, and branch
  protection for `main`.
- Review `docs/dependency-audit.md` and remove resolved advisory exceptions
  from `deny.toml`.
- Review the first commit before pushing: this repository has no prior commit,
  so there is no historical diff or GitHub branch protection yet.

## Binary release

- Do not publish `release/CDisk.app` or the debug bundle. Those are ignored
  local development artifacts and can contain local build paths.
- Build a release bundle from a clean tagged commit.
- Preferred stable release: sign with a macOS Developer ID Application
  certificate, submit to Apple for notarization, staple the ticket, and verify
  with both `codesign --strict` and `spctl --assess`.
- If an explicitly named `-unsigned` preview is published before signing is
  available, build it in release mode from the tagged commit, ad-hoc sign and
  verify it, disclose the Gatekeeper limitation in the release notes and
  README, and never describe it as notarized.
- Generate SHA-256 checksums for every uploaded artifact.
- Test scan, pause/resume/cancel, preview, Finder reveal, settings/history, and
  a non-destructive cleanup fixture on a clean macOS user account.
- Do not include local `settings.json`, `history.json`, scan output, logs, or
  screenshots containing full paths in release artifacts or issue reports.
