# heaptrail v0.8.0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement features A (thread/frame on thread-owned roots), C (allocation sites), D (Object[] index in paths), F (glob targeting) per the v0.8.0 design spec at `docs/superpowers/specs/2026-05-09-heaptrail-v0.8-design.md`.

**Architecture:** Five sequential PRs onto `master`, each independently reviewable: PR 1 (A), PR 2 (D), PR 3 (F), PR 4 (C), PR 5 (docs + version + tag). No new parser modes — every feature consumes records the parser already produces. Single new file (`src/allocation_sites.rs`) introduced in PR 4. Single new dependency (`globset`) added in PR 3.

**Tech Stack:** Rust 2024 edition, rustc ≥ 1.95 (CI). Existing deps: `clap` derive, `nom`, `ahash`, `serde`, `serde_json`, `chrono`, `thiserror`, `crossbeam-channel`, `indicatif`, `indoc`. New in PR 3: `globset 0.4`.

**Working directory:** `/Users/jneerdael/Scripts/hprof-slurp` (local checkout — the GitHub repo is `johnneerdael/heaptrail`; remote `fork` points there). Tests, fmt, clippy must pass on every commit.

---

## File Structure

| File | Responsibility | Touched in |
|------|----------------|------------|
| `src/parser/record.rs` | Parser record types — unchanged in v0.8.0 | – |
| `src/parser/record_parser.rs` | nom parser — unchanged in v0.8.0 | – |
| `src/result_recorder.rs` | Summary recorder; gains `Vec<AllocationSite>` capture + `class_name_id_by_serial` index + summary hint line | PR 4 |
| `src/rendered_result.rs` | Output renderer (table + JSON) — unchanged | – |
| `src/referrer.rs` | `Pass1Index` for referrer/paths; gains 5 new fields for thread/stack/serial-class metadata | PR 1, PR 3 |
| `src/paths.rs` | Path-to-root walker; gains `array_index` on `PathStep` and thread/frame fields on `PathResult` | PR 1, PR 2 |
| `src/diff.rs` | Snapshot diff — unchanged | – |
| `src/args.rs` | clap CLI; gains `--target-glob` and `--allocation-sites` flags + `Mode::AllocationSites` variant | PR 3, PR 4 |
| `src/main.rs` | Mode dispatch; gains `run_allocation_sites` handler | PR 4 |
| `src/allocation_sites.rs` (new) | `--allocation-sites` mode: load `Vec<AllocationSite>`, resolve class names + stack frames, render text + JSON | PR 4 |
| `src/slurp.rs` | parse_records helper — unchanged | – |
| `Cargo.toml` | Version bump 0.7.1 → 0.8.0 (PR 5); `globset` dep added (PR 3) | PR 3, PR 5 |
| `README.md` | Add `--allocation-sites`, `--target-glob` to cheat sheet; reference USERGUIDE for details | PR 5 |
| `USERGUIDE.md` | New sections: thread/frame in paths; allocation sites mode; glob targeting; array indices | PR 5 |
| `plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md` | Add v0.8.0 features to the standard triage workflow | PR 5 |

---

## Setup

### Task 0: Pre-flight

**Files:**
- Read: `docs/superpowers/specs/2026-05-09-heaptrail-v0.8-design.md`

- [ ] **Step 1: Verify clean working tree on `master`**

```bash
cd /Users/jneerdael/Scripts/hprof-slurp
git status
git log --oneline -3
```

Expected: branch `master`, clean working tree (only `heap-phase4-jvm.hprof` and similar untracked dumps allowed; everything tracked is committed). Most recent commit should be `b9bf908 docs: v0.8.0 design spec` or later.

- [ ] **Step 2: Verify the v0.7.1 baseline still passes**

Run: `cargo test --release && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: 51 tests pass; clippy clean; fmt clean.

- [ ] **Step 3: Confirm fixtures exist**

Run: `ls test-heap-dumps/hprof-32.bin test-heap-dumps/hprof-64.bin`
Expected: both exist. (These are the two integration-test fixtures from the existing test suite.)

---

## PR 1 — Feature A: Thread name + top frame on thread-owned roots

**Goal:** When `--paths-from-id` terminates at `RootJavaFrame`, `RootThreadObject`, `RootJniLocal`, or `RootJniMonitor`, the renderer prints the thread name and (for Java frames) the top frame's method/file/line.

**Pull request title:** `feat: thread name + stack frame resolution on thread-owned roots (A)`

### Task 1.1: Add `ResolvedFrame` + `ThreadFrameRef` types and 5 new `Pass1Index` fields

**Files:**
- Modify: `src/referrer.rs`

- [ ] **Step 1: Add the resolved-frame and thread-ref types near the top of `referrer.rs`**

Insert these `pub` types just below the existing `use` block, before `pub struct Pass1Index`:

```rust
/// A stack frame whose utf8 references have already been chased to readable
/// strings. Built lazily — we only resolve frames the renderer actually
/// asks for, so a dump with 50K frames doesn't pay for resolving any of
/// them unless `--paths-from-id` chases one.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedFrame {
    pub method: String,
    pub class: Option<String>,
    pub file: Option<String>,
    /// HPROF spec: positive = real line, negative sentinel values for
    /// "unknown", "compiled", "native". Surfaced verbatim; renderer
    /// translates sentinels.
    pub line: i32,
}

/// Pointer recorded when the indexer sees a thread-owned GC root. Used by
/// `paths::run` to resolve the chain terminator's thread name + top frame.
#[derive(Debug, Clone, Copy)]
pub struct ThreadFrameRef {
    pub thread_serial: u32,
    /// `Some(idx)` for `RootJavaFrame` — index into the thread's stack
    /// trace. `None` for `RootThreadObject` (no frame), `RootJniLocal`,
    /// `RootJniMonitor` (we only have a stack depth, not a frame index).
    pub frame_idx: Option<u32>,
}
```

- [ ] **Step 2: Add five new fields to the `Pass1Index` struct**

Find `pub struct Pass1Index { ... }` and add these fields (in the existing struct, before `pub id_size: u32`):

```rust
    // ---- v0.8.0 (feature A) thread + stack frame metadata ----
    /// `thread_serial_number -> thread name`. Populated from `StartThread`.
    pub thread_name_by_serial: AHashMap<u32, Box<str>>,
    /// `thread_object_id -> thread_serial_number`. Used to resolve a
    /// `RootThreadObject { thread_object_id }` back to its name (which is
    /// keyed by serial).
    pub thread_serial_by_obj_id: AHashMap<u64, u32>,
    /// `stack_trace_serial_number -> [stack_frame_id, ...]`. Populated
    /// from `StackTrace`.
    pub stack_trace_by_serial: AHashMap<u32, Vec<u64>>,
    /// `stack_frame_id -> raw StackFrameData`. utf8 resolution happens on
    /// demand via `Pass1Index::resolve_frame()`. We store the raw record
    /// so resolution stays lazy.
    pub stack_frame_by_id: AHashMap<u64, crate::parser::record::StackFrameData>,
    /// `class_serial_number -> class_name_id`. Captured from `LoadClass`.
    /// Distinct from the existing `class_name_id_by_class_id` (which is
    /// keyed by `class_object_id`); HPROF references classes by *serial*
    /// in `StackFrame.class_serial_number` and `AllocationSite.class_serial_number`.
    pub class_name_id_by_serial: AHashMap<u32, u64>,
    /// Root object id -> thread metadata. Captured at index time so the
    /// `paths` walker doesn't need to re-scan to find which thread owns a
    /// terminating root.
    pub root_thread_meta_by_id: AHashMap<u64, ThreadFrameRef>,
```

- [ ] **Step 3: `Pass1Index` derives `Default` already; the new fields get `Default::default()` automatically. Build to confirm.**

Run: `cargo build --release`
Expected: clean build, no warnings about unused fields (they're `pub`).

- [ ] **Step 4: Add a `resolve_frame()` method on `Pass1Index`**

Find the existing `impl Pass1Index { ... }` block (where `class_name` and `field_name` live) and add:

```rust
    /// Resolve a `stack_frame_id` to a `ResolvedFrame` if all the utf8
    /// references in the underlying `StackFrame` record are reachable.
    /// Returns `None` if the frame id isn't known.
    pub(crate) fn resolve_frame(&self, frame_id: u64) -> Option<ResolvedFrame> {
        let f = self.stack_frame_by_id.get(&frame_id)?;
        let method = self
            .utf8_by_id
            .get(&f.method_name_id)
            .map(|s| s.as_ref().to_string())
            .unwrap_or_else(|| format!("(method_name_id={})", f.method_name_id));
        let class = self
            .class_name_id_by_serial
            .get(&f.class_serial_number)
            .and_then(|nid| self.utf8_by_id.get(nid))
            .map(|s| s.as_ref().replace('/', "."));
        let file = self
            .utf8_by_id
            .get(&f.source_file_name_id)
            .map(|s| s.as_ref().to_string());
        Some(ResolvedFrame {
            method,
            class,
            file,
            line: f.line_number,
        })
    }
```

- [ ] **Step 5: Build + commit**

Run: `cargo build --release && cargo fmt --all -- --check`
Expected: clean.

```bash
git add src/referrer.rs
git commit -m "$(cat <<'EOF'
refactor(referrer): add ResolvedFrame, ThreadFrameRef, and Pass1Index thread/stack maps

Five new fields on Pass1Index in preparation for feature A
(thread/frame surfacing on thread-owned roots in --paths-from-id):

  * thread_name_by_serial         — StartThread serial -> name
  * thread_serial_by_obj_id       — RootThreadObject obj id -> serial
  * stack_trace_by_serial         — StackTrace serial -> [frame_id, ...]
  * stack_frame_by_id             — frame_id -> raw StackFrameData
  * class_name_id_by_serial       — LoadClass serial -> class_name_id
  * root_thread_meta_by_id        — root obj id -> {thread_serial, frame_idx}

