# heaptrail CLI Improvements Design

## Context

`HEAPTRAIL_FEATURE_GUIDE.md` captures a real Android heap investigation with
heaptrail 1.1.1. The run showed three high-value CLI gaps:

- JSON output is useful but timestamped sidecars are awkward for CI and agents.
- `--bitmaps` reports a misleading unavailable-feature message when
  `android.graphics.Bitmap` is not loaded in an otherwise valid Android dump.
- Primitive-array previews identify payloads, but users still classify JSON,
  XML, image signatures, compressed data, and opaque binary buffers by eye.
- Android allocation-tracking capture is fragile enough that users need a
  helper that validates process state and dump artifacts before analysis.

The approved sequencing is one roadmap with three independently shippable
slices:

1. `--json-out` and diagnostic cleanup.
2. Preview content classifiers.
3. Android capture helper.

## Goals

- Make heaptrail easier to automate in CI, scripts, and agent workflows.
- Make reports more self-explanatory without requiring GUI heap tools.
- Reduce misleading errors and failed Android capture attempts.
- Keep each slice small enough to merge and validate independently.

## Non-Goals

- Do not change default human output unless a new flag is provided or an error
  message is being corrected.
- Do not change heap parsing semantics for ordinary summary, referrer, paths,
  diff, retained-size, leak-suspect, allocation-site, or bitmap modes.
- Do not add release version bumps or release changelog sections as part of
  normal feature work.
- Do not require ADB for non-Android heap analysis.

## Slice 1: JSON Output Path and Diagnostics

Add `--json-out <path>` as an optional companion to `--json`.

Behavior:

- `--json` keeps its current behavior when `--json-out` is absent.
- `--json --json-out reports/leaks.json` writes the selected mode's JSON to
  exactly that path.
- `--json-out` without `--json` is rejected with a clear argument error.
- Parent directories are not created implicitly. If the parent does not exist,
  standard I/O failure is reported.
- All current JSON-producing modes support the flag: summary, referrers,
  paths, merge paths, diff, allocation sites, leak suspects, and bitmaps.
- Text output still prints to stdout.
- The success line should name the explicit output path.

Diagnostic cleanup:

- Replace the current `--bitmaps` error wording with:

  ```text
  android.graphics.Bitmap class is not loaded in this dump; bitmap accounting has nothing to report. This can happen on Android dumps from screens that have not used Bitmap-backed images.
  ```

- Prefer a domain-specific unavailable-feature error over `NotYetImplemented`
  for this condition if it keeps call sites clearer.
- Keep the command nonzero for now. Exit-code taxonomy is a separate future
  slice.

Files likely touched:

- `src/args.rs` for `--json-out`, mode propagation, and parser tests.
- `src/main.rs` for shared JSON writing.
- `src/rendered_result.rs` for summary JSON path handling.
- `src/bitmaps.rs` and possibly `src/errors.rs` for the corrected diagnostic.
- `README.md` and `USERGUIDE.md` for CLI examples.

## Slice 2: Preview Content Classifiers

Add lightweight content classification to primitive-array previews.

Behavior:

- Classifiers run only when `--preview-bytes N` captures preview bytes.
- Existing preview text and hex output remain, but include a concise label such
  as `content: JSON`, `content: XML`, `content: PNG image`, or
  `content: binary/repeated-fill`.
- JSON output includes the classification alongside preview data where preview
  data is already serialized.
- Classifiers are heuristic and should avoid overclaiming. Unknown binary data
  should remain `binary` or `unknown binary`, not a guessed application format.

Initial classifier set:

- JSON object or array: trimmed text starts with `{` or `[` and has a plausible
  JSON prefix.
- XML: trimmed text starts with `<?xml` or `<` followed by an XML-like name.
- UTF-8 text and UTF-16 text: use the existing decoder path.
- Image signatures: PNG, JPEG, GIF, WebP.
- Compressed data: gzip and ZIP signatures.
- Protobuf-like binary: low-confidence label only when bytes have repeated
  varint/tag-shaped patterns and no stronger signature matches.
- Repeated-fill buffers: long runs of the same byte or very small repeating
  patterns.

