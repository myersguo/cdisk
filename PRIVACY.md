# Privacy

CDisk is local-only. The application code does not send telemetry, analytics,
scan results, filenames, or cleanup history over the network.

## Data read

Depending on the selected scan, CDisk reads filesystem metadata, directory
names, file allocation sizes, modification times, Git status for configured
project roots, the process table, the mount table, and open-file information.
CDisk itself does not read or index ordinary file bodies. Git subprocesses may
read repository metadata and index state to determine whether generated paths
are tracked or ignored. No file content is uploaded.

macOS may require Full Disk Access to inspect protected locations. Granting
that permission expands what CDisk can read, but does not enable network
upload.

## Data stored

CDisk stores these local JSON files under:

```text
~/Library/Application Support/com.myersguo.cdisk/
```

- `settings.json`: configured project roots and protected paths.
- `history.json`: up to 100 cleanup records, including item titles, full local
  paths, statuses, messages, timestamps, and disk-space measurements.

These files are created with a private application directory (`0700`) and
private files (`0600`). Scan results that have not been cleaned remain in
memory and are not persisted.

The selected UI language is stored in the WebView's local storage as
`cdisk.locale`.

## Data deletion

Quit CDisk and remove its Application Support directory to delete settings and
history. This does not restore files that CDisk previously removed.

## Network behavior

The packaged application has no HTTP client dependency or remote endpoint.
Development uses a localhost Vite server. Links in documentation are not
opened or contacted by the packaged application.