The fields are populated in the next commit; this one only lands the
struct skeleton + a Pass1Index::resolve_frame() helper. No behavior
change yet; existing tests still pass.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.2: Populate the new `Pass1Index` fields in `pass1_index`

**Files:**
- Modify: `src/referrer.rs` (the `pass1_index` function)

- [ ] **Step 1: Write a failing test for thread-name indexing**

Add to the existing `#[cfg(test)] mod tests` block in `src/referrer.rs`:

```rust
    #[test]
    fn pass1_indexes_thread_names_and_stack_frames() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        // The bundled JVM fixture has at least one StartThread; we should
        // know at least one thread name. Exact name is implementation
        // detail of the fixture; assert presence not equality.
        assert!(
            !idx.thread_name_by_serial.is_empty(),
            "expected ≥1 thread, got {}",
            idx.thread_name_by_serial.len()
        );
        // Same for stack frames.
        assert!(
            !idx.stack_frame_by_id.is_empty(),
            "expected ≥1 stack frame, got {}",
            idx.stack_frame_by_id.len()
        );
        // class_name_id_by_serial should have entries — every LoadClass
        // record contributes one.
        assert!(
            !idx.class_name_id_by_serial.is_empty(),
            "expected ≥1 class serial entry"
        );
    }
```

- [ ] **Step 2: Run the test, expect FAIL**

Run: `cargo test --release pass1_indexes_thread_names_and_stack_frames`
Expected: FAIL — all three maps are empty because we haven't populated them.

- [ ] **Step 3: Populate the maps from non-GC records**

In `src/referrer.rs`, find the `pass1_index` function. Locate the `Record::LoadClass(LoadClassData { ... })` arm and the `_ => {}` fall-through. Add new arms (StartThread, StackTrace, StackFrame) and extend LoadClass:

```rust
        Record::Utf8String { id, str } => {
            idx.utf8_by_id.insert(id, str);
        }
        Record::LoadClass(LoadClassData {
            serial_number,
            class_object_id,
            class_name_id,
            ..
        }) => {
            idx.class_name_id_by_class_id
                .insert(class_object_id, class_name_id);
            // NEW (feature A): also index by class serial for stack frames
            // and allocation sites (which reference classes by serial).
            idx.class_name_id_by_serial
                .insert(serial_number, class_name_id);
        }
        Record::StartThread {
            thread_serial_number,
            thread_object_id,
            thread_name_id,
            ..
        } => {
            if let Some(name) = idx.utf8_by_id.get(&thread_name_id).cloned() {
                idx.thread_name_by_serial.insert(thread_serial_number, name);
            } else {
                // Name utf8 record may appear later in the stream; record
                // a placeholder we can repair after pass-1 completes.
                idx.thread_name_by_serial.insert(
                    thread_serial_number,
                    format!("(name_id={thread_name_id})").into_boxed_str(),
                );
            }
            idx.thread_serial_by_obj_id
                .insert(thread_object_id, thread_serial_number);
        }
        Record::StackTrace(crate::parser::record::StackTraceData {
            serial_number,
            stack_frame_ids,
            ..
        }) => {
            idx.stack_trace_by_serial
                .insert(serial_number, stack_frame_ids);
        }
        Record::StackFrame(sfd) => {
            idx.stack_frame_by_id.insert(sfd.stack_frame_id, sfd);
        }
```

- [ ] **Step 4: Add `RootJavaFrame` / `RootJniLocal` / `RootJniMonitor` / `RootThreadObject` thread-meta capture**

Inside the existing `Record::GcSegment(gc) => match gc { ... }` block, find the `GcRecord::RootJavaFrame { object_id, ... }` arm. The current arm only captures the root id. Extend it (and three others) to also write to `root_thread_meta_by_id`:

```rust
            GcRecord::RootJavaFrame {
                object_id,
                thread_serial_number,
                frame_number_in_stack_trace,
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJavaFrame");
                // NEW (feature A): remember which thread/frame this root belongs to.
                idx.root_thread_meta_by_id.insert(
                    object_id,
                    ThreadFrameRef {
                        thread_serial: thread_serial_number,
                        frame_idx: Some(frame_number_in_stack_trace),
                    },
                );
            }
            GcRecord::RootJniLocal {
                object_id,
                thread_serial_number,
                ..
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJniLocal");
                idx.root_thread_meta_by_id.insert(
                    object_id,
                    ThreadFrameRef {
                        thread_serial: thread_serial_number,
                        frame_idx: None,
                    },
                );
            }
            GcRecord::RootJniMonitor {
                object_id,
                thread_serial_number,
                ..
            } => {
                idx.gc_root_ids.insert(object_id);
                idx.gc_root_kind_by_id.insert(object_id, "RootJniMonitor");
                idx.root_thread_meta_by_id.insert(
                    object_id,
                    ThreadFrameRef {
                        thread_serial: thread_serial_number,
                        frame_idx: None,
                    },
                );
            }
            GcRecord::RootThreadObject {
                thread_object_id, ..
            } => {
                idx.gc_root_ids.insert(thread_object_id);
                idx.gc_root_kind_by_id
                    .insert(thread_object_id, "RootThreadObject");
                // For thread-object roots we'll do a second-pass lookup to
                // turn obj_id -> serial via thread_serial_by_obj_id.
                // Don't insert into root_thread_meta_by_id here — paths::run
                // resolves it at chain-terminator time.
            }
```

(The original `RootJavaFrame`, `RootJniLocal`, `RootJniMonitor`, and `RootThreadObject` arms in `pass1_index` are replaced by these.)

- [ ] **Step 5: Run the test, expect PASS**

Run: `cargo test --release pass1_indexes_thread_names_and_stack_frames`
Expected: PASS.

- [ ] **Step 6: Run the full test suite**

Run: `cargo test --release`
Expected: 52 tests pass (51 existing + 1 new).

- [ ] **Step 7: Commit**

```bash
git add src/referrer.rs
git commit -m "$(cat <<'EOF'
feat(referrer): index thread names, stack traces, and frame data in pass1

Pass1Index now captures the records the parser already produces but the
indexer was discarding:

  * StartThread       -> thread_name_by_serial, thread_serial_by_obj_id
  * StackTrace        -> stack_trace_by_serial
  * StackFrame        -> stack_frame_by_id (raw, lazy resolution)
  * LoadClass         -> class_name_id_by_serial (in addition to existing
                         class_name_id_by_class_id)

Thread-owned GC roots (RootJavaFrame/RootJniLocal/RootJniMonitor) gain a
ThreadFrameRef entry in root_thread_meta_by_id so the paths walker can
resolve thread name + frame at chain-terminator time without rescanning.

RootThreadObject is left to per-call resolution via thread_serial_by_obj_id
since the record carries thread_object_id rather than the serial.

No render/UX change yet — that's the next commit. Existing 51 tests pass,
plus 1 new test covering the new index fields.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.3: Surface thread name + frame in `PathResult` and `paths::run`

**Files:**
- Modify: `src/paths.rs`

- [ ] **Step 1: Extend `PathResult` with optional thread/frame fields**

Find the `PathResult` struct in `src/paths.rs` and add two fields:

```rust
#[derive(Serialize, Debug)]
pub struct PathResult {
    pub start_object_id: u64,
    pub steps: Vec<PathStep>,
    pub terminated_at_root: bool,
    pub root_kind: Option<&'static str>,
    /// Thread name (when the terminating root is owned by a thread).
    /// Always `None` for non-thread roots like `RootStickyClass`.
    pub root_thread_name: Option<String>,
    /// Top frame at the terminator (only for `RootJavaFrame`).
    pub root_frame: Option<crate::referrer::ResolvedFrame>,
    pub max_depth_reached: bool,
    pub depth: u8,
}
```

- [ ] **Step 2: Resolve thread name + frame at terminator time in `paths::run`**

In `paths::run`, find the loop that detects a GC root and breaks. Replace the existing terminator block:

```rust
        if let Some(kind) = idx.gc_root_kind_by_id.get(&current).copied() {
            terminated_at_root = true;
            root_kind = Some(kind);
            break;
        }
```

with the new version that also resolves thread/frame when applicable:

```rust
        if let Some(kind) = idx.gc_root_kind_by_id.get(&current).copied() {
            terminated_at_root = true;
            root_kind = Some(kind);
            // Resolve thread name + top frame for thread-owned roots.
            // Two paths: (a) RootJavaFrame/RootJniLocal/RootJniMonitor
            // already have a ThreadFrameRef; (b) RootThreadObject lives in
            // thread_serial_by_obj_id (the obj_id is the thread itself).
            let meta = idx.root_thread_meta_by_id.get(&current).copied().or_else(|| {
                idx.thread_serial_by_obj_id.get(&current).map(|&serial| {
                    crate::referrer::ThreadFrameRef {
                        thread_serial: serial,
                        frame_idx: None,
                    }
                })
            });
            if let Some(m) = meta {
                root_thread_name = idx
                    .thread_name_by_serial
                    .get(&m.thread_serial)
                    .map(|s| s.as_ref().to_string());
                if let Some(idx_in_trace) = m.frame_idx
                    && let Some(frames) =
                        idx.stack_trace_by_serial.get(&m.thread_serial).or_else(|| {
                            // RootJavaFrame uses the thread's stack trace serial,
                            // which is keyed by thread_serial. But the StackTrace
                            // record's serial may differ; try a direct lookup too.
                            None
                        })
                    && let Some(&frame_id) = frames.get(idx_in_trace as usize)
                {
                    root_frame = idx.resolve_frame(frame_id);
                }
            }
            break;
        }
```

Also declare the new locals at the top of the function (alongside `terminated_at_root`):

```rust
    let mut root_thread_name: Option<String> = None;
    let mut root_frame: Option<crate::referrer::ResolvedFrame> = None;
