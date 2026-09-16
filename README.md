# CDisk

[简体中文](README.zh-CN.md)

CDisk is a local-first macOS disk maintenance tool built with Rust, Tauri,
and React. It provides a complete workflow for scanning, understanding,
confirming, cleaning, and recording disk cleanup operations.

[Privacy](PRIVACY.md) · [Security](SECURITY.md) ·
[Dependency audit](docs/dependency-audit.md) · [MIT License](LICENSE)

## Installation

The current prebuilt release supports Apple Silicon Macs. It is ad-hoc signed
and has not been notarized by Apple. On first launch, right-click CDisk in
Finder, choose **Open**, and confirm that you want to launch it.

Install with the checksum-verifying script:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/myersguo/cdisk/main/scripts/install.sh | sh
```

Or install with Homebrew:

```bash
brew install --cask myersguo/tap/cdisk
```

## Features

- **Seven languages:** English, Simplified Chinese, Traditional Chinese,
  Japanese, Korean, Spanish, and French. CDisk initially follows the macOS
  language and saves an explicit language choice locally.
- **Focused desktop interface:** a compact sidebar, clear selection states,
  and independently scrolling candidate and detail panes.
- **Daily cleanup:** individual cache and log candidates with search,
  categories, path-usage checks, and running-application protection.
- **Project cleanup:** configurable development roots with Git ignored/tracked
  checks, nested repository and sensitive-file protection, and conservative
  handling of artifacts used during the last seven days.
- **Installer review:** common installer packages in Downloads and Desktop,
  while mounted disk images remain protected.
- **Disk analysis:** read-only analysis of a custom directory or the Data
  volume, with drilldown, parent navigation, size sorting, and Finder reveal.
- **Protection list:** persistent path protection without discarding existing
  scan results.
- **Cleanup history:** per-item success, skipped, failure, and interruption
  states, plus disk-space measurements before and after an operation.

## Scanning and cleanup

Daily cleanup, project cleanup, installer review, and disk analysis can run at
the same time. Each page keeps its own progress, results, filters, and
selection, while only one scan runs on a given page.

Pause preserves the current traversal position. Completed and eligible
candidates remain selectable while paused; entering cleanup preview ends the
remaining scan and keeps completed results. Cancellation also wakes a paused
task and preserves completed candidates. Incomplete candidates cannot be
cleaned until they are rechecked. Disk-analysis results are always read-only.
A scan or cancellation never deletes files.

Candidates are initially unselected. Before permanent deletion, CDisk shows
the exact paths, creates a short-lived confirmation plan, and revalidates every
item. Cleanup does not request `sudo` and protects system roots, credentials,
application databases, Codex sessions, and VM runtimes.

Large previews use at most four validation workers and display completed
count, current path, and a cancellation action. Preview never deletes data.
Execution remains serialized and repeats complete directory, Git, mount,
identity, sensitive-file, and usage checks immediately before each deletion.
Items that changed after scanning are skipped.

## Why can't an item be selected?

- **Manual protection:** the detail pane shows the matching rule. Remove that
  rule and recheck the item, or remove it from Settings.
- **Mounted image:** eject the disk image in Finder, then recheck it. This
  protection cannot be bypassed by removing a manual rule.
- **Path in use:** close the listed process and recheck the item.
- **Application running:** app-owned caches and logs remain protected until
  the exact owning application exits.
- **System or data protection:** credentials, Git-tracked content, and other
  sensitive data do not offer a force-unlock action.
- **Unknown or incomplete state:** recheck the item; CDisk does not bypass a
  failed safety check.

Saving all scan settings requires active scans to end, including paused scans.
Existing results, filters, and selections are retained. New protection rules
lock matching candidates immediately; candidates affected by a removed rule
must be rechecked before they become eligible.

Development caches such as Go, npm, pnpm, Bun, uv, pip, and Homebrew use
path-scoped open-file checks instead of a broad runtime-name gate. App-owned
caches and logs for Chrome, Lark, WeChat, Codex, and JetBrains applications
also check whether their exact `.app` owner is running. Probe timeouts or
malformed output produce an unknown state and block cleanup.

CDisk does not currently provide application uninstalling, system
optimization, live monitoring, or Finder Trash recovery.

## Development

```bash
pnpm install
pnpm run tauri dev
```

## Verification

```bash
pnpm run verify
```

Browser regression tests require `pnpm dev` and Playwright:

```bash
pnpm run test:browser
```

The browser UI started by `pnpm dev` is clearly marked as demo mode and does
not perform disk operations. The native application uses the Rust backend.

## Privacy and security

- The packaged application contains no telemetry, analytics, cloud sync,
  automatic updater, or network-upload feature.
- Scan results remain in memory. Settings and the latest 100 cleanup records
  are stored in the current user's Application Support directory.
- Cleanup history contains full local paths and is sensitive local data. See
  the [privacy policy](PRIVACY.md).
- Cleanup accepts only candidate IDs from a backend scan snapshot and
  revalidates path identity, Git state, mounts, sensitive files, and usage
  immediately before permanent deletion.
- Report vulnerabilities through GitHub Private Vulnerability Reporting.
  Do not attach private files or unredacted local paths to public issues. See
  the [security policy](SECURITY.md).

## Packaging

```bash
pnpm run tauri build --debug --bundles app
```

The local debug bundle is written to
`src-tauri/target/debug/bundle/macos/CDisk.app`. Do not repeatedly run
`cargo clean` between verification steps. A trusted public binary still
requires Developer ID signing and Apple notarization.

Before publishing, follow the
[release checklist](docs/release-checklist.md) for source, dependency,
signature, notarization, checksum, and privacy checks. The local `release/`
directory and debug bundles must not be uploaded as trusted release artifacts.

Local settings and history are stored under:

```text
~/Library/Application Support/com.myersguo.cdisk/
```

Unreadable directories are reported as partial results. If needed, grant
CDisk Full Disk Access in macOS System Settings. Unknown permissions are never
treated as zero-byte or cleanup-eligible results.