Rendering:

- Summary largest-array previews, paths previews, referrer id previews, and
  list-strings standalone-array previews should all use the same classifier.
- Keep labels short and report-friendly.
- Do not redact content in this slice. Privacy-aware redaction remains a future
  feature.

Files likely touched:

- `src/preview.rs` for classifier types, heuristics, and unit tests.
- `src/rendered_result.rs`, `src/paths.rs`, and `src/referrer.rs` for rendering
  labels.
- JSON result structs if preview classification is emitted in machine output.
- `README.md` and `USERGUIDE.md` for examples.

## Slice 3: Android Capture Helper

Add an Android-focused helper command for reliable dump capture and validation.

Suggested command shape:

```bash
heaptrail android-capture --serial 192.168.50.98:5555 --package com.nexio.tv --out artifacts/run
heaptrail android-capture --serial 192.168.50.98:5555 --package com.nexio.tv --out artifacts/run --allocation-sites
```

Behavior:

- Resolve the target PID with `adb shell pidof <package>`.
- Optionally launch or foreground the package with `monkey -p <package> 1`.
- Record foreground activity evidence using `dumpsys window`.
- Dump to `/data/local/tmp/<generated-name>.hprof`.
- Pull the dump to the chosen output directory.
- Validate that the pulled file exists and is nonzero.
- Run a cheap heaptrail summary pass to record whether AllocationSites records
  are present.
- Write a transcript file containing commands run, PID, foreground evidence,
  local file path, dump size, heaptrail version, and allocation-site
  availability.

Allocation-tracking behavior:

- When `--allocation-sites` is requested, attempt the documented allocation
  tracking setup before dumping.
- Detect and report 0-byte HPROF output as a capture failure, not as a parser
  failure.
- If allocation tracking is unavailable or produces no AllocationSites records,
  keep the ordinary heap dump when valid and make the transcript explicit.

Safety and scope:

- Do not delete device files by default in the first implementation.
- Do not support multiple packages or repeated timeline captures in the first
  implementation.
- Treat ADB command failures as actionable errors with the failed command and
  stderr included.
- Prefer a small internal command-runner abstraction so tests can simulate ADB
  output without a device.

Files likely touched:

- `src/args.rs` for subcommand parsing.
- A new `src/android_capture.rs` module for capture orchestration.
- `src/main.rs` for dispatch.
- `src/errors.rs` for capture-specific errors if needed.
- `README.md` and `USERGUIDE.md` for capture workflow documentation.

## Testing Strategy

Slice 1:

- Parser tests for `--json-out`, including rejection without `--json`.
- Unit tests for output-path selection.
- Smoke tests on `test-heap-dumps/hprof-64.bin` verifying explicit JSON files
  are written for summary and at least one non-summary mode.
- Unit or integration test for corrected bitmap diagnostic using a fixture that
  lacks `android.graphics.Bitmap`.

Slice 2:

- Unit tests in `src/preview.rs` for each classifier.
- Rendering tests proving labels appear in summary/path/referrer preview blocks.
- Regression tests proving binary output still uses hex and text previews still
  escape control characters.

Slice 3:

- Unit tests with a fake command runner covering successful capture, missing
  PID, zero-byte pull, ADB command failure, and allocation-sites absent.
- A manual smoke checklist for a real Android device or emulator because CI may
  not have ADB/device access.

## Documentation Strategy

- Update README examples only for features that ship in the corresponding
  slice.
- Update `USERGUIDE.md` with a stable JSON output example after slice 1.
- Document classifier labels after slice 2.
- Document Android capture helper commands and failure modes after slice 3.
- Do not cut release sections or version bumps in normal feature PRs.

## Open Decisions

- Whether `--json-out` should imply `--json`. The design rejects this for
  explicitness and to avoid silent behavior changes.
- Whether `android-capture` should foreground the app by default. The first
  implementation should make this explicit with a flag if there is concern
  about changing app state.
- Whether the Android helper belongs in heaptrail long term or should later be
  split into a separate wrapper. The first implementation can keep it internal
  while the workflow stabilizes.