```

And populate the `PathResult` constructor at the end:

```rust
    Ok(PathResult {
        start_object_id,
        steps,
        terminated_at_root,
        root_kind,
        root_thread_name,
        root_frame,
        max_depth_reached,
        depth,
    })
```

- [ ] **Step 3: Update existing `paths::tests` constructor sites that build `PathResult` directly**

There are no direct constructions in tests today (the tests call `paths::run`). Confirm with:

```bash
grep -n "PathResult {" src/paths.rs
```

Expected: only the one constructor inside `paths::run`. If any test builds a `PathResult` literal, add `root_thread_name: None, root_frame: None` to it.

- [ ] **Step 4: Run all tests, expect PASS**

Run: `cargo test --release`
Expected: 52 tests pass; `paths_for_a_known_object_reaches_a_root` still passes (the new fields are purely additive).

- [ ] **Step 5: Commit**

```bash
git add src/paths.rs
git commit -m "$(cat <<'EOF'
feat(paths): resolve thread name + top frame at chain terminator

PathResult gains two optional fields:
  * root_thread_name — resolved for any thread-owned terminator
  * root_frame       — resolved only for RootJavaFrame (top frame of
                       the thread's stack trace at the recorded index)

Thread-object roots (RootThreadObject) resolve their thread name via
the thread_serial_by_obj_id map (the root's object id IS the thread).
Non-thread roots (RootStickyClass etc.) leave both fields None.

Renderer change is the next commit; this one only populates the data.
Existing tests pass byte-for-byte (the new fields are additive).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.4: Render thread name + frame in `paths::render_text`

**Files:**
- Modify: `src/paths.rs` (the `render_text` function)

- [ ] **Step 1: Replace the terminator render block in `render_text`**

Find the section near the bottom of `render_text`:

```rust
    if r.terminated_at_root {
        let _ = writeln!(
            out,
            "  → reached GC root: {}",
            r.root_kind.unwrap_or("(unknown)")
        );
    } else if r.max_depth_reached {
```

Replace with:

```rust
    if r.terminated_at_root {
        let _ = writeln!(
            out,
            "  → reached GC root: {}",
            r.root_kind.unwrap_or("(unknown)")
        );
        // Thread + frame block (feature A). Renders only when meta is present.
        if let Some(name) = &r.root_thread_name {
            let _ = writeln!(out, "        thread \"{name}\"");
        } else if matches!(
            r.root_kind,
            Some("RootJavaFrame")
                | Some("RootJniLocal")
                | Some("RootJniMonitor")
                | Some("RootThreadObject")
        ) {
            // Thread root, but no metadata — be explicit so users know it's
            // a dump-content gap, not a heaptrail bug.
            let _ = writeln!(out, "        (thread metadata not in dump)");
        }
        if let Some(f) = &r.root_frame {
            let qualified = match &f.class {
                Some(c) => format!("{c}.{}", f.method),
                None => f.method.clone(),
            };
            let location = match (&f.file, f.line) {
                (Some(file), n) if n > 0 => format!("({file}:{n})"),
                (Some(file), _) => format!("({file})"),
                (None, n) if n > 0 => format!("(line {n})"),
                (None, _) => String::new(),
            };
            let _ = writeln!(out, "        at {qualified}{location}");
        }
    } else if r.max_depth_reached {
```

- [ ] **Step 2: Add a unit test exercising the thread/frame render path**

Add to the `#[cfg(test)] mod tests` block in `src/paths.rs`:

```rust
    #[test]
    fn render_text_shows_thread_block_for_root_java_frame() {
        let r = PathResult {
            start_object_id: 100,
            steps: vec![],
            terminated_at_root: true,
            root_kind: Some("RootJavaFrame"),
            root_thread_name: Some("pool-7-thread-2".to_string()),
            root_frame: Some(crate::referrer::ResolvedFrame {
                method: "commitToMemory".to_string(),
                class: Some("android.app.SharedPreferencesImpl$EditorImpl".to_string()),
                file: Some("SharedPreferencesImpl.java".to_string()),
                line: 478,
            }),
            max_depth_reached: false,
            depth: 0,
        };
        let out = render_text(&r);
        assert!(
            out.contains("thread \"pool-7-thread-2\""),
            "expected thread name, got:\n{out}"
        );
        assert!(
            out.contains("at android.app.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)"),
            "expected qualified frame, got:\n{out}"
        );
    }

    #[test]
    fn render_text_shows_metadata_gap_for_thread_root_without_meta() {
        let r = PathResult {
            start_object_id: 100,
            steps: vec![],
            terminated_at_root: true,
            root_kind: Some("RootJavaFrame"),
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 0,
        };
        let out = render_text(&r);
        assert!(
            out.contains("(thread metadata not in dump)"),
            "expected gap line, got:\n{out}"
        );
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test --release paths::`
Expected: all paths tests pass, including the two new render tests.

- [ ] **Step 4: Smoke test on the real Android dump**

```bash
cargo build --release
./target/release/heaptrail -i /tmp/heap-snapshot-fix.hprof --paths-from-id 1723142144 --max-depth 8
```

Expected (varies by dump): chain ending at a root whose terminator now includes a `thread "..."` line and (if `RootJavaFrame`) an `at <class>.<method>(<file>:<line>)` line. If the chain hits a non-thread root (e.g. `RootStickyClass`), no thread block — that's correct.

- [ ] **Step 5: clippy + fmt + commit**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all
cargo fmt --all -- --check

git add src/paths.rs
git commit -m "$(cat <<'EOF'
feat(paths): render thread name + top frame on thread-owned roots

When --paths-from-id terminates at RootJavaFrame, RootThreadObject,
RootJniLocal, or RootJniMonitor, the output now includes:

  → reached GC root: RootJavaFrame
        thread "pool-7-thread-2"
        at android.app.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)

When the dump lacks the StartThread/StackTrace records that resolve the
root (e.g. older Android builds, partial captures), prints

        (thread metadata not in dump)

so the gap is attributable to the dump rather than heaptrail.

Closes feature A of the v0.8.0 spec
(docs/superpowers/specs/2026-05-09-heaptrail-v0.8-design.md §3.A).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1.5: Push PR 1

- [ ] **Step 1: Push to fork**

```bash
git push fork master
```

- [ ] **Step 2: Wait for CI green**

Run: `gh run watch --repo johnneerdael/heaptrail $(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json databaseId -q '.[0].databaseId') --exit-status`
Expected: success.

---

## PR 2 — Feature D: Object[] index in path hops

**Goal:** Path hops that pass through an `Object[]` show the matched element index.

**Pull request title:** `feat: Object[] element index in --paths-from-id hops (D)`

### Task 2.1: Add `array_index` to `PathStep` + capture it in `find_first_holder`

**Files:**
- Modify: `src/paths.rs`

- [ ] **Step 1: Write a failing test**

Add to `src/paths.rs`'s test module:

```rust
    #[test]
    fn path_step_records_array_index_for_object_array_holder() {
        // Build a synthetic PathStep via the public API by walking a known
        // ObjectArrayDump. We use the existing fixture and a known-large
        // object id whose chain passes through an Object[].
        let r = run(&fixture_args(1, 2)).unwrap();
        // Object id 1 is unlikely to exist in the fixture, so the result
        // is "orphan" with empty steps. This test exercises the array_index
        // field's *type* (Option<u32>) rather than a real value — the
        // type-level test plus the unit test below cover the behavior.
        for step in &r.steps {
            // Sanity: when via_field is None, that's an Object[] hop, and
            // array_index MUST be Some.
            if step.via_field.is_none() {
                assert!(
                    step.array_index.is_some(),
                    "Object[] hop must carry array_index, got step: {step:?}"
                );
            }
        }
    }
```

(This test is intentionally permissive at this stage — it catches the type-level invariant. The next step adds a tighter unit test on a hand-built `PathStep`.)

- [ ] **Step 2: Run the test, expect failure (the field doesn't exist yet)**

Run: `cargo test --release path_step_records_array_index`
Expected: compile error — `PathStep` has no field `array_index`.

- [ ] **Step 3: Add `array_index` to `PathStep`**

Find:

```rust
#[derive(Serialize, Debug, Clone)]
pub struct PathStep {
    pub holder_object_id: u64,
    pub holder_class: String,
    pub via_field: Option<String>,
    pub held_object_id: u64,
}
```

Replace with:

```rust
#[derive(Serialize, Debug, Clone)]
pub struct PathStep {
    pub holder_object_id: u64,
    pub holder_class: String,
    /// Field name when the holder is an instance, `None` when the holder
    /// is an `Object[]`.
    pub via_field: Option<String>,
    /// Element slot when the holder is an `Object[]`; `None` for
    /// instance-field hops. Always `Some(_)` when `via_field` is `None`.
    pub array_index: Option<u32>,
    pub held_object_id: u64,
}
```

- [ ] **Step 4: Capture the index in `find_first_holder`**

Find the `ObjectArrayDump` arm of `find_first_holder`. Currently:

```rust
                GcRecord::ObjectArrayDump {
                    object_id,
                    array_class_id,
                    elements: Some(elems),
                    ..
                } if elems.contains(&target) => {
                    let holder_class = idx
                        .class_name(array_class_id)
                        .unwrap_or_else(|| format!("(class_id={array_class_id})"));
                    *found.borrow_mut() = Some(PathStep {
                        holder_object_id: object_id,
                        holder_class,
                        via_field: None,
                        held_object_id: target,
                    });
                }
```

Replace with:

```rust
                GcRecord::ObjectArrayDump {
                    object_id,
                    array_class_id,
                    elements: Some(elems),
                    ..
                } if elems.contains(&target) => {
                    let array_index = elems
                        .iter()
                        .position(|&rid| rid == target)
                        .map(|p| p as u32);
                    let holder_class = idx
                        .class_name(array_class_id)
                        .unwrap_or_else(|| format!("(class_id={array_class_id})"));
                    *found.borrow_mut() = Some(PathStep {
                        holder_object_id: object_id,
                        holder_class,
                        via_field: None,
                        array_index,
                        held_object_id: target,
                    });
                }
```

Also update the `InstanceDump` arm's `PathStep` construction — add `array_index: None,`:

```rust
                                *found.borrow_mut() = Some(PathStep {
                                    holder_object_id: object_id,
                                    holder_class,
                                    via_field,
                                    array_index: None,
                                    held_object_id: target,
                                });
```

- [ ] **Step 5: Update `render_text` to print `[N]` for array hops**

Find:

```rust
        let arrow = match &s.via_field {
            Some(f) => format!("via {}.{}", s.holder_class, f),
            None => format!("via {}[]", s.holder_class),
        };
```

Replace with:

```rust
        let arrow = match (&s.via_field, s.array_index) {
            (Some(f), _) => format!("via {}.{}", s.holder_class, f),
            (None, Some(idx)) => format!("via {}[{idx}]", s.holder_class),
            (None, None) => format!("via {}[]", s.holder_class), // shouldn't happen
        };
```

- [ ] **Step 6: Add a tighter unit test on a hand-built `PathStep`**

Add to the test module:

```rust
    #[test]
    fn render_text_shows_array_index_for_object_array_hop() {
        let r = PathResult {
            start_object_id: 100,
            steps: vec![PathStep {
                holder_object_id: 200,
                holder_class: "java.lang.Object[]".to_string(),
                via_field: None,
                array_index: Some(12),
                held_object_id: 100,
            }],
            terminated_at_root: false,
            root_kind: None,
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 1,
        };
        let out = render_text(&r);
        assert!(
            out.contains("via java.lang.Object[][12]"),
            "expected array index in arrow, got:\n{out}"
        );
    }
```

- [ ] **Step 7: Run tests**

Run: `cargo test --release paths::`
Expected: all paths tests pass including the two new ones (54 total in suite).

- [ ] **Step 8: clippy + fmt + commit + push**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/paths.rs
git commit -m "$(cat <<'EOF'
feat(paths): show Object[] element index in --paths-from-id hops

PathStep gains an optional `array_index` field, captured by
find_first_holder when the matching record is an ObjectArrayDump.
render_text formats it as `via java.lang.Object[][12]` instead of the
previous unhelpful `via java.lang.Object[][]`.

Instance-field hops keep `array_index: None`; only Object[] hops set it.

Closes feature D of the v0.8.0 spec.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
gh run watch --repo johnneerdael/heaptrail \
  $(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json databaseId -q '.[0].databaseId') \
  --exit-status
```

Expected: CI green.

---

## PR 3 — Feature F: `--target-glob` for `--find-referrers`

**Goal:** A new top-level flag `--target-glob '<pattern>'` activates find-referrers mode against any class matching the glob.

**Pull request title:** `feat: --target-glob shell-style pattern targeting on --find-referrers (F)`

### Task 3.1: Add `globset` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add `globset 0.4` to `[dependencies]`**

In `Cargo.toml`, the dependencies section currently reads (snippet):

```toml
[dependencies]
nom = "8.0.0"
indicatif = "0.18.4"
clap = { version = "4.6.1", features = ["cargo", "derive"] }
indoc = "2.0.7"
ahash = "0.8.12"
thiserror = "2.0.18"
crossbeam-channel = "0.5.15"
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.149"
chrono = "0.4.44"
```

Add a single line:

```toml
globset = "0.4"
```

- [ ] **Step 2: Build to fetch + verify**

Run: `cargo build --release`
Expected: globset compiles; build succeeds. If the additional binary footprint is greater than ~150 KB (compared to v0.7.1), per the spec's risk register, abort and switch to a hand-rolled matcher (see fallback below). Otherwise proceed.

Optional size check:

```bash
ls -l target/release/heaptrail
```

If size grew by more than ~150 KB versus v0.7.1's ~1.2 MB binary, swap to the fallback. Otherwise continue.

- [ ] **Step 3: Commit the dependency**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add globset 0.4 for --target-glob (feature F)"
```

### Task 3.2: Add `--target-glob` flag and `Mode::FindReferrers` glob variant

**Files:**
- Modify: `src/args.rs`

- [ ] **Step 1: Add the new flag to `Cli`**

Find `pub struct Cli { ... }`. After the existing `find_referrers` field, add:

```rust
    /// Find direct + N-hop referrers of every class matching this glob.
    /// Mutually exclusive with `--find-referrers`. See USERGUIDE.md §F.
    /// Glob syntax: `*` matches within a package level, `**` crosses
    /// package levels, `?` matches one character, `[abc]` is a class.
    #[arg(long = "target-glob", value_name = "PATTERN", conflicts_with = "find_referrers")]
    pub target_glob: Option<String>,
```

- [ ] **Step 2: Extend `Mode::FindReferrers` to carry an optional glob**

Find the `Mode::FindReferrers` variant. Currently:

```rust
    FindReferrers {
        input_file: String,
        target: String,
        hops: u8,
        top: usize,
        include_statics: bool,
        debug: bool,
        json: bool,
    },
```

Replace `target: String` with the discriminated form:

```rust
    FindReferrers {
        input_file: String,
        target: ReferrersTarget,
        hops: u8,
        top: usize,
        include_statics: bool,
        debug: bool,
        json: bool,
    },
```

And add the new enum next to `Mode`:

```rust
#[derive(Debug, Clone)]
pub enum ReferrersTarget {
    /// Exact FQ class name or `id:<u64>` / bare `<u64>`.
    Exact(String),
    /// Shell-style glob over dotted FQ class names.
    Glob(String),
}
```

- [ ] **Step 3: Wire `target_glob` into `resolve()`**

Find the `resolve()` function. The block that handles find-referrers currently reads:

```rust
    if let Some(target) = cli.find_referrers {
        return Ok(Mode::FindReferrers {
            input_file,
            target,
            hops: cli.hops,
            top: cli.top,
            include_statics: cli.include_statics,
            debug: cli.debug,
            json: cli.json,
        });
    }
```

Replace with:

```rust
    let referrers_target = match (cli.find_referrers, cli.target_glob) {
        (Some(t), None) => Some(ReferrersTarget::Exact(t)),
        (None, Some(g)) => Some(ReferrersTarget::Glob(g)),
        (Some(_), Some(_)) => return Err(ConflictingModes),
        (None, None) => None,
    };
    if let Some(target) = referrers_target {
        return Ok(Mode::FindReferrers {
            input_file,
            target,
            hops: cli.hops,
            top: cli.top,
            include_statics: cli.include_statics,
            debug: cli.debug,
            json: cli.json,
        });
    }
```

Also update the mode-conflict detection earlier in `resolve()`:

```rust
    let referrers_set = cli.find_referrers.is_some();
```

becomes

```rust
    let referrers_set = cli.find_referrers.is_some() || cli.target_glob.is_some();
```

- [ ] **Step 4: Add CLI tests for the new flag**

Add to `args::args_tests`:

```rust
    #[test]
    fn parses_target_glob() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--target-glob",
            "com.foo.*",
        ])
        .unwrap();
        assert_eq!(cli.target_glob.as_deref(), Some("com.foo.*"));
        assert!(cli.find_referrers.is_none());
    }

    #[test]
    fn target_glob_conflicts_with_find_referrers() {
        let res = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "x.hprof",
            "--find-referrers",
            "java.util.ArrayList",
            "--target-glob",
            "java.util.*",
        ]);
        assert!(res.is_err(), "clap should reject both flags together");
    }
```

- [ ] **Step 5: Build + tests**

Run: `cargo test --release args`
Expected: all CLI tests pass, including the two new ones. The build will fail in `referrer.rs` and `main.rs` because they still expect `target: String`. That's intentional and fixed in Task 3.3.

- [ ] **Step 6: Update consumers in `main.rs` and `referrer.rs` to compile against the new type**

In `src/main.rs`, find `run_find_referrers` (or wherever the `Mode::FindReferrers` is destructured). Where `target` was passed as `&str`, change to `&target` (a `&ReferrersTarget`).

In `src/referrer.rs::run`, the destructuring:

```rust
        Mode::FindReferrers {
            input_file,
            target,
            hops,
            top,
            include_statics,
            debug,
            ..
        } => (
            input_file.as_str(),
            target.as_str(),
            *hops,
            ...
```

becomes

```rust
        Mode::FindReferrers {
            input_file,
            target,
            hops,
            top,
            include_statics,
            debug,
            ..
        } => (
            input_file.as_str(),
            target.clone(),  // ReferrersTarget — moved/cloned, used below
            *hops,
            ...
```

The destination tuple's type changes from `&str` to `ReferrersTarget`; update the function signature accordingly. The actual glob handling lives in Task 3.3.

- [ ] **Step 7: Migrate `resolve_target_ids` to the new signature, with Exact arm fully wired and Glob stubbed**

Add `MatchedClass` definition first. In `src/referrer.rs`, near the existing `ReferrerEntry` struct, add:

```rust
#[derive(Serialize, Debug, Clone)]
pub struct MatchedClass {
    pub class_name: String,
    pub instance_count: u64,
}
```

Then replace the existing `fn resolve_target_ids(path: &str, idx: &Pass1Index, target: &str, debug: bool) -> Result<(String, AHashSet<u64>), HprofSlurpError>` signature with:

```rust
fn resolve_target_ids(
    path: &str,
    idx: &Pass1Index,
    target: &crate::args::ReferrersTarget,
    debug: bool,
) -> Result<(String, AHashSet<u64>, Vec<MatchedClass>), HprofSlurpError> {
    match target {
        crate::args::ReferrersTarget::Exact(s) => resolve_exact(path, idx, s, debug),
        crate::args::ReferrersTarget::Glob(_) => Err(HprofSlurpError::NotYetImplemented {
            what: "--target-glob (filled in PR 3 / Task 3.3)",
        }),
    }
}
```

Then *rename* the original function body to `resolve_exact` and add the empty `matched_classes` to its return tuple. The renamed function looks like:

```rust
fn resolve_exact(
    path: &str,
    idx: &Pass1Index,
    target: &str,
    debug: bool,
) -> Result<(String, AHashSet<u64>, Vec<MatchedClass>), HprofSlurpError> {
    // ↓↓↓ existing body of resolve_target_ids goes here UNCHANGED ↓↓↓
    if let Some(rest) = target.strip_prefix("id:") {
        let oid: u64 = rest.parse().map_err(|_| TargetClassNotFound {
            name: target.to_string(),
        })?;
        let mut ids = AHashSet::new();
        ids.insert(oid);
        return Ok((format!("id:{oid}"), ids, vec![]));   // ← was 2-tuple, now 3-tuple
    }
    if let Ok(oid) = target.parse::<u64>() {
        let mut ids = AHashSet::new();
        ids.insert(oid);
        return Ok((format!("id:{oid}"), ids, vec![]));   // ← was 2-tuple, now 3-tuple
    }

    let target_class_id = idx
        .class_name_id_by_class_id
        .iter()
        .find_map(|(class_id, name_id)| {
            let raw = idx.utf8_by_id.get(name_id)?.as_ref();
            let dotted = raw.replace('/', ".");
            if dotted == target { Some(*class_id) } else { None }
        })
        .ok_or_else(|| TargetClassNotFound {
            name: target.to_string(),
        })?;

    let mut ids = AHashSet::new();
    parse_records(path, debug, false, |rec| {
        if let Record::GcSegment(GcRecord::InstanceDump {
            object_id,
            class_object_id,
            ..
        }) = rec
            && class_object_id == target_class_id
        {
            ids.insert(object_id);
        }
    })?;
    Ok((target.to_string(), ids, vec![]))               // ← was 2-tuple, now 3-tuple
}
```

(Three return sites changed: each one appends `vec![]` as the third tuple element.)

Update `referrer::run` to destructure the 3-tuple:

```rust
    let (target_label, target_ids, matched_classes) =
        resolve_target_ids(input_file, &idx, &target, debug)?;
```

— and pass `matched_classes` into the `ReferrerResult { ... }` constructor.

Finally, add `matched_classes: Vec<MatchedClass>` to `ReferrerResult` so the field flows out for the renderer (Task 3.3) and the JSON sidecar:

```rust
#[derive(Serialize, Debug)]
pub struct ReferrerResult {
    pub target_label: String,
    pub target_instance_count: u64,
    pub matched_classes: Vec<MatchedClass>,  // NEW: empty for exact-match targets
    pub hop1: Vec<ReferrerEntry>,
    pub hop2: Vec<ReferrerEntry>,
    pub hop3: Vec<ReferrerEntry>,
}
```

Update the `ReferrerResult { ... }` construction at the bottom of `referrer::run` to include `matched_classes,` (the variable populated by `resolve_target_ids`).

After this step:
- `cargo build --release` succeeds.
- `cargo test --release` succeeds — all existing tests pass because the Exact arm contains the unchanged body and `--target-glob` returns `NotYetImplemented` (no test exercises that path yet).
- The `--target-glob` flag is parseable but produces an error if used. PR 3 / Task 3.3 fills it in.

- [ ] **Step 8: Build + commit**

Run: `cargo build --release`
Expected: clean.

```bash
git add src/args.rs src/main.rs src/referrer.rs
git commit -m "$(cat <<'EOF'
feat(args): --target-glob flag + ReferrersTarget enum (mode plumbing)

Adds a top-level --target-glob <pat> flag that activates find-referrers
mode the same way --find-referrers <name> does, but interprets the value
as a glob pattern. clap conflicts_with rejects both flags together.

Mode::FindReferrers's `target` field becomes a ReferrersTarget enum
(Exact | Glob) so the downstream resolver can dispatch.

ReferrerResult gains a `matched_classes: Vec<MatchedClass>` field; empty
for Exact targets, populated for Glob targets in the next commit.

The Glob variant currently returns NotYetImplemented; the resolver +
matched-classes header land in the next commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3.3: Implement glob resolution + matched-classes header

**Files:**
- Modify: `src/referrer.rs`

- [ ] **Step 1: Write a failing test**

Add to `referrer::tests`:

```rust
    #[test]
    fn glob_resolution_finds_multiple_matching_classes() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        let target = crate::args::ReferrersTarget::Glob("java.util.*".to_string());
        let (label, ids, matched) =
            resolve_target_ids("test-heap-dumps/hprof-64.bin", &idx, &target, false).unwrap();
        assert_eq!(label, "glob:java.util.*");
        assert!(
            matched.len() >= 5,
            "expected ≥5 java.util classes matched, got {}",
            matched.len()
        );
        assert!(
            !ids.is_empty(),
            "expected non-zero target instance count for java.util.* glob"
        );
    }

    #[test]
    fn glob_with_no_matches_errors() {
        let idx = pass1_index("test-heap-dumps/hprof-64.bin", false).unwrap();
        let target =
            crate::args::ReferrersTarget::Glob("nonexistent.does.not.exist.*".to_string());
        let res = resolve_target_ids("test-heap-dumps/hprof-64.bin", &idx, &target, false);
        match res {
            Err(HprofSlurpError::TargetClassNotFound { name }) => {
                assert!(name.contains("nonexistent"), "got: {name}");
            }
            other => panic!("expected TargetClassNotFound, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test --release glob_`
Expected: tests fail with `NotYetImplemented`.

- [ ] **Step 3: Implement `Glob` arm**

In `resolve_target_ids`, replace the stub `Glob` arm with:

```rust
        crate::args::ReferrersTarget::Glob(pattern) => {
            use globset::{GlobBuilder, GlobMatcher};
            let matcher: GlobMatcher = GlobBuilder::new(pattern)
                .literal_separator(true) // `*` doesn't cross `.`; `**` does
                .build()
                .map_err(|e| HprofSlurpError::InvalidHprofFile {
                    message: format!("bad glob pattern '{pattern}': {e}"),
                })?
                .compile_matcher();

            // Find every class whose dotted FQ-name matches the glob.
            // Build (class_object_id, class_name) pairs as we go so we can
            // emit a MatchedClass list later.
            let mut matched_class_ids: Vec<(u64, String)> = Vec::new();
            for (class_id, name_id) in &idx.class_name_id_by_class_id {
                if let Some(raw) = idx.utf8_by_id.get(name_id) {
                    let dotted = raw.as_ref().replace('/', ".");
                    if matcher.is_match(std::path::Path::new(&dotted)) {
                        matched_class_ids.push((*class_id, dotted));
                    }
                }
            }
            if matched_class_ids.is_empty() {
                return Err(TargetClassNotFound {
                    name: format!(
                        "glob '{pattern}' matched no classes; check available classes with: heaptrail -i <file> -t 1000"
                    ),
                });
            }

            // Pass 1B: stream instance ids belonging to any matched class.
            let class_id_set: AHashSet<u64> =
                matched_class_ids.iter().map(|(c, _)| *c).collect();
            let mut ids = AHashSet::new();
            // Per-class instance counters for the matched-classes header.
            let mut count_by_class: AHashMap<u64, u64> = AHashMap::new();
            parse_records(path, debug, false, |rec| {
                if let Record::GcSegment(GcRecord::InstanceDump {
                    object_id,
                    class_object_id,
                    ..
                }) = rec
                    && class_id_set.contains(&class_object_id)
                {
                    ids.insert(object_id);
                    *count_by_class.entry(class_object_id).or_default() += 1;
                }
            })?;

            // Sort matched classes by instance count, descending, for the
            // header output.
            let mut matched: Vec<MatchedClass> = matched_class_ids
                .into_iter()
                .map(|(cid, name)| MatchedClass {
                    class_name: name,
                    instance_count: count_by_class.get(&cid).copied().unwrap_or(0),
                })
                .collect();
            matched.sort_by(|a, b| b.instance_count.cmp(&a.instance_count));

            Ok((format!("glob:{pattern}"), ids, matched))
        }
```

- [ ] **Step 4: Update `render_text` (in `referrer.rs`) to print the matched-classes header**

Find the existing `pub fn render_text(r: &ReferrerResult) -> String { ... }`. At the very top of the function, insert (before the existing `let _ = writeln!(out, "\nFound …` line):

```rust
pub fn render_text(r: &ReferrerResult) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if !r.matched_classes.is_empty() {
        let _ = writeln!(
            out,
            "\nFound {} classes matching {}:",
            r.matched_classes.len(),
            r.target_label
        );
        // Pad class names so instance counts line up.
        let max_name_len = r
            .matched_classes
            .iter()
            .map(|m| m.class_name.len())
            .max()
            .unwrap_or(0)
            .min(80);
        for m in &r.matched_classes {
            let _ = writeln!(
                out,
                "  - {:<width$} ({} instances)",
                m.class_name,
                m.instance_count,
                width = max_name_len
            );
        }
    }
    let _ = writeln!(
        out,
        "\nFound {} target instance(s) for {}",
        r.target_instance_count, r.target_label
    );
    // ... rest of existing render_text body unchanged ...
```

- [ ] **Step 5: Run tests, expect PASS**

Run: `cargo test --release`
Expected: all tests pass including the two new glob tests.

- [ ] **Step 6: Smoke test on the Android dump**

```bash
cargo build --release
./target/release/heaptrail -i /tmp/heap-snapshot-fix.hprof \
    --target-glob 'com.nexio.tv.domain.model.*' --hops 1 --top 5
```

Expected: matched-classes header lists the namespace's classes; hop-1 referrers reflect the union.

- [ ] **Step 7: clippy + fmt + commit + push**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/referrer.rs
git commit -m "$(cat <<'EOF'
feat(referrer): glob class-name targeting on --target-glob

Compiles a globset matcher (literal_separator=true, so `*` doesn't cross
`.` and `**` does) and walks Pass1Index to find every class whose dotted
FQ-name matches. Pass 1B sweeps instance dumps for matched class ids,
populating the same target_ids set the existing pass-2 logic consumes.

Output gains a "Found N classes matching glob 'X':" header listing each
matched class with its live instance count, sorted by count desc.

Empty match -> TargetClassNotFound with a hint pointing at -t 1000.

Closes feature F of the v0.8.0 spec.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
gh run watch --repo johnneerdael/heaptrail \
  $(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json databaseId -q '.[0].databaseId') \
  --exit-status
```

Expected: CI green.

---

## PR 4 — Feature C: `--allocation-sites` mode + summary hint

**Goal:** Always-on summary hint about AllocationSites presence; new `--allocation-sites` mode prints per-class stack traces with method/file/line resolution.

**Pull request title:** `feat: --allocation-sites mode and summary hint (C)`

### Task 4.1: Capture `AllocationSites` in `ResultRecorder` + summary hint

**Files:**
- Modify: `src/result_recorder.rs`

- [ ] **Step 1: Add fields to `ResultRecorder` for the new state**

Find `pub struct ResultRecorder { ... }`. Add three fields (anywhere in the existing field list):

```rust
    /// Captured allocation-site records. Empty when the dump was not
    /// captured under allocation tracking.
    allocation_sites: Vec<crate::parser::record::AllocationSite>,
    /// Sum of `total_bytes_allocated` over all `AllocationSites` records
    /// (records can be split across multiple HPROF tags).
    allocation_sites_record_count: u32,
```

- [ ] **Step 2: Initialize the new fields in `ResultRecorder::new`**

In the constructor, set both to defaults:

```rust
            allocation_sites: Vec::new(),
            allocation_sites_record_count: 0,
```

- [ ] **Step 3: Capture `AllocationSites` records in the recorder loop**

Find the `match record { ... }` block in `ResultRecorder::record_records` (or wherever records are dispatched — search for `Record::AllocationSites`). The current code likely just increments a counter and discards. Replace with:

```rust
                    Record::AllocationSites {
                        ref allocation_sites,
                        ..
                    } => {
                        self.allocation_sites_record_count += 1;
                        // Move the per-site list into the recorder so we
                        // don't pay for cloning. The record is owned by us.
                        self.allocation_sites
                            .extend(std::mem::take(allocation_sites.as_mut()).into_iter());
                        self.allocation_sites += 1;
                    }
```

(If the existing code already has a counter `self.allocation_sites: u32`, rename that field or merge — the new `allocation_sites: Vec` replaces it. The integer counter becomes `allocation_sites_record_count`.)

- [ ] **Step 4: Write the summary hint**

Find the `summary` field assembly in `RenderedResult` construction (search for `summary` in result_recorder.rs). Append the AllocationSites line:

```rust
let alloc_sites_line = if self.allocation_sites.is_empty() {
    "AllocationSites: not present (capture with `am profile start <pid>`)".to_string()
} else {
    format!(
        "AllocationSites: {} sites across {} records (run with --allocation-sites for stack traces)",
        self.allocation_sites.len(),
        self.allocation_sites_record_count
    )
};
```

Insert it into the rendered summary string just after the existing "Heap dumps containing in total ..." line. The exact location depends on where the summary is built — find the relevant `writeln!(summary, ...)` call and add a sibling.

- [ ] **Step 5: Expose `allocation_sites` on `RenderedResult`**

Add to `RenderedResult` (in `src/rendered_result.rs`):

```rust
pub struct RenderedResult {
    ...
    pub allocation_sites: Vec<crate::parser::record::AllocationSite>,
    pub allocation_sites_record_count: u32,
    ...
}
```

In `ResultRecorder`'s consumer (look for `RenderedResult { ... }` construction), pass:

```rust
            allocation_sites: std::mem::take(&mut self.allocation_sites),
            allocation_sites_record_count: self.allocation_sites_record_count,
```

- [ ] **Step 6: Add a unit test asserting the summary hint behavior**

Add to `result_recorder::tests`:

```rust
    #[test]
    fn summary_hint_says_not_present_when_no_alloc_sites() {
        // Use the bundled JVM fixture which has no allocation tracking.
        let r = crate::slurp::slurp_file("test-heap-dumps/hprof-64.bin", false, false).unwrap();
        let s = r.serialize(20);
        assert!(
            s.contains("AllocationSites: not present"),
            "expected hint, got:\n{s}"
        );
        assert!(r.allocation_sites.is_empty());
    }
```

- [ ] **Step 7: Run tests + clippy + fmt + commit**

```bash
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

git add src/result_recorder.rs src/rendered_result.rs
git commit -m "$(cat <<'EOF'
feat(recorder): capture AllocationSites + summary hint

ResultRecorder now retains the AllocationSite vec from the parser
(previously discarded) and emits a one-line summary hint:

  AllocationSites: 12,453 sites across 287 records (run with --allocation-sites for stack traces)

or, when the dump has no alloc-tracking data:

  AllocationSites: not present (capture with `am profile start <pid>`)

The hint always appears so users know whether `--allocation-sites` will
work without having to try it.

The captured Vec<AllocationSite> is exposed on RenderedResult for the
new --allocation-sites mode (next commit) to consume.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.2: Add `--allocation-sites` flag + `Mode::AllocationSites`

**Files:**
- Modify: `src/args.rs`, `src/errors.rs` (if needed)

- [ ] **Step 1: Add the flag and Mode variant**

In `src/args.rs::Cli`:

```rust
    /// Show per-class allocation sites with stack traces. Requires the dump
    /// to have been captured with allocation tracking enabled (Android:
    /// `am profile start <pid>`).
    #[arg(long = "allocation-sites", default_value_t = false)]
    pub allocation_sites: bool,
```

In `Mode`, add a new variant:

```rust
    AllocationSites {
        input_file: String,
        top: usize,
        debug: bool,
        json: bool,
    },
```

In `resolve()`, the modes-set count:

```rust
    let mode_count = [referrers_set, paths_set, diff_set, cli.allocation_sites]
        .iter()
        .filter(|b| **b)
        .count();
```

And the dispatch:

```rust
    if cli.allocation_sites {
        return Ok(Mode::AllocationSites {
            input_file,
            top: cli.top,
            debug: cli.debug,
            json: cli.json,
        });
    }
```

(Place this before the `find-referrers` branch since they're mutually exclusive.)

- [ ] **Step 2: Add a CLI test**

```rust
    #[test]
    fn parses_allocation_sites_flag() {
        let cli = Cli::try_parse_from(["heaptrail", "-i", "x.hprof", "--allocation-sites"]).unwrap();
        assert!(cli.allocation_sites);
    }

    #[test]
    fn allocation_sites_conflicts_with_find_referrers() {
        let cli = Cli::try_parse_from([
            "heaptrail",
            "-i",
            "test-heap-dumps/hprof-64.bin",
            "--allocation-sites",
            "--find-referrers",
            "java.util.ArrayList",
        ])
        .unwrap();
        let err = resolve(cli).unwrap_err();
        match err {
            HprofSlurpError::ConflictingModes => {}
            other => panic!("expected ConflictingModes, got {other:?}"),
        }
    }
```

- [ ] **Step 3: Add the new error variant if needed**

If `errors.rs` doesn't already cover "no allocation sites in dump" — add it:

```rust
    #[error("no AllocationSites records in this dump (capture with `am profile start <pid>`)")]
    NoAllocationSites,
```

- [ ] **Step 4: Build + test + commit**

```bash
cargo test --release args
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all

git add src/args.rs src/errors.rs
git commit -m "feat(args): --allocation-sites flag + Mode::AllocationSites variant"
```

### Task 4.3: Implement `src/allocation_sites.rs` — render mode

**Files:**
- Create: `src/allocation_sites.rs`
- Modify: `src/main.rs` (mod declaration + dispatch)

- [ ] **Step 1: Create `src/allocation_sites.rs`**

```rust
//! `--allocation-sites` — print per-class top-N allocation sites with
//! their stack traces resolved to readable method/file/line references.
//!
//! Requires the dump to contain `AllocationSites` records (only present
//! when allocation tracking was enabled at capture time —
//! `am profile start <pid>` on Android).

use serde::Serialize;
use std::cmp::Reverse;

use crate::args::Mode;
use crate::errors::HprofSlurpError;
use crate::parser::record::AllocationSite;
use crate::referrer::{Pass1Index, ResolvedFrame, pass1_index};
use crate::rendered_result::RenderedResult;
use crate::slurp::slurp_file;

#[derive(Serialize, Debug, Clone)]
pub struct ResolvedAllocSite {
    pub class_name: String,
    pub bytes_allocated: u32,
    pub instances_allocated: u32,
    pub bytes_alive: u32,
    pub instances_alive: u32,
    pub stack_trace: Vec<ResolvedFrame>,
}

#[derive(Serialize, Debug)]
pub struct AllocationSitesResult {
    pub total_sites: usize,
    pub top: Vec<ResolvedAllocSite>,
}

pub fn run(mode: &Mode) -> Result<AllocationSitesResult, HprofSlurpError> {
    let (input_file, top, debug) = match mode {
        Mode::AllocationSites {
            input_file,
            top,
            debug,
            ..
        } => (input_file.as_str(), *top, *debug),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "allocation_sites::run only handles Mode::AllocationSites",
            });
        }
    };

    // Slurp the file once for the AllocationSite list (lives on RenderedResult)
    // and again for the Pass1Index (needed for class+frame resolution).
    let rendered: RenderedResult = slurp_file(input_file, debug, false)?;
    if rendered.allocation_sites.is_empty() {
        return Err(HprofSlurpError::NoAllocationSites);
    }
    let idx = pass1_index(input_file, debug)?;

    let mut sites: Vec<AllocationSite> = rendered.allocation_sites;
    sites.sort_by_key(|s| Reverse(s.bytes_allocated));

    let resolved: Vec<ResolvedAllocSite> = sites
        .iter()
        .take(top)
        .map(|s| {
            let class_name = idx
                .class_name_id_by_serial
                .get(&s.class_serial_number)
                .and_then(|nid| idx.utf8_by_id.get(nid))
                .map(|raw| raw.as_ref().replace('/', "."))
                .unwrap_or_else(|| format!("(class_serial={})", s.class_serial_number));

            let stack_trace = idx
                .stack_trace_by_serial
                .get(&s.stack_trace_serial_number)
                .map(|frame_ids| {
                    frame_ids
                        .iter()
                        .filter_map(|&fid| idx.resolve_frame(fid))
                        .collect()
                })
                .unwrap_or_default();

            ResolvedAllocSite {
                class_name,
                bytes_allocated: s.bytes_allocated,
                instances_allocated: s.instances_allocated,
                bytes_alive: s.bytes_alive,
                instances_alive: s.instances_alive,
                stack_trace,
            }
        })
        .collect();

    Ok(AllocationSitesResult {
        total_sites: sites.len(),
        top: resolved,
    })
}

pub fn render_text(r: &AllocationSitesResult) -> String {
    use crate::utils::pretty_bytes_size;
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\nTop {} allocation sites by bytes_allocated (of {} total):\n",
        r.top.len(),
        r.total_sites
    );
    for s in &r.top {
        let bytes = pretty_bytes_size(u64::from(s.bytes_allocated));
        let _ = writeln!(
            out,
            "  ─ {:>10}  /  {:>10} instances  {}#<init>",
            bytes, s.instances_allocated, s.class_name
        );
        for f in &s.stack_trace {
            let qualified = match &f.class {
                Some(c) => format!("{c}.{}", f.method),
                None => f.method.clone(),
            };
            let location = match (&f.file, f.line) {
                (Some(file), n) if n > 0 => format!("({file}:{n})"),
                (Some(file), _) => format!("({file})"),
                _ => String::new(),
            };
            let _ = writeln!(out, "        at {qualified}{location}");
        }
        let _ = writeln!(out);
    }
    out
}
```

- [ ] **Step 2: Wire it into `main.rs`**

Add to the top of `main.rs`:

```rust
mod allocation_sites;
```

In the `Mode` dispatch in `main_result()`:

```rust
        mode @ Mode::AllocationSites { .. } => run_allocation_sites(mode, now),
```

Add the handler:

```rust
fn run_allocation_sites(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let json = match &mode {
        Mode::AllocationSites { json, .. } => *json,
        _ => unreachable!(),
    };
    let result = allocation_sites::run(&mode)?;
    if json {
        let path = format!(
            "heaptrail-allocation-sites-{}.json",
            chrono::Utc::now().timestamp_millis()
        );
        let f = std::fs::File::create(&path)?;
        serde_json::to_writer(std::io::BufWriter::new(f), &result)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", allocation_sites::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}
```

- [ ] **Step 3: Add a unit test for the resolution + render**

Add a test inside `src/allocation_sites.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_when_dump_has_no_alloc_sites() {
        let mode = Mode::AllocationSites {
            input_file: "test-heap-dumps/hprof-64.bin".to_string(),
            top: 10,
            debug: false,
            json: false,
        };
        match run(&mode) {
            Err(HprofSlurpError::NoAllocationSites) => {}
            other => panic!("expected NoAllocationSites, got {other:?}"),
        }
    }
}
```

- [ ] **Step 4: Run tests + smoke + commit**

```bash
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check

# Smoke tests
./target/release/heaptrail -i test-heap-dumps/hprof-64.bin --allocation-sites 2>&1 | tail -5
# Expected: error: no AllocationSites records in this dump (capture with `am profile start <pid>`)

./target/release/heaptrail -i /tmp/heap-snapshot-fix.hprof -t 5 2>&1 | grep AllocationSites
# Expected: AllocationSites: not present  OR  AllocationSites: N sites across M records

git add src/allocation_sites.rs src/main.rs
git commit -m "$(cat <<'EOF'
feat: --allocation-sites mode prints class+stack-trace allocation report

New mode reads the AllocationSite vec captured by the recorder, resolves
each site's class_serial -> class name and stack_trace_serial -> frames
via Pass1Index, sorts by bytes_allocated, and prints the top N with
qualified method names + file:line.

JSON sidecar: heaptrail-allocation-sites-<ts>.json with
[{class_name, bytes_allocated, instances_allocated, stack_trace: [...]}]

Errors with NoAllocationSites when the dump has no alloc-tracking
records (the standard capture path: `am profile start <pid>` before
`am dumpheap`).

Closes feature C of the v0.8.0 spec.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4.4: Push PR 4

- [ ] **Step 1: Push + watch CI**

```bash
git push fork master
gh run watch --repo johnneerdael/heaptrail \
  $(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json databaseId -q '.[0].databaseId') \
  --exit-status
```

Expected: green.

---

## PR 5 — Docs + version bump + release

**Goal:** Land version 0.8.0, refresh docs, tag, release.

**Pull request title:** `chore: bump to 0.8.0; document v0.8.0 features (A, C, D, F)`

### Task 5.1: Bump `Cargo.toml` to 0.8.0

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Edit `Cargo.toml`**

Change:

```toml
version = "0.7.1"
```

to:

```toml
version = "0.8.0"
```

- [ ] **Step 2: Build (regenerates `Cargo.lock`)**

Run: `cargo build --release`
Expected: clean, `heaptrail v0.8.0` in the compile line.

### Task 5.2: Update `README.md`

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the new modes/flags to the cheat sheet**

Find the "Beyond the summary" section and the per-mode subsections. After the existing `### \`--diff-from\` / \`--diff-to\`` subsection, add:

```markdown
### `--target-glob` — pattern targeting

```bash
heaptrail -i my.hprof --target-glob 'com.example.**' --hops 2
```

Glob matches against dotted FQ class names (`*` within a package level,
`**` across levels, `?` single char, `[abc]` class). Mutually exclusive
with `--find-referrers`. Details in
[USERGUIDE §F](USERGUIDE.md#f---target-glob--pattern-targeting).

### `--allocation-sites` — per-class stack traces

```bash
heaptrail -i my.hprof --allocation-sites --top 20
```

Requires the dump to have been captured with allocation tracking
(`am profile start <pid>` on Android). Prints the top-N allocation
sites with their stack traces. Summary always reports whether the dump
has alloc-tracking data. Details in
[USERGUIDE §C](USERGUIDE.md#c---allocation-sites--per-class-stack-traces).
```

- [ ] **Step 2: Add a one-line note about thread/frame surfacing in paths-from-id**

In the existing `### \`--paths-from-id\`` subsection, add at the end:

```markdown
When a chain terminates at a thread-owned root (`RootJavaFrame`,
`RootThreadObject`, `RootJniLocal`, `RootJniMonitor`), the output
includes the thread name + top frame method/file/line.
```

### Task 5.3: Update `USERGUIDE.md`

**Files:**
- Modify: `USERGUIDE.md`

- [ ] **Step 1: Add new sections F, C, D**

Insert after the existing §6 (`--diff-from` / `--diff-to`):

```markdown
## F — `--target-glob` — pattern targeting

`--find-referrers` accepts an exact FQ class name. When you want to
target a *family* of classes — every model class in a package, every
inner iterator class — use `--target-glob` instead.

```bash
# All MetaPreview-related model classes
heaptrail -i heap.hprof --target-glob 'com.nexio.tv.domain.model.*' --hops 2

# Every Iterator inner class anywhere
heaptrail -i heap.hprof --target-glob '**$Itr'

# Match a single character
heaptrail -i heap.hprof --target-glob 'com.example.User?'
```

Glob syntax matches dotted FQ class names:

| Pattern | Meaning |
|---------|---------|
| `*` | one package level (no `.`) |
| `**` | zero or more levels (crosses `.`) |
| `?` | exactly one character |
| `[abc]` | one of the listed characters |

Output prepends a "matched classes" header listing each class with its
live instance count, sorted by count desc:

```
Found 4 classes matching glob 'com.nexio.tv.domain.model.*':
  - com.nexio.tv.domain.model.MetaPreview         (123,382 instances)
  - com.nexio.tv.domain.model.CatalogRow          (28,697 instances)
  ...
```

Mutually exclusive with `--find-referrers <name>`; passing both is a
CLI error.

## C — `--allocation-sites` — per-class stack traces

When a heap dump is captured *with allocation tracking* enabled, the
hprof contains stack frames for every allocation. This is the most
direct path from "this class is huge" to "this is the line that
allocated it."

### Capturing an alloc-tracked dump

```bash
adb shell am profile start <pid>          # turn on alloc tracking
# (run the suspect interaction)
adb shell am dumpheap <pid> /sdcard/heap.hprof
adb shell am profile stop <pid>            # turn off
adb pull /sdcard/heap.hprof
```

### Running the report

```bash
heaptrail -i heap.hprof --allocation-sites --top 20
```

Output:

```
Top 20 allocation sites by bytes_allocated (of 12,453 total):

  ─ 1.21 GiB  /  4,812,000 instances  com.nexio.tv.domain.model.MetaPreview#<init>
        at com.squareup.moshi.adapters.ClassJsonAdapter.fromJson(ClassJsonAdapter.java:128)
        at com.squareup.moshi.JsonAdapter$1.fromJson(JsonAdapter.java:194)
        at com.nexio.tv.network.HomeRepository.fetchCatalog(HomeRepository.kt:87)
        ...
```

Each entry shows total bytes allocated by this site (across the dump's
lifetime), instances allocated, and the resolved Java stack trace from
the top frame down.

### When the dump has no alloc data

`heaptrail summary` reports it explicitly:

```
AllocationSites: not present (capture with `am profile start <pid>`)
```

Running `--allocation-sites` on a non-tracked dump exits with the same
hint as an error, so scripts know to fall back.

## D — Object[] indices in `--paths-from-id`

When a path hop passes through an `Object[]`, the output now includes
the matched element index:

```
  hop 5  ── id=518041528  (via java.lang.Object[][12])
```

Useful when an `ArrayList.elementData` sits between you and the
target — you can correlate index 12 back to a known position in the
collection (e.g., a paged result's 13th entry).
```

- [ ] **Step 2: Add an "A — thread/frame on terminator" note inside §5 (`--paths-from-id`)**

After the example output block, add a subsection:

```markdown
### Thread name + top frame on thread-owned roots

When the chain terminates at one of:

- `RootJavaFrame` — a Java stack frame holds the object
- `RootThreadObject` — the chain reached the `Thread` itself
- `RootJniLocal` / `RootJniMonitor` — JNI references

heaptrail prints the thread name and (for `RootJavaFrame`) the top
frame's method/file/line:

```
  → reached GC root: RootJavaFrame
        thread "pool-7-thread-2"
        at android.app.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)
```

When the dump's `StartThread` / `StackTrace` records are missing, the
gap is reported explicitly:

```
  → reached GC root: RootJavaFrame
        (thread metadata not in dump)
```
```

### Task 5.4: Update plugin SKILL.md

**Files:**
- Modify: `plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md`

- [ ] **Step 1: Add v0.8.0 features to the operating-modes section**

Find "## The four operating modes" and rename to "## The five operating modes" (we now have 5: summary, find-referrers, paths-from-id, diff-from, allocation-sites).

After the existing `### 4. \`--diff-from <a> --diff-to <b>\`` subsection, add:

```markdown
### 5. `--allocation-sites` — per-class allocation stack traces

```bash
heaptrail -i heap.hprof --allocation-sites --top 20
```

**What it tells you:** For dumps captured with allocation tracking
(`am profile start <pid>`), prints the top-N allocation sites with their
resolved Java stack traces. The most direct path from "this class is
huge" to "this is the line that allocated it."

When the dump has no alloc-tracking data, exits with a hint pointing
at `am profile start`. Summary mode always reports presence/absence
of the data even when running without `--allocation-sites`.

**Wall time:** ~150 ms on a 235 MiB dump (same as summary; the data
is loaded by the same slurp pass).
```

Also update the "When to use" cheat sheet table:

```markdown
| Compare two snapshots | `heaptrail --diff-from a.hprof --diff-to b.hprof` |
| Per-class allocation sites + stack traces | `heaptrail -i heap.hprof --allocation-sites` |
| Pattern targeting (glob) | `heaptrail -i heap.hprof --target-glob 'com.foo.**'` |
| JSON sidecar | append `--json` to any of the above |
```

- [ ] **Step 2: Update the "Standard triage workflow" to reference allocation-sites**

In the workflow numbered list, add a new step 6:

```markdown
6. (Optional) **`--allocation-sites`** when the dump was captured under
   `am profile start`. The most direct path from "this class is huge"
   to "this is the line that allocated it."
```

- [ ] **Step 3: Bump version reference**

Find any `version 0.7.0+` or `0.7.1+` lines in SKILL.md and bump to `0.8.0+`.

### Task 5.5: Commit, tag, release

- [ ] **Step 1: Run final checks**

```bash
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
```

Expected: green across the board.

- [ ] **Step 2: Commit + push**

```bash
git add Cargo.toml Cargo.lock README.md USERGUIDE.md \
        plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md
git commit -m "$(cat <<'EOF'
chore: bump to 0.8.0; document v0.8.0 features

  * Cargo.toml: 0.7.1 -> 0.8.0 (minor; new flags, no breaking changes)
  * README.md: cheat-sheet entries for --allocation-sites and --target-glob
  * USERGUIDE.md: new sections C (allocation sites), F (glob), and D
    (Object[] indices); appended thread/frame description to §5
  * SKILL.md: fifth operating mode added; workflow updated; version bump

Closes the v0.8.0 spec
(docs/superpowers/specs/2026-05-09-heaptrail-v0.8-design.md). Features
A, C, D, F all landed across PRs 1-4.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"

git push fork master
gh run watch --repo johnneerdael/heaptrail \
  $(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json databaseId -q '.[0].databaseId') \
  --exit-status
```

- [ ] **Step 3: Tag + release**

```bash
git tag -a v0.8.0 -m "v0.8.0 — thread/stack metadata + allocation sites + glob targeting + path indices"
git push fork v0.8.0

cat > /tmp/release-notes-080.md <<'NOTES'
## v0.8.0 — Thread metadata, allocation sites, glob targeting

Four metadata-surfacing wins, no parser-mode changes, fully
backwards-compatible.

### A — Thread name + top frame on thread-owned roots

When `--paths-from-id` terminates at `RootJavaFrame`, `RootThreadObject`,
`RootJniLocal`, or `RootJniMonitor`, the output now includes:

```
  → reached GC root: RootJavaFrame
        thread "pool-7-thread-2"
        at android.app.SharedPreferencesImpl$EditorImpl.commitToMemory(SharedPreferencesImpl.java:478)
```

### C — `--allocation-sites` mode

For dumps captured with allocation tracking (`am profile start <pid>`),
heaptrail can now print the top-N allocation sites with resolved Java
stack traces. Summary always reports whether the data is present.

### D — `Object[]` element index in path hops

`--paths-from-id` array hops show `via java.lang.Object[][12]` instead of
the previous unhelpful `via java.lang.Object[][]`.

### F — `--target-glob` pattern targeting

A new top-level flag accepts shell-style globs against dotted FQ class
names, with output listing every matched class:

```
heaptrail -i my.hprof --target-glob 'com.example.**' --hops 2
```

`*` stays within a package level; `**` crosses levels; `?` matches one
character; `[abc]` is a character class.

### Compatibility

- Every existing CLI invocation produces byte-identical output unless
  the dump itself contains the relevant metadata.
- Existing JSON output gains optional fields; no fields removed.
- No new parser modes, no new tags, no new dependencies beyond `globset`.

### Roadmap

- v0.9.0 — feature B (`--preview-bytes` content preview)
- v1.0.0 — feature E (full Lengauer–Tarjan dominator tree)

Both have their own design specs in `docs/superpowers/specs/`.
NOTES

gh release create v0.8.0 --repo johnneerdael/heaptrail \
  --title "heaptrail v0.8.0" -F /tmp/release-notes-080.md
```

Expected: release URL printed; release workflow auto-runs and uploads binaries + publishes 0.8.0 to crates.io (same idempotent flow used in v0.7.1).

- [ ] **Step 4: Watch the release workflow**

```bash
sleep 5
gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1
gh run watch --repo johnneerdael/heaptrail \
  $(gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1 --json databaseId -q '.[0].databaseId') \
  --exit-status
```

Expected: all 6 binary uploads succeed; crates.io publish succeeds (new version number, no idempotent skip).

- [ ] **Step 5: Verify release contents**

```bash
gh release view v0.8.0 --repo johnneerdael/heaptrail --json assets -q '.assets[].name'
```

Expected output (in any order):

```
heaptrail-aarch64-apple-darwin.tar.gz
heaptrail-aarch64-pc-windows-msvc.zip
heaptrail-aarch64-unknown-linux-gnu.tar.gz
heaptrail-x86_64-apple-darwin.tar.gz
heaptrail-x86_64-pc-windows-msvc.zip
heaptrail-x86_64-unknown-linux-gnu.tar.gz
```

And:

```bash
curl -sf https://crates.io/api/v1/crates/heaptrail/0.8.0 | python3 -m json.tool | head
```

Expected: returns the metadata for v0.8.0 (HTTP 200).

---

## Self-Review Checklist (run before declaring the plan ready to execute)

- [ ] **Spec coverage:** every requirement in `docs/superpowers/specs/2026-05-09-heaptrail-v0.8-design.md` has a corresponding task.
  - §3.A → Tasks 1.1–1.4
  - §3.C → Tasks 4.1–4.3
  - §3.D → Task 2.1
  - §3.F → Tasks 3.1–3.3
  - §4 (testing) → tests embedded in each task; alloc-tracked fixture acquisition optional (we tolerate 'not present' on existing fixtures and use `NoAllocationSites` error to validate the empty path).
  - §5 (rollout) → Task 5.5.
- [ ] **No placeholders:** every step shows code, exact commands, or both. No "TBD" or "implement later".
- [ ] **Type consistency:** `ResolvedFrame`, `ThreadFrameRef`, `MatchedClass`, `ReferrersTarget`, `Mode::AllocationSites`, `AllocationSitesResult`, `ResolvedAllocSite` defined in exactly one place each, referenced by the same name throughout. `PathStep.array_index: Option<u32>` consistent across declaration, capture, and render. `Pass1Index.thread_name_by_serial`, `class_name_id_by_serial`, etc., consistent in every reference.
- [ ] **Existing tests stay green:** every PR's last step runs `cargo test --release` + clippy + fmt before commit.

## Risk Notes

- **`globset` binary size**: if PR 3 step 2 finds the binary grew >150 KB, fall back to a hand-rolled glob matcher (~30 LOC for `*`/`**`/`?`/`[abc]`). The plan's `resolve_target_ids` glob path uses `globset`'s `is_match`; the fallback would replace it with a custom function having the same signature.
- **Alloc-tracked fixture**: PR 4's tests cover the empty-data path (`NoAllocationSites` error). Full end-to-end of stack-trace resolution is covered by unit tests with synthetic AllocationSite + StackTrace + StackFrame + LoadClass + Utf8 records. Add a real alloc-tracked fixture in a follow-up if/when one is captured; for v0.8.0, synthetic + empty-path coverage is sufficient.
- **Existing ResultRecorder field rename**: Task 4.1 step 3 may collide with an existing `self.allocation_sites: u32` counter. Search for that field first; if present, rename to `allocation_sites_record_count` and use the new `Vec<AllocationSite>` field name `allocation_sites` for the new state.
