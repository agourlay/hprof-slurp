# Android Mapping Support Design

## Goal

Make heaptrail output useful for obfuscated Android release builds by supporting
manual R8/ProGuard mapping files and automatic mapping discovery from a local
Gradle build that matches the app version installed on a device.

## Scope

This design covers three implementation slices:

1. Manual `--mapping <PATH>` support across heaptrail analysis modes.
2. Auto-discovery of the correct mapping file from a local Android project.
3. Input and capture robustness improvements that were exposed by real Nexio
   dumps.

`--group-holders` and broader exit-code taxonomy are valuable follow-ups, but
they are intentionally kept out of the first mapping plan so the symbolication
pipeline can land cleanly.

## User-Facing Behavior

Manual mapping:

```bash
heaptrail -i after.hprof --mapping app/build/outputs/mapping/universalRelease/mapping.txt --leak-suspects
heaptrail --diff-from before.hprof --diff-to after.hprof --mapping mapping.txt --json
```

When a mapping is supplied, heaptrail deobfuscates class names in text output
for summary, diff, referrers, paths, merged paths, leak suspects, allocation
sites, and bitmap reports. Field names in holder paths and referrer rows are
deobfuscated when the mapping contains a field mapping for the obfuscated holder
class and field name.

JSON keeps existing field names where possible for compatibility, and adds raw
obfuscated fields only where the deobfuscated value replaces an existing public
value. For example, a class row can expose `class_name` as deobfuscated and
`obfuscated_class_name` as the raw hprof name.

Automatic discovery:

```bash
heaptrail -i after.hprof \
  --auto-mapping \
  --project-root ~/Scripts/nexio \
  --package com.nexio.tv \
  --serial 192.168.50.98:5555 \
  --leak-suspects
```

`--auto-mapping` discovers the installed app version with `adb shell dumpsys
package <package>`, scans Gradle APK metadata under
`<project-root>/app/build/outputs/apk`, matches `applicationId`, `versionCode`,
and `versionName`, then selects
`<project-root>/app/build/outputs/mapping/<variantName>/mapping.txt`.

If discovery finds exactly one matching mapping file, heaptrail uses it and
prints a short notice before the normal report:

```text
Using mapping: /path/app/build/outputs/mapping/universalRelease/mapping.txt
Matched package com.nexio.tv versionCode=77 versionName=0.59 variant=universalRelease
```

If discovery cannot find a match, heaptrail continues without mapping only when
the user passed `--auto-mapping=optional`. The default `--auto-mapping` is
strict and exits with a clear error. If multiple matching mapping files exist,
heaptrail exits with candidate paths and asks the user to use `--mapping`.

Android capture:

```bash
heaptrail android-capture \
  --serial 192.168.50.98:5555 \
  --package com.nexio.tv \
  --out artifacts/run \
  --project-root ~/Scripts/nexio \
  --auto-mapping
```

Capture reports include mapping discovery metadata in the transcript: selected
mapping path, matched variant, version code/name, and R8 `pg_map_id` /
`pg_map_hash` when present. The capture command does not need to deobfuscate the
HProf itself; it records enough metadata for later analysis commands.

## Architecture

Add a small symbolication layer with three responsibilities:

- Parse R8/ProGuard mapping files into a compact lookup.
- Resolve a mapping path explicitly or by Gradle/ADB discovery.
- Apply symbolication at report construction boundaries, not deep inside the
  streaming parser.

The parser and graph builders keep raw HProf names. Analysis modules receive an
optional `Symbolicator` after the core data is computed and before rendering or
JSON serialization. This avoids contaminating object identity, class matching,
and graph construction with display-only name changes.

## Mapping Parser

Create `src/mapping.rs`.

Required data:

- `MappingInfo`
  - `path: PathBuf`
  - `pg_map_id: Option<String>`
  - `pg_map_hash: Option<String>`
- `Symbolicator`
  - `class_by_obfuscated: HashMap<String, String>`
  - `fields_by_obfuscated_class: HashMap<String, HashMap<String, String>>`

Parse these R8/ProGuard lines:

```text
com.nexio.tv.domain.model.MetaPreview -> d1.q2:
    java.lang.String title -> a
```

Class lookup maps `d1.q2` to `com.nexio.tv.domain.model.MetaPreview`.
Field lookup maps `(d1.q2, a)` to `title`.

Method mapping is not required in the first slice. Stack frame method
deobfuscation can follow later; the first version improves heap class and holder
field readability, which is the dominant heaptrail output surface.

Array handling:

- `d1.q2[]` becomes `com.nexio.tv.domain.model.MetaPreview[]`.
- `d1.q2[][]` becomes `com.nexio.tv.domain.model.MetaPreview[][]`.
- Primitive arrays such as `byte[]`, `char[]`, and `int[]` remain unchanged.

