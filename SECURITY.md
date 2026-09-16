# Security Policy

## Supported version

Security fixes are applied to the latest release on `main`.
The repository does not yet have a stable compatibility promise; security
fixes may require upgrading to the latest release.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting for this repository. Do
not open a public issue for a vulnerability that could cause unintended file
deletion, path traversal, local data exposure, or arbitrary command execution.

Include the affected version, macOS version, reproduction steps, and whether
any filesystem content changed. Do not attach private user files, credentials,
or full local paths unless they are required and have been redacted.

## Security boundary

CDisk is a local desktop application. It does not provide a network service,
telemetry, cloud sync, or an update client. It runs without `sudo` and stores
settings and cleanup history under the current user's Application Support
directory.

Cleanup is intentionally fail-closed:

- the frontend submits only backend-generated candidate IDs;
- system roots, user homes, credentials, Git metadata, app support data,
  symlinks, mount points, and incomplete scans are protected;
- preview runs bounded current-state qualification, never deletes data, and
  produces a short-lived single-use plan;
- every item is revalidated immediately before deletion;
- deletion uses verified parent descriptors, no-follow operations, identity
  checks, exclusive same-parent isolation, and mount-boundary checks.

This does not claim protection from a malicious process running as the same
macOS user and continuously racing filesystem state. Review `README.md` and
`docs/mole-benchmark.md` for current limits.

## Distribution

Repository builds are development artifacts unless a release is signed with a
Developer ID certificate and notarized by Apple. Ad-hoc signing is suitable
for local testing only.

The current `release/` directory is ignored from source control and contains
local test builds. Do not redistribute those bundles as trusted releases.

## Dependency audit notes

`pnpm audit` and RustSec `cargo audit` are part of the release review. The
current macOS build has no known vulnerability advisories. RustSec reports
unmaintained `unic-*` crates through Tauri's `urlpattern` dependency. It also
reports `glib`/`proc-macro-error` advisories in Tauri's Linux GTK dependency
graph; those crates are not present in the `aarch64-apple-darwin` graph or the
macOS application binary. See [dependency audit notes](docs/dependency-audit.md)
for exact advisory IDs and rationale. Dependabot monitors npm, Cargo, and
GitHub Actions updates.