Synthetic names such as `R8$$REMOVED$$CLASS$$0` are valid map entries but should
not cause errors if encountered.

## Mapping Discovery

Create `src/mapping_discovery.rs`.

Inputs:

- `package: String`
- `project_root: PathBuf`
- `serial: Option<String>`
- `strict: bool`
- injectable command runner for tests

ADB query:

```bash
adb [-s SERIAL] shell dumpsys package PACKAGE
```

Parse at minimum:

- `versionCode=<number>`
- `versionName=<string>`

Gradle metadata scan:

- Search `app/build/outputs/apk/**/output-metadata.json`.
- Parse `applicationId`, `variantName`, and first element's `versionCode` /
  `versionName`.
- Match all three values when available:
  - `applicationId == package`
  - `versionCode == device versionCode`
  - `versionName == device versionName`

Mapping path:

```text
<project_root>/app/build/outputs/mapping/<variantName>/mapping.txt
```

If the exact path does not exist, report the matched APK metadata path and the
missing expected mapping path.

The first version supports the standard single-module Android application layout
used by Nexio and most Gradle Android apps. Multi-module customization can be
added later with `--mapping-search-root`.

## CLI Shape

Add global analysis options:

- `--mapping <PATH>`
- `--auto-mapping[=strict|optional]`
- `--project-root <DIR>`
- `--package <PACKAGE>`
- `--serial <SERIAL>`

`--mapping` and `--auto-mapping` are mutually exclusive.

`--auto-mapping` requires `--project-root` and `--package`. `--serial` is
optional and uses ADB's default target when omitted.

For `android-capture`, reuse:

- `--auto-mapping[=strict|optional]`
- `--project-root <DIR>`

The existing capture `--package` and `--serial` fields provide the app identity
and device selection. Capture records mapping metadata in the transcript, but
does not need a `--mapping` option unless later analysis is folded into capture.

## Error Handling

Manual mapping errors are strict:

- missing mapping file: `mapping file not found: <path>`
- unreadable mapping file: include the I/O error
- malformed class mapping: include line number and a short excerpt

Auto mapping strict mode exits when:

- ADB version query fails
- device version cannot be parsed
- no Gradle metadata matches
- multiple metadata files match
- matched mapping path is missing

Auto mapping optional mode emits a warning and proceeds without a mapping for
no-match and missing-path cases. It still exits on malformed local metadata if
that metadata is needed to decide between candidates.

Input robustness:

- Before parsing any user-supplied HProf input, check file size.
- If size is 0, return `input hprof is 0 bytes: <path>`.
- If parsing fails from an unexpected EOF, report `input hprof appears truncated`
  with the path and the underlying short-read error.

## Testing

Unit tests:

- mapping parser handles class lines, field lines, comments, arrays, and R8
  metadata headers.
- symbolicator leaves unknown classes and primitive arrays unchanged.
- discovery parses representative `dumpsys package` output.
- discovery matches Nexio-style `output-metadata.json` to
  `outputs/mapping/<variantName>/mapping.txt`.
- ambiguity and missing mapping path produce actionable errors.
- CLI rejects `--mapping` with `--auto-mapping`.
- CLI requires `--project-root` and `--package` for auto mapping.

Integration-style tests:

- run summary on `test-heap-dumps/hprof-64.bin` with a tiny mapping fixture and
  assert deobfuscated class names appear.
- run referrers/paths with a fixture mapping that deobfuscates holder fields.
- run capture with a fake ADB runner and fake Gradle metadata, then assert the
  transcript records mapping metadata.

Manual real-dump validation:

- Use Nexio `before.hprof` / `after.hprof`.
- Use explicit
  `--mapping ~/Scripts/nexio/app/build/outputs/mapping/universalRelease/mapping.txt`.
- Use `--auto-mapping --project-root ~/Scripts/nexio --package com.nexio.tv`.
- Verify summary/leak-suspect/referrer/path output shows deobfuscated names for
  classes that previously appeared as `d1.q2`, `zh.l1`, `ai.m`, and similar.

## Documentation

Update:

- `README.md` quick reference with manual and auto mapping examples.
- `USERGUIDE.md` Android capture and obfuscated-build sections.
- `HEAPTRAIL_FEATURE_GUIDE.md` only if explicitly requested, because it is
  currently an untracked local guide.

Do not bump release metadata or add release changelog entries.

## Open Follow-Ups

- `--group-holders`: group referrer rows by deobfuscated package/class family.
- Method and stack-frame deobfuscation from R8 method mappings.
- `--mapping-search-root` for non-standard module layouts.
- Exit-code taxonomy for CI.
- Native-memory correlation hooks and redaction mode from the feature guide.
