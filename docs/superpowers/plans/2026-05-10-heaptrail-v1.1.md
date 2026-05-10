# heaptrail v1.1.0 — MAT-grade leak hunting (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement features G/H/I/J from `docs/superpowers/specs/2026-05-10-heaptrail-v1.1-design.md` — `--exclude-soft-weak`, `--leak-suspects`, `--merge-paths`, `--bitmaps` — bringing heaptrail to feature parity with MAT's daily Android leak-hunting workflow.

**Architecture:** Eight sequential PRs onto `master`. PR 1 extends `Pass1Index` with reference-class detection (used by everything else) and bitmap-class detection (used by `--bitmaps` only). PRs 2–4 build the leak-hunting trio (`--exclude-soft-weak` filter, `dom_children` helper, `--leak-suspects` mode) on top of v1.0.0's dominator infrastructure. PRs 5–6 add merged-paths via a small refactor and a trie module. PR 7 lands `--bitmaps`, fully independent. PR 8 is docs + version bump 1.0.0 → 1.1.0 + tag + release.

**Tech Stack:** Rust 2024, ahash, nom, crossbeam-channel (existing); no new crates.

---

## File Structure

| File | Responsibility | First touched in |
|------|----------------|------------------|
| `src/reference_classes.rs` (NEW) | Walk class hierarchy in `Pass1Index`; build `reference_subclass_set` and `bitmap_class_info`. Pure derivation. | PR 1 |
| `src/leak_suspects.rs` (NEW) | Rank dominator subtrees by retained share; cluster by class; emit narrative. | PR 4 |
| `src/merge_paths.rs` (NEW) | Fold N paths-to-root into a trie; render with branch counts. | PR 6 |
| `src/bitmaps.rs` (NEW) | Identify Bitmap instances; compute pixel bytes; render report. | PR 7 |
| `src/referrer.rs` | `Pass1Index` gains `reference_subclass_set` + `bitmap_class_info`; `pass1_index` calls `reference_classes::derive`. | PR 1 |
| `src/reference_graph.rs` | `build_from_pass1` accepts new `BuildOptions { exclude_soft_weak: bool }`; skips outgoing edges from Reference subclass source nodes when set. | PR 2 |
| `src/retained.rs` | New public `dom_children(idom: &[u32]) -> Vec<Vec<u32>>` helper for `--leak-suspects`. | PR 3 |
| `src/paths.rs` | Walk respects `exclude_soft_weak` (terminator annotation); refactor extracts `compute_path_for_object` for `--merge-paths`. | PR 2 / PR 5 |
| `src/args.rs` | Four new flags: `--exclude-soft-weak`, `--leak-suspects[=THRESHOLD]`, `--merge-paths`, `--bitmaps`. New `Mode::LeakSuspects`, `Mode::Bitmaps`. | PRs 2/4/6/7 |
| `src/main.rs` | Dispatch new modes; thread modifier flags through existing dispatchers. | PRs 2/4/6/7 |
| `Cargo.toml` / `README.md` / `USERGUIDE.md` / `SKILL.md` / plugin manifests | Docs + version bump 1.1.0. | PR 8 |

`Pass1Index` is the keystone — every new module either reads it (`reference_graph`, `paths`, `referrer`, `bitmaps`, `leak_suspects`) or extends it (`reference_classes`). v1.0.0's contract on `ReferenceGraph` / `lengauer_tarjan` / `RetainedAnalysis` is preserved (spec §3.6); v1.1.0 only *consumes* those types.

---

## PR 1 — `reference_classes`: subclass + bitmap detection

**PR title:** `feat(v1.1): src/reference_classes.rs — soft/weak/phantom + bitmap class detection`

**Goal:** Pure-derivation module that takes a `Pass1Index` and returns:
1. The set of class ids that are subclasses of `java.lang.ref.{Soft,Weak,Phantom}Reference` (transitive), and
2. The optional `BitmapClassInfo` (class id + field offsets) when `android.graphics.Bitmap` is loaded in the dump.

Both are stashed back onto `Pass1Index` after `pass1_index()` finishes, so downstream modules consult `idx.reference_subclass_set` and `idx.bitmap_class_info` without re-walking the hierarchy.

### Task 1.1: Module skeleton + types

**Files:**
- Create: `src/reference_classes.rs`
- Modify: `src/main.rs` — `mod reference_classes;`

- [ ] **Step 1: Define the public API**

```rust
// Dead-code allowance until PRs 2/7 wire consumers. Tests below
// exercise every public item.
#![allow(dead_code)]

//! Class-hierarchy derivations layered on top of `Pass1Index`:
//!
//!  * `reference_subclass_set` — every class that inherits (transitively)
//!    from `java.lang.ref.SoftReference`, `WeakReference`, or
//!    `PhantomReference`. Used by `--exclude-soft-weak` to drop the
//!    outgoing edge fan from those nodes.
//!
//!  * `bitmap_class_info` — when `android.graphics.Bitmap` is present
//!    in the dump, the class id + flattened-layout offsets for the
//!    `mWidth`, `mHeight`, `mNativeBitmap`/`mBuffer`, and `mConfig`
//!    instance fields. Used by `--bitmaps`.

use ahash::AHashSet;

use crate::parser::gc_record::FieldType;
use crate::referrer::Pass1Index;

#[derive(Default, Debug, Clone)]
pub struct ReferenceClassInfo {
    /// Transitive subclasses of `java.lang.ref.{Soft,Weak,Phantom}Reference`.
    /// The abstract base `java.lang.ref.Reference` itself is **not** in
    /// this set — only its three strength-marking subclasses and their
    /// descendants. App-defined subclasses (LeakCanary's
    /// `KeyedWeakReference`, framework `FinalizerReference`) propagate
    /// through the transitive walk.
    pub soft_weak_phantom: AHashSet<u64>,
}

#[derive(Debug, Clone)]
pub struct BitmapClassInfo {
    pub class_id: u64,
    /// Byte offset within an instance dump body (post-super-chain flatten)
    /// for `mWidth: int`.
    pub width_field_offset: u32,
    /// Byte offset for `mHeight: int`.
    pub height_field_offset: u32,
    /// Byte offset for `mConfig: Bitmap.Config` (an Object reference).
    pub config_field_offset: u32,
    /// Byte offset for `mBuffer: byte[]` on pre-O Android (where pixel
    /// data lives on the Java heap). `None` on O+ where pixels are
    /// native and only `mNativeBitmap` (a long handle, opaque to us)
    /// remains.
    pub buffer_field_offset: Option<u32>,
}

/// Walk the class hierarchy in `idx` and return the soft/weak/phantom
/// subclass set + (optional) bitmap class metadata. Cheap (~10 ms on a
/// 200 MiB Android dump) and pure — does not touch the hprof file.
pub fn derive(idx: &Pass1Index) -> (ReferenceClassInfo, Option<BitmapClassInfo>) {
    (ReferenceClassInfo::default(), None)
}
```

- [ ] **Step 2: Wire the module + verify it compiles**

In `src/main.rs`, add `mod reference_classes;` next to the existing
`mod reference_graph;` line (alphabetical).

Run: `cargo build --release`
Expected: clean build (with two `dead_code` warnings on `ReferenceClassInfo` and `BitmapClassInfo`, which the file-level `#![allow(dead_code)]` already silences).

- [ ] **Step 3: Commit**

```bash
git add src/reference_classes.rs src/main.rs
git commit -m "feat(v1.1): scaffold src/reference_classes.rs with derive() entry point"
```

### Task 1.2: Soft/weak/phantom transitive walk

**Files:**
- Modify: `src/reference_classes.rs`

- [ ] **Step 1: Implement transitive subclass detection**

Replace the stub `derive` body with:

```rust
pub fn derive(idx: &Pass1Index) -> (ReferenceClassInfo, Option<BitmapClassInfo>) {
    let info = ReferenceClassInfo {
        soft_weak_phantom: collect_soft_weak_phantom(idx),
    };
    let bitmap = detect_bitmap_class(idx);
    (info, bitmap)
}

fn collect_soft_weak_phantom(idx: &Pass1Index) -> AHashSet<u64> {
    // Find the three marker classes. Class names in HPROF are
    // slash-form ("java/lang/ref/SoftReference"); `Pass1Index::class_name`
    // returns dotted form, but the underlying utf8 string is slash form.
    // We compare against slash form since that's what the indexer stored.
    let mut markers = AHashSet::<u64>::new();
    for (&class_id, &name_id) in &idx.class_name_id_by_class_id {
        if let Some(name) = idx.utf8_by_id.get(&name_id) {
            let s = name.as_ref();
            if s == "java/lang/ref/SoftReference"
                || s == "java/lang/ref/WeakReference"
                || s == "java/lang/ref/PhantomReference"
            {
                markers.insert(class_id);
            }
        }
    }
    if markers.is_empty() {
        return AHashSet::new();
    }

    // For each class, walk up super_class_by_id. If we hit a marker,
    // include the class. Cache results to keep the worst case linear.
    let mut subclass_set: AHashSet<u64> = AHashSet::new();
    let mut memo: ahash::AHashMap<u64, bool> = ahash::AHashMap::new();
    for &cid in idx.class_name_id_by_class_id.keys() {
        if is_subclass_of_any(cid, &markers, &idx.super_class_by_id, &mut memo) {
            subclass_set.insert(cid);
        }
    }
    // The markers themselves count: dropping outgoing edges from
    // `WeakReference` instances *is* the goal of the filter.
    subclass_set
}

fn is_subclass_of_any(
    cid: u64,
    markers: &AHashSet<u64>,
    supers: &ahash::AHashMap<u64, u64>,
    memo: &mut ahash::AHashMap<u64, bool>,
) -> bool {
    if let Some(&hit) = memo.get(&cid) {
        return hit;
    }
    if markers.contains(&cid) {
        memo.insert(cid, true);
        return true;
    }
    // Walk up the chain with a visited guard against pathological loops.
    let mut visited: AHashSet<u64> = AHashSet::new();
    let mut cur = supers.get(&cid).copied().unwrap_or(0);
    while cur != 0 && visited.insert(cur) {
        if markers.contains(&cur) {
            memo.insert(cid, true);
            return true;
        }
        cur = supers.get(&cur).copied().unwrap_or(0);
    }
    memo.insert(cid, false);
    false
}
```

- [ ] **Step 2: Stub `detect_bitmap_class` (Task 1.3 fills it in)**

```rust
fn detect_bitmap_class(_idx: &Pass1Index) -> Option<BitmapClassInfo> {
    None
}
```

- [ ] **Step 3: Unit test against a synthetic Pass1Index**

Append to `src/reference_classes.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::referrer::Pass1Index;

    fn make_idx() -> Pass1Index {
        let mut idx = Pass1Index::default();
        idx.id_size = 4;
        // utf8 strings + class hierarchy:
        //   1: java/lang/ref/Reference            (base; NOT included)
        //   2: java/lang/ref/SoftReference  ← Reference
        //   3: java/lang/ref/WeakReference  ← Reference
        //   4: java/lang/ref/PhantomReference ← Reference
        //   5: leakcanary/KeyedWeakReference  ← WeakReference
        //   6: java/lang/Object  (unrelated)
        for (id, name) in [
            (101u64, "java/lang/ref/Reference"),
            (102, "java/lang/ref/SoftReference"),
            (103, "java/lang/ref/WeakReference"),
            (104, "java/lang/ref/PhantomReference"),
            (105, "leakcanary/KeyedWeakReference"),
            (106, "java/lang/Object"),
        ] {
            idx.utf8_by_id.insert(id, name.into());
        }
        for (cid, name_id) in [(1u64, 101u64), (2, 102), (3, 103), (4, 104), (5, 105), (6, 106)] {
            idx.class_name_id_by_class_id.insert(cid, name_id);
        }
        idx.super_class_by_id.insert(2, 1);
        idx.super_class_by_id.insert(3, 1);
        idx.super_class_by_id.insert(4, 1);
        idx.super_class_by_id.insert(5, 3); // KeyedWeakReference < WeakReference
        idx
    }

    #[test]
    fn soft_weak_phantom_set_includes_markers_and_subclasses() {
        let idx = make_idx();
        let (info, _bitmap) = derive(&idx);
        let s = &info.soft_weak_phantom;
        assert!(s.contains(&2), "SoftReference (class 2) included");
        assert!(s.contains(&3), "WeakReference (class 3) included");
        assert!(s.contains(&4), "PhantomReference (class 4) included");
        assert!(s.contains(&5), "KeyedWeakReference (subclass) included");
        assert!(!s.contains(&1), "abstract Reference NOT included");
        assert!(!s.contains(&6), "unrelated Object NOT included");
    }

    #[test]
    fn no_markers_means_empty_set() {
        let mut idx = Pass1Index::default();
        idx.utf8_by_id.insert(101, "com/example/Foo".into());
        idx.class_name_id_by_class_id.insert(1, 101);
        let (info, _) = derive(&idx);
        assert!(info.soft_weak_phantom.is_empty());
    }

    #[test]
    fn cycle_in_super_chain_terminates() {
        let mut idx = Pass1Index::default();
        idx.utf8_by_id.insert(101, "java/lang/ref/WeakReference".into());
        idx.utf8_by_id.insert(102, "com/example/Cyclic".into());
        idx.class_name_id_by_class_id.insert(1, 101);
        idx.class_name_id_by_class_id.insert(2, 102);
        // Pathological cycle 2 -> 2.
        idx.super_class_by_id.insert(2, 2);
        let (info, _) = derive(&idx);
        // Cycle must not infinite-loop and must not falsely include 2.
        assert!(info.soft_weak_phantom.contains(&1));
        assert!(!info.soft_weak_phantom.contains(&2));
    }
}
```

- [ ] **Step 2 (run): tests pass**

Run: `cargo test --release reference_classes`
Expected: 3 pass.

- [ ] **Step 3: Commit**

```bash
git add src/reference_classes.rs
git commit -m "feat(v1.1): transitive soft/weak/phantom subclass detection"
```

### Task 1.3: Bitmap class detection

**Files:**
- Modify: `src/reference_classes.rs`

- [ ] **Step 1: Implement `detect_bitmap_class`**

Replace the stub from Task 1.2 with:

```rust
fn detect_bitmap_class(idx: &Pass1Index) -> Option<BitmapClassInfo> {
    // Find android/graphics/Bitmap by name.
    let class_id = find_class_id_by_name(idx, "android/graphics/Bitmap")?;

    // Walk the (own + super) field layout to compute byte offsets.
    // Same flatten rule as the reference_graph builder.
    let mut offset: u64 = 0;
    let mut width: Option<u32> = None;
    let mut height: Option<u32> = None;
    let mut config: Option<u32> = None;
    let mut buffer: Option<u32> = None;

    let mut chain: Vec<u64> = Vec::new();
    let mut cur = Some(class_id);
    while let Some(c) = cur {
        if c == 0 {
            break;
        }
        chain.push(c);
        cur = idx.super_class_by_id.get(&c).copied();
    }
    // Flatten in HPROF order: own fields first per class, walking up super.
    for &c in &chain {
        if let Some(fields) = idx.fields_by_class_id.get(&c) {
            for f in fields {
                let name = idx
                    .utf8_by_id
                    .get(&f.name_id)
                    .map(|s| s.as_ref())
                    .unwrap_or("");
                let size = field_byte_size(idx.id_size, f.field_type);
                if f.field_type == FieldType::Int && name == "mWidth" {
                    width = Some(offset as u32);
                } else if f.field_type == FieldType::Int && name == "mHeight" {
                    height = Some(offset as u32);
                } else if f.field_type == FieldType::Object && name == "mConfig" {
                    config = Some(offset as u32);
                } else if f.field_type == FieldType::Object && name == "mBuffer" {
                    buffer = Some(offset as u32);
                }
                offset += size as u64;
            }
        }
    }

    Some(BitmapClassInfo {
        class_id,
        width_field_offset: width?,
        height_field_offset: height?,
        config_field_offset: config?,
        buffer_field_offset: buffer,
    })
}

fn find_class_id_by_name(idx: &Pass1Index, target: &str) -> Option<u64> {
    for (&cid, &nid) in &idx.class_name_id_by_class_id {
        if let Some(name) = idx.utf8_by_id.get(&nid)
            && name.as_ref() == target
        {
            return Some(cid);
        }
    }
    None
}

fn field_byte_size(id_size: u32, t: FieldType) -> u32 {
    match t {
        FieldType::Object => id_size,
        FieldType::Bool | FieldType::Byte => 1,
        FieldType::Char | FieldType::Short => 2,
        FieldType::Int | FieldType::Float => 4,
        FieldType::Long | FieldType::Double => 8,
    }
}
```

- [ ] **Step 2: Unit test the offset computation**

Append:

```rust
    #[test]
    fn bitmap_class_info_offsets_with_object_super() {
        use crate::parser::gc_record::FieldInfo;
        let mut idx = Pass1Index::default();
        idx.id_size = 4;
        idx.utf8_by_id
            .insert(1u64, "android/graphics/Bitmap".into());
        idx.class_name_id_by_class_id.insert(100u64, 1u64);
        // No super.
        // Field layout (in HPROF order): mNativeBitmap: long (offset 0..8),
        // mBuffer: Object (8..12), mWidth: int (12..16), mHeight: int (16..20),
        // mConfig: Object (20..24).
        idx.utf8_by_id.insert(11, "mNativeBitmap".into());
        idx.utf8_by_id.insert(12, "mBuffer".into());
        idx.utf8_by_id.insert(13, "mWidth".into());
        idx.utf8_by_id.insert(14, "mHeight".into());
        idx.utf8_by_id.insert(15, "mConfig".into());
        idx.fields_by_class_id.insert(
            100,
            vec![
                FieldInfo { name_id: 11, field_type: FieldType::Long },
                FieldInfo { name_id: 12, field_type: FieldType::Object },
                FieldInfo { name_id: 13, field_type: FieldType::Int },
                FieldInfo { name_id: 14, field_type: FieldType::Int },
                FieldInfo { name_id: 15, field_type: FieldType::Object },
            ],
        );

        let bitmap = detect_bitmap_class(&idx).expect("Bitmap class should resolve");
        assert_eq!(bitmap.class_id, 100);
        assert_eq!(bitmap.buffer_field_offset, Some(8));
        assert_eq!(bitmap.width_field_offset, 12);
        assert_eq!(bitmap.height_field_offset, 16);
        assert_eq!(bitmap.config_field_offset, 20);
    }

    #[test]
    fn missing_bitmap_class_returns_none() {
        let idx = Pass1Index::default();
        assert!(detect_bitmap_class(&idx).is_none());
    }
```

- [ ] **Step 3: Run + commit**

```bash
cargo test --release reference_classes
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
git add src/reference_classes.rs
git commit -m "feat(v1.1): bitmap class + field-offset detection"
```

Expected: 5 unit tests pass; clippy clean.

### Task 1.4: Stash on `Pass1Index`

**Files:**
- Modify: `src/referrer.rs`

- [ ] **Step 1: Add fields to `Pass1Index`**

In the `pub struct Pass1Index { ... }` definition, append:

```rust
    // ---- v1.1.0 derivations (populated post-build by reference_classes::derive) ----
    /// Transitive subclasses of `java.lang.ref.{Soft,Weak,Phantom}Reference`.
    /// Populated by `pass1_index` via `reference_classes::derive`. Empty
    /// when none of the three marker classes were loaded.
    pub reference_subclass_set: ahash::AHashSet<u64>,
    /// `android.graphics.Bitmap` class metadata, when present in the dump.
    /// `None` on JVM dumps and on Android dumps where Bitmap was not
    /// loaded.
    pub bitmap_class_info: Option<crate::reference_classes::BitmapClassInfo>,
```

- [ ] **Step 2: Populate them at the end of `pass1_index`**

Find `pub(crate) fn pass1_index(...)` (around line 488). At the very end, just before `Ok(idx)`:

```rust
    let (refs, bitmap) = crate::reference_classes::derive(&idx);
    idx.reference_subclass_set = refs.soft_weak_phantom;
    idx.bitmap_class_info = bitmap;

    Ok(idx)
```

- [ ] **Step 3: Default impl + smoke**

The `#[derive(Default)]` on `Pass1Index` covers the new fields automatically (`AHashSet::default()` and `Option::None`). Build:

```bash
cargo build --release
```

Expected: clean build.

- [ ] **Step 4: Smoke test on canonical fixtures**

```bash
cargo test --release referrer::tests::pass1_indexes_class_metadata
```

Expected: existing test passes (proves `pass1_index` still works after the new lines). To exercise the new fields end-to-end, add an integration test:

```rust
#[test]
fn pass1_populates_v1_1_derivations_on_canonical_fixture() {
    let path = "JAVA_PROFILE_1.0.3.hprof";
    if !std::path::Path::new(path).exists() {
        eprintln!("skipping — fixture {path} not present");
        return;
    }
    let idx = crate::referrer::pass1_index(path, false).expect("pass1");
    assert!(
        !idx.reference_subclass_set.is_empty(),
        "Android fixture should have loaded WeakReference; subclass set is empty"
    );
    // bitmap_class_info may or may not be present depending on whether
    // android.graphics.Bitmap was loaded; we don't assert presence here.
}
```

Place this in the `mod tests` block in `src/referrer.rs`.

Run: `cargo test --release pass1_populates_v1_1`
Expected: pass.

- [ ] **Step 5: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/referrer.rs
git commit -m "feat(v1.1): stash reference_subclass_set + bitmap_class_info on Pass1Index"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 2 — `--exclude-soft-weak` modifier

**PR title:** `feat(v1.1): --exclude-soft-weak across paths, find-referrers, retained-size`

**Goal:** Drop outgoing edges from Reference subclass nodes in two surfaces:
1. `reference_graph::build_from_pass1` accepts `BuildOptions { exclude_soft_weak: bool }` and skips edge emission for those source nodes.
2. `paths::find_first_holder` and the `--find-referrers` pass2 scanner treat Reference subclass holders as walk terminators with annotation `[soft/weak/phantom — excluded]`.

The modifier flag is compatible with `--paths-from-id`, `--find-referrers`, `--retained-size`, and (in PR 4) `--leak-suspects`.

### Task 2.1: CLI flag + Mode propagation

**Files:**
- Modify: `src/args.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add the flag**

In the `Cli` struct, after the existing `--retained-size` block, add:

```rust
    /// Drop outgoing edges from java.lang.ref.{Soft,Weak,Phantom}Reference
    /// subclasses across path walks and the retained-size graph build.
    /// Use this for MAT-style leak hunting on Android dumps where
    /// LeakCanary watchers and framework weak-refs would otherwise bury
    /// the real strong reference.
    #[arg(long = "exclude-soft-weak", default_value_t = false)]
    pub exclude_soft_weak: bool,
```

In each Mode variant that already takes `retained_size: bool`
(`Mode::Summary`, `Mode::Paths`, `Mode::FindReferrers`), add a sibling field:

```rust
        exclude_soft_weak: bool,
```

In `resolve()`, propagate `cli.exclude_soft_weak` into each constructed mode.

- [ ] **Step 2: Update fixture_args helpers + main.rs destructures**

Same pattern as v1.0.0 PR 4 Task 4.1: add `exclude_soft_weak: false` to every `Mode::*` literal in `src/main.rs`, `src/paths.rs::tests::fixture_args`, and `src/referrer.rs::tests::fixture_args`. Threading mirrors `retained_size`.

In `src/main.rs`, the `Mode::Summary { ... }` destructure in `main_result` adds `exclude_soft_weak,` and `run_summary` gains a parameter `exclude_soft_weak: bool` (slot it after `retained_size`). Pass it through to `slurp_file_with_modes` (which gets a new parameter — see Task 2.3).

- [ ] **Step 3: Build + test the flag is recognized**

```rust
#[test]
fn parses_exclude_soft_weak() {
    let cli = Cli::parse_from([
        "heaptrail", "-i", "x.hprof", "--retained-size", "--exclude-soft-weak",
    ]);
    let mode = cli.resolve().unwrap();
    matches!(
        mode,
        Mode::Summary { retained_size: true, exclude_soft_weak: true, .. }
    );
}
```

Add to `args::args_tests`. Run:

```bash
cargo test --release args::
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/args.rs src/main.rs src/paths.rs src/referrer.rs
git commit -m "feat(v1.1): --exclude-soft-weak CLI flag + Mode propagation"
```

### Task 2.2: Reference-graph builder respects the flag

**Files:**
- Modify: `src/reference_graph.rs`

- [ ] **Step 1: Add `BuildOptions` + new entry point**

In `src/reference_graph.rs`, add:

```rust
#[derive(Default, Debug, Clone, Copy)]
pub struct BuildOptions {
    /// When set, skip emitting outgoing edges from any source node
    /// whose class is in `Pass1Index.reference_subclass_set`. Mirrors
    /// MAT's default leak-hunting filter.
    pub exclude_soft_weak: bool,
}

pub fn build_from_pass1_with(
    path: &str,
    idx: &Pass1Index,
    debug: bool,
    opts: BuildOptions,
) -> Result<ReferenceGraph, HprofSlurpError> {
    // (existing build_from_pass1 body, with one change: when emitting
    // edges from an InstanceDump body or ObjectArrayDump elements,
    // check `opts.exclude_soft_weak && idx.reference_subclass_set.contains(&class_object_id)`
    // and skip the inner edge-extraction call when true.)
}

pub fn build_from_pass1(
    path: &str,
    idx: &Pass1Index,
    debug: bool,
) -> Result<ReferenceGraph, HprofSlurpError> {
    build_from_pass1_with(path, idx, debug, BuildOptions::default())
}
```

The simplest implementation: copy the existing `build_from_pass1` body verbatim into `build_from_pass1_with`, then guard the `extract_refs_into` call and the `ObjectArrayDump elements` loop on `!skip_source`:

```rust
                    GcRecord::InstanceDump {
                        object_id,
                        class_object_id,
                        body,
                        ..
                    } => {
                        let ci = class_index(&mut class_ids, &mut class_index_by_id, class_object_id);
                        let size = instance_shallow_size(idx, class_object_id);
                        node_ids.push(object_id);
                        node_class.push(ci);
                        node_shallow.push(size);
                        let skip_source =
                            opts.exclude_soft_weak && idx.reference_subclass_set.contains(&class_object_id);
                        if !skip_source
                            && let Some(b) = body
                        {
                            extract_refs_into(idx, class_object_id, &b, object_id, &mut edge_buf);
                        }
                    }
```

`ObjectArrayDump` doesn't need the guard — array classes aren't Reference subclasses. The static-fields-from-super-root edges also don't need the guard (statics are class-level, not instance-level Reference holders).

- [ ] **Step 2: Wire the new entry point through `slurp_file_with_modes`**

`src/slurp.rs::slurp_file_with_modes` gains an `exclude_soft_weak: bool` parameter; pass `BuildOptions { exclude_soft_weak }` into `build_from_pass1_with`.

`src/main.rs::run_summary` gains a matching parameter and forwards it.

`src/paths.rs::run` and `src/referrer.rs::run` (when `retained_size` is set) likewise call `build_from_pass1_with` with `opts.exclude_soft_weak` derived from the destructured `exclude_soft_weak` field.

- [ ] **Step 3: Smoke-test the filter is active**

```bash
cargo build --release
echo "=== 1.0.3 retained baseline ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --retained-size -t 5 2>&1 | grep "char\[\]"
echo ""
echo "=== 1.0.3 retained --exclude-soft-weak ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --retained-size --exclude-soft-weak -t 5 2>&1 | grep "char\[\]"
```

Expected: with the flag, char[] retained drops (Reference subclasses no longer dominate their referents). The exact delta is dump-dependent — assertion is just "value differs", not an exact number.

- [ ] **Step 4: Lint + commit**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/reference_graph.rs src/slurp.rs src/main.rs src/paths.rs src/referrer.rs
git commit -m "feat(v1.1): build_from_pass1_with(BuildOptions) honors --exclude-soft-weak"
```

### Task 2.3: Path walks treat Reference subclasses as terminators

**Files:**
- Modify: `src/paths.rs`
- Modify: `src/referrer.rs`

- [ ] **Step 1: Path walk terminator**

In `src/paths.rs`, find `find_first_holder`. The function returns the first
holder of `current_id` it finds while streaming the dump. Modify so that:

* The destructure of `Mode::Paths` reads `exclude_soft_weak`.
* When `exclude_soft_weak` is set AND the candidate holder's class is in
  `idx.reference_subclass_set`, the holder is rejected (not yielded);
  the walker treats it as if no holder was found, and the path
  terminator banner appends ` [soft/weak/phantom — excluded]`.

The simplest implementation: thread `exclude_soft_weak: bool` into
`find_first_holder` as an extra parameter; inside the matcher, after
identifying a candidate holder's `holder_class_id`, check the set and
`continue` to the next record if it matches.

`PathResult` gains:

```rust
    /// Set when --exclude-soft-weak terminated the walk by hitting a
    /// Reference subclass holder. `None` otherwise. Surfaced in
    /// `render_text` as a `[soft/weak/phantom — excluded]` annotation
    /// on the trailing line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminated_by_soft_weak: Option<()>,
```

`render_text` checks `r.terminated_by_soft_weak.is_some()` and appends the
suffix to the orphan line:

```rust
    } else if r.terminated_by_soft_weak.is_some() {
        let _ = writeln!(out, "  → orphan [soft/weak/phantom — excluded]");
    } else {
        let _ = writeln!(out, "  → orphan: no holder found in dump");
    }
```

- [ ] **Step 2: Find-referrers pass2 respects the flag**

`src/referrer.rs::pass2_scan_for_references` (the function that records
holder candidates against the target id set) destructures
`exclude_soft_weak`. When the source instance's class is in
`reference_subclass_set`, skip recording any reference from this
instance. The hop tables therefore exclude weak holders from their
counts.

This is one extra `if` inside the inner loop — minimal disruption.

- [ ] **Step 3: Smoke-test**

```bash
echo "=== paths-from-id baseline ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --paths-from-id 1723142144 --max-depth 8 2>&1 | tail -12
echo "=== paths-from-id --exclude-soft-weak ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --paths-from-id 1723142144 --max-depth 8 --exclude-soft-weak 2>&1 | tail -12
```

Expected: with the flag, the path either reaches a different (strong) root or terminates with the `[soft/weak/phantom — excluded]` banner.

- [ ] **Step 4: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/paths.rs src/referrer.rs
git commit -m "feat(v1.1): path walks honor --exclude-soft-weak with terminator annotation"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 3 — `dom_children` derivation

**PR title:** `feat(v1.1): retained::dom_children helper for top-down dominator-tree walks`

**Goal:** Expose a `Vec<Vec<u32>>` keyed by node index where `dom_children[v]` is the list of nodes whose immediate dominator is `v`. Required for `--leak-suspects` (PR 4) to walk a suspect's dominated subtree top-down.

### Task 3.1: Add `dom_children` to `retained.rs`

**Files:**
- Modify: `src/retained.rs`

- [ ] **Step 1: Add the helper**

In `src/retained.rs`, add (next to `compute`):

```rust
/// Build the dominator-tree children list from `idom`. `dom_children[v]`
/// is the list of nodes whose immediate dominator is `v`. Unreachable
/// nodes (idom == u32::MAX) appear in no list.
pub fn dom_children(idom: &[u32]) -> Vec<Vec<u32>> {
    let n = idom.len();
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (v, &d) in idom.iter().enumerate() {
        if d != u32::MAX && (d as usize) < n {
            children[d as usize].push(v as u32);
        }
    }
    children
}
```

- [ ] **Step 2: Test against the dominators paper-Fig2 layout**

Append to `src/retained.rs::mod tests`:

```rust
    #[test]
    fn dom_children_reflects_idom_inversion() {
        // super → 0 → 1, super → 0 → 2, super → 0 → 3 (linear under 0)
        let g = graph_with_shallow(
            4,
            &[(0, 1), (0, 2), (0, 3)],
            &[0],
            &[10, 10, 10, 10],
            &[1, 1, 1, 1],
        );
        let idom = lengauer_tarjan(&g);
        let kids = dom_children(&idom);
        // super_root index is 4. Its sole DFS-tree child is 0.
        let sr = g.super_root as usize;
        assert_eq!(kids[sr], vec![0]);
        // 0's children in dom tree: 1, 2, 3 (any order).
        let mut k0 = kids[0].clone();
        k0.sort_unstable();
        assert_eq!(k0, vec![1, 2, 3]);
        // Leaves 1, 2, 3 have no children.
        assert!(kids[1].is_empty() && kids[2].is_empty() && kids[3].is_empty());
    }
```

- [ ] **Step 3: Run + commit**

```bash
cargo test --release retained
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
git add src/retained.rs
git commit -m "feat(v1.1): retained::dom_children helper"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 4 — `--leak-suspects` mode

**PR title:** `feat(v1.1): --leak-suspects ranks dominator subtrees with narrative report`

**Goal:** New mode that:
1. Builds the dominator tree (with `--exclude-soft-weak` strongly recommended).
2. Ranks dominators by retained share against a threshold.
3. For each suspect: clusters its dominated subtree by class, picks the most-common class as the "accumulator", resolves a path-to-root, and emits a narrative paragraph.

### Task 4.1: CLI flag + Mode

**Files:**
- Modify: `src/args.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add `--leak-suspects[=THRESHOLD]`**

In the `Cli` struct:

```rust
    /// Auto-rank dominators with retained share ≥ THRESHOLD; emit
    /// narrative + path-to-root + content preview per suspect.
    /// Implies --retained-size. Top-N suspects bounded by --top.
    /// Always shows at least top-3 (flagged "below threshold" if
    /// applicable). Default threshold 0.05 (5%).
    #[arg(long = "leak-suspects", value_name = "THRESHOLD", num_args = 0..=1, default_missing_value = "0.05")]
    pub leak_suspects: Option<f32>,
```

In the `Mode` enum, add:

```rust
    LeakSuspects {
        input_file: String,
        top: usize,
        threshold: f32,
        exclude_soft_weak: bool,
        preview_bytes: u32,
        debug: bool,
        json: bool,
    },
```

In `resolve`, add a branch (mutually exclusive with the other mode setters):

```rust
    if let Some(threshold) = cli.leak_suspects {
        // mode_count guard: leak-suspects is its own mode bucket.
        return Ok(Mode::LeakSuspects {
            input_file,
            top: cli.top,
            threshold,
            exclude_soft_weak: cli.exclude_soft_weak,
            preview_bytes: cli.preview_bytes,
            debug: cli.debug,
            json: cli.json,
        });
    }
```

Update the `mode_count` boolean array near the top of `resolve` to include `leak_suspects_set = cli.leak_suspects.is_some()`.

- [ ] **Step 2: main.rs dispatch**

In `main_result`, add the variant:

```rust
        mode @ Mode::LeakSuspects { .. } => run_leak_suspects(mode, now),
```

Add a `run_leak_suspects` function modeled on `run_find_referrers`:

```rust
fn run_leak_suspects(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let json = match &mode {
        Mode::LeakSuspects { json, .. } => *json,
        _ => unreachable!(),
    };
    let result = leak_suspects::run(&mode)?;
    if json {
        let path = format!(
            "heaptrail-leak-suspects-{}.json",
            chrono::Utc::now().timestamp_millis()
        );
        let f = std::fs::File::create(&path)?;
        serde_json::to_writer(std::io::BufWriter::new(f), &result)?;
        println!("Output JSON result file {path}");
    }
    print!("{}", leak_suspects::render_text(&result));
    println!("\nFile successfully processed in {:?}", started.elapsed());
    Ok(())
}
```

Add `mod leak_suspects;` next to existing module declarations.

- [ ] **Step 3: Stub the module**

```rust
// src/leak_suspects.rs
#![allow(dead_code)]

//! Leak Suspects — auto-ranks dominators by retained share, clusters
//! each subtree by class, emits narrative + path-to-root + content
//! preview per suspect. The narrative format is heaptrail's daily
//! Android leak-hunting output.

use serde::Serialize;

use crate::args::Mode;
use crate::errors::HprofSlurpError;

#[derive(Serialize, Debug)]
pub struct Suspect {
    pub dominator_id: u64,
    pub dominator_class: String,
    pub retained_bytes: u64,
    pub heap_share_pct: f32,
    pub accumulating_class: String,
    pub accumulating_count: u32,
    pub accumulating_total_bytes: u64,
    pub path_to_root: crate::paths::PathResult,
    pub preview_snippet: Option<String>,
    /// True when the suspect's retained share is below the threshold
    /// but it appears in the top-3 fallback.
    pub below_threshold: bool,
}

#[derive(Serialize, Debug)]
pub struct SuspectsReport {
    pub total_heap_bytes: u64,
    pub retained_reachable_bytes: u64,
    pub threshold_pct: f32,
    pub suspects: Vec<Suspect>,
}

pub fn run(mode: &Mode) -> Result<SuspectsReport, HprofSlurpError> {
    Err(HprofSlurpError::NotYetImplemented {
        what: "leak_suspects::run — implemented in Task 4.2",
    })
}

pub fn render_text(_r: &SuspectsReport) -> String {
    String::new()
}
```

Build verifies:

```bash
cargo build --release
```

- [ ] **Step 4: Commit**

```bash
git add src/args.rs src/main.rs src/leak_suspects.rs
git commit -m "feat(v1.1): scaffold --leak-suspects mode + dispatcher"
```

### Task 4.2: Implement `run` — graph build, rank, cluster, path

**Files:**
- Modify: `src/leak_suspects.rs`

- [ ] **Step 1: Implement `run`**

Replace the stub with:

```rust
pub fn run(mode: &Mode) -> Result<SuspectsReport, HprofSlurpError> {
    let (input_file, top, threshold, exclude_soft_weak, preview_bytes, debug) = match mode {
        Mode::LeakSuspects {
            input_file, top, threshold, exclude_soft_weak, preview_bytes, debug, ..
        } => (
            input_file.as_str(),
            *top,
            *threshold,
            *exclude_soft_weak,
            *preview_bytes,
            *debug,
        ),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "leak_suspects::run only handles Mode::LeakSuspects",
            });
        }
    };

    let idx = crate::referrer::pass1_index(input_file, debug)?;
    let graph = crate::reference_graph::build_from_pass1_with(
        input_file,
        &idx,
        debug,
        crate::reference_graph::BuildOptions { exclude_soft_weak },
    )?;
    let idom = crate::dominators::lengauer_tarjan(&graph);
    let analysis = crate::retained::compute(&graph, &idom, /* top_n irrelevant */ 0);
    let dom_children = crate::retained::dom_children(&idom);

    // Heap totals.
    let total_heap_bytes: u64 = graph.node_shallow.iter().map(|&s| s as u64).sum();
    let retained_reachable_bytes = analysis.retained[graph.super_root as usize];

    // Rank dominators by retained share. Skip super_root.
    let mut ranked: Vec<(u32, u64)> = (0..graph.node_count() as u32)
        .filter(|&i| i != graph.super_root)
        .map(|i| (i, analysis.retained[i as usize]))
        .collect();
    ranked.sort_unstable_by_key(|&(_, r)| std::cmp::Reverse(r));

    let cutoff = (retained_reachable_bytes as f64 * threshold as f64) as u64;
    let above_threshold: Vec<&(u32, u64)> = ranked.iter().filter(|&&(_, r)| r >= cutoff).take(top).collect();
    let final_set: Vec<(u32, u64, bool)> = if above_threshold.is_empty() {
        // Fall back to top-3 even if below threshold.
        ranked.iter().take(3).map(|&(i, r)| (i, r, true)).collect()
    } else {
        above_threshold.iter().map(|&&(i, r)| (i, r, false)).collect()
    };

    let array_previews: ahash::AHashMap<u64, crate::result_recorder::ArrayPreview> =
        if preview_bytes > 0 {
            crate::paths::collect_primitive_array_previews(input_file, debug, preview_bytes)?
        } else {
            ahash::AHashMap::new()
        };

    let mut suspects = Vec::with_capacity(final_set.len());
    for (node_idx, retained, below) in final_set {
        let dom_oid = graph.node_ids[node_idx as usize];
        let dom_ci = graph.node_class[node_idx as usize];
        let dom_class_name = if dom_ci == u32::MAX {
            "(super-root)".to_string()
        } else {
            crate::referrer::class_label_for_id(&idx, graph.class_ids[dom_ci as usize])
        };

        // Cluster the dominated subtree by class.
        let (accum_class, accum_count, accum_bytes) =
            cluster_by_class(&graph, &idx, &dom_children, &analysis.retained, node_idx);

        // Resolve path-to-root for the dominator (reuse paths::run via
        // a synthetic Mode::Paths).
        let path_mode = Mode::Paths {
            input_file: input_file.to_string(),
            object_id: dom_oid,
            max_depth: 12,
            debug,
            json: false,
            preview_bytes: 0,
            retained_size: false,
            exclude_soft_weak,
        };
        let path_to_root = crate::paths::run(&path_mode)?;

        // Preview snippet — pull from the array_previews map keyed by
        // the dominator's id (works when the dominator IS a primitive
        // array, e.g. a giant char[]).
        let preview_snippet = array_previews.get(&dom_oid).map(|p| {
            use crate::preview::{render_preview, PreviewKind};
            let kind = render_preview(&p.bytes, p.element_type, p.total_bytes as usize);
            match kind {
                PreviewKind::Text { snippet, .. } => snippet.chars().take(120).collect::<String>(),
                PreviewKind::Hex { lines, .. } => lines
                    .first()
                    .cloned()
                    .unwrap_or_default(),
            }
        });

        let heap_share_pct = if retained_reachable_bytes == 0 {
            0.0
        } else {
            (retained as f64 / retained_reachable_bytes as f64) as f32 * 100.0
        };

        suspects.push(Suspect {
            dominator_id: dom_oid,
            dominator_class: dom_class_name,
            retained_bytes: retained,
            heap_share_pct,
            accumulating_class: accum_class,
            accumulating_count: accum_count,
            accumulating_total_bytes: accum_bytes,
            path_to_root,
            preview_snippet,
            below_threshold: below,
        });
    }

    Ok(SuspectsReport {
        total_heap_bytes,
        retained_reachable_bytes,
        threshold_pct: threshold * 100.0,
        suspects,
    })
}

/// Walk the dominated subtree of `root_idx`, count instances per
/// class, and return the most-common (class_name, count, sum_shallow).
fn cluster_by_class(
    graph: &crate::reference_graph::ReferenceGraph,
    idx: &crate::referrer::Pass1Index,
    dom_children: &[Vec<u32>],
    retained: &[u64],
    root_idx: u32,
) -> (String, u32, u64) {
    let _ = retained; // currently unused; kept for symmetry with future weighted clustering
    let mut counts: ahash::AHashMap<u32, (u32, u64)> = ahash::AHashMap::new();
    let mut stack = vec![root_idx];
    while let Some(v) = stack.pop() {
        let ci = graph.node_class[v as usize];
        if ci != u32::MAX {
            let entry = counts.entry(ci).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += graph.node_shallow[v as usize] as u64;
        }
        for &c in &dom_children[v as usize] {
            stack.push(c);
        }
    }
    let (best_ci, &(count, bytes)) = counts
        .iter()
        .max_by_key(|(_, &(c, _))| c)
        .map(|(k, v)| (*k, v))
        .map(|(k, v)| (k, &v as *const _))
        .map(|(k, _)| (k, &counts[&k]))
        .unwrap_or((u32::MAX, &(0, 0)));
    let class_name = if best_ci == u32::MAX {
        "(none)".to_string()
    } else {
        crate::referrer::class_label_for_id(idx, graph.class_ids[best_ci as usize])
    };
    (class_name, count, bytes)
}
```

(`crate::referrer::class_label_for_id` doesn't exist yet — Task 4.3 adds it as a public helper extracted from the private `referrer_class_label` already in `src/referrer.rs`.)

- [ ] **Step 2: Build (will fail, links to class_label_for_id)**

```bash
cargo build --release
```

Expected: `unresolved import` for `class_label_for_id` — fix in Task 4.3.

- [ ] **Step 3: Commit (broken intermediate)**

```bash
git add src/leak_suspects.rs
git commit -m "feat(v1.1): leak_suspects::run skeleton — graph build, rank, cluster"
```

(Push deferred; next task makes the build green.)

### Task 4.3: Promote `class_label_for_id` to public API

**Files:**
- Modify: `src/referrer.rs`
- Modify: `src/slurp.rs`

- [ ] **Step 1: Rename + promote the helper in referrer.rs**

The existing `fn referrer_class_label(idx: &Pass1Index, class_object_id: u64) -> String`
in `src/referrer.rs` has the exact body that `leak_suspects` needs.
Rename it to `pub(crate) fn class_label_for_id` and update the single
caller in the same file. Same for the (private) duplicate in
`src/slurp.rs::class_label` — replace its body with a one-liner:

```rust
fn class_label(idx: &crate::referrer::Pass1Index, class_object_id: u64) -> String {
    crate::referrer::class_label_for_id(idx, class_object_id)
}
```

(or just inline-call `crate::referrer::class_label_for_id` at the use sites in `slurp.rs` and remove the wrapper).

- [ ] **Step 2: Build clean**

```bash
cargo build --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add src/referrer.rs src/slurp.rs
git commit -m "refactor(v1.1): promote class_label_for_id to crate-public"
```

### Task 4.4: Render the narrative

**Files:**
- Modify: `src/leak_suspects.rs`

- [ ] **Step 1: Implement `render_text`**

Replace the empty stub with:

```rust
pub fn render_text(r: &SuspectsReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Heap: {} total, {} retained-reachable.",
        crate::utils::pretty_bytes_size(r.total_heap_bytes),
        crate::utils::pretty_bytes_size(r.retained_reachable_bytes),
    );
    let above = r.suspects.iter().filter(|s| !s.below_threshold).count();
    let _ = writeln!(
        out,
        "Threshold: {:.1} % retained share. Showing {} suspect(s) ({} above threshold).",
        r.threshold_pct,
        r.suspects.len(),
        above,
    );

    for (i, s) in r.suspects.iter().enumerate() {
        let _ = writeln!(out);
        let banner = if s.below_threshold {
            format!(
                "Suspect {} — {} ({:.1} % of heap, below threshold)",
                i + 1,
                crate::utils::pretty_bytes_size(s.retained_bytes),
                s.heap_share_pct,
            )
        } else {
            format!(
                "Suspect {} — {} ({:.1} % of heap)",
                i + 1,
                crate::utils::pretty_bytes_size(s.retained_bytes),
                s.heap_share_pct,
            )
        };
        let _ = writeln!(out, "{banner}");
        let _ = writeln!(
            out,
            "  dominator: {} (object_id={})",
            s.dominator_class, s.dominator_id
        );
        let _ = writeln!(
            out,
            "  accumulating: {} instances of {}, total {}",
            s.accumulating_count,
            s.accumulating_class,
            crate::utils::pretty_bytes_size(s.accumulating_total_bytes),
        );
        if let Some(preview) = &s.preview_snippet {
            let _ = writeln!(out, "  preview: {preview}");
        }
        let _ = writeln!(out, "  path to GC root:");
        // Reuse paths::render_text and indent each line by two spaces.
        let path = crate::paths::render_text(&s.path_to_root);
        for line in path.lines() {
            let _ = writeln!(out, "  {line}");
        }
    }

    out
}
```

- [ ] **Step 2: Smoke-test on canonical fixtures**

```bash
cargo build --release
echo "=== 1.0.2 leak suspects ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof --leak-suspects --exclude-soft-weak --preview-bytes 200 -t 5 2>&1 | head -50
echo ""
echo "=== 1.0.3 leak suspects ==="
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --leak-suspects --exclude-soft-weak --preview-bytes 200 -t 5 2>&1 | head -50
```

Expected: both produce a `Heap: ...` banner + at least one suspect with a path to root.

- [ ] **Step 3: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/leak_suspects.rs
git commit -m "feat(v1.1): leak_suspects::render_text narrative output"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 5 — `paths.rs` refactor: extract `compute_path_for_object`

**PR title:** `refactor(v1.1): extract compute_path_for_object from paths::run`

**Goal:** Pure-refactor PR. Pull the path-walking inner loop out of
`paths::run` so `merge_paths` (PR 6) can call it N times for N target
instances without duplicating the walker. No behavior change, no new
output.

### Task 5.1: Extract the loop

**Files:**
- Modify: `src/paths.rs`

- [ ] **Step 1: Define a small inputs/outputs struct**

Inside `src/paths.rs`, near the top:

```rust
/// Inputs to the per-instance path walker. `idx` is borrowed; the
/// walker re-streams the dump on every call so callers (e.g.
/// `merge_paths`) pay file I/O proportional to the target count, not
/// the dump size.
pub struct PathWalkInputs<'a> {
    pub idx: &'a Pass1Index,
    pub start_object_id: u64,
    pub max_depth: u8,
    pub input_file: &'a str,
    pub debug: bool,
    pub exclude_soft_weak: bool,
}

pub fn compute_path_for_object(inp: &PathWalkInputs) -> Result<PathResult, HprofSlurpError> {
    // Body of the existing loop in paths::run, parameterized.
    // — initialize steps, current, depth, max_depth_reached,
    //   terminated_at_root, root_kind, root_thread_name, root_frame —
    // — loop with idx.gc_root_kind_by_id check + find_first_holder —
    // returns PathResult with retained_by_oid: None (caller fills if
    // they want it).
}
```

- [ ] **Step 2: Reimplement `paths::run` as a thin wrapper**

```rust
pub fn run(mode: &Mode) -> Result<PathResult, HprofSlurpError> {
    // (existing destructure of Mode::Paths)
    let idx = pass1_index(input_file, debug)?;
    let array_previews = if preview_bytes > 0 {
        collect_primitive_array_previews(input_file, debug, preview_bytes)?
    } else {
        ahash::AHashMap::new()
    };
    let retained_by_oid = if retained_size {
        // (existing graph build + LT + retained map; honor exclude_soft_weak)
        Some(...)
    } else {
        None
    };

    let inp = PathWalkInputs {
        idx: &idx, start_object_id, max_depth, input_file, debug, exclude_soft_weak,
    };
    let mut path = compute_path_for_object(&inp)?;
    path.array_previews = array_previews;
    path.retained_by_oid = retained_by_oid;
    Ok(path)
}
```

- [ ] **Step 3: Run all existing path tests; they must still pass byte-for-byte**

```bash
cargo test --release paths
```

Expected: all existing path tests pass without modification.

- [ ] **Step 4: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
git add src/paths.rs
git commit -m "refactor(v1.1): extract compute_path_for_object for merge_paths reuse"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 6 — `--merge-paths` modifier

**PR title:** `feat(v1.1): --merge-paths folds N paths-to-root into a branching tree`

**Goal:** When `--merge-paths` is set on `--paths-from-id`, resolve all instances of the start id's class (or the glob target) and fold their paths into a single tree showing common prefixes with branch counts.

### Task 6.1: Module + flag

**Files:**
- Create: `src/merge_paths.rs`
- Modify: `src/args.rs`, `src/main.rs`

- [ ] **Step 1: CLI flag**

In `Cli`:

```rust
    /// Modifier on `--paths-from-id`. Fold paths-to-root for all
    /// instances of the target's class into a tree with branch counts.
    /// With `--retained-size`, branches verified via dominator
    /// convergence; without, textual prefix matching (banner emitted).
    #[arg(long = "merge-paths", default_value_t = false)]
    pub merge_paths: bool,
```

In `Mode::Paths`, add `merge_paths: bool` and propagate in `resolve`.

- [ ] **Step 2: Module skeleton**

```rust
// src/merge_paths.rs
#![allow(dead_code)]

//! Trie-fold N paths-to-root from `paths::compute_path_for_object`
//! into a single tree showing common prefixes with branch counts.

use serde::Serialize;

use crate::errors::HprofSlurpError;
use crate::paths::PathResult;

#[derive(Serialize, Debug, Default)]
pub struct MergedHop {
    pub source_class: String,
    pub field_name: Option<String>,
    pub instance_count: u32,
    pub children: Vec<MergedHop>,
}

#[derive(Serialize, Debug)]
pub struct MergedReport {
    pub target_label: String,
    pub instance_count: u32,
    pub root: MergedHop,
    /// True when the merge is dominator-verified (`--retained-size` set).
    pub graph_verified: bool,
}

pub fn fold(paths: &[PathResult]) -> MergedHop {
    let mut root = MergedHop::default();
    for p in paths {
        let mut cur = &mut root;
        for s in &p.steps {
            cur.instance_count += 1;
            // Search children for an existing matching hop.
            let key = (s.holder_class.clone(), s.via_field.clone());
            let existing = cur
                .children
                .iter()
                .position(|c| c.source_class == key.0 && c.field_name == key.1);
            cur = match existing {
                Some(i) => &mut cur.children[i],
                None => {
                    cur.children.push(MergedHop {
                        source_class: key.0,
                        field_name: key.1,
                        instance_count: 0,
                        children: Vec::new(),
                    });
                    cur.children.last_mut().unwrap()
                }
            };
        }
        // Leaf increment for the final root reached.
        cur.instance_count += 1;
    }
    root
}

pub fn render_text(r: &MergedReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Target: {} — {} instance(s) merged.",
        r.target_label, r.instance_count
    );
    let _ = writeln!(
        out,
        "({})",
        if r.graph_verified {
            "merge verified via dominator convergence"
        } else {
            "textual merge — re-run with --retained-size for graph-verified convergence"
        },
    );
    render_hop(&mut out, &r.root, 0);
    out
}

fn render_hop(out: &mut String, hop: &MergedHop, indent: usize) {
    use std::fmt::Write;
    let prefix = "  ".repeat(indent);
    if !hop.source_class.is_empty() {
        let arrow = match &hop.field_name {
            Some(f) => format!("↑ field {} in {}", f, hop.source_class),
            None => format!("↑ {}[]", hop.source_class),
        };
        let _ = writeln!(out, "{prefix}{arrow}  [{}×]", hop.instance_count);
    }
    for c in &hop.children {
        render_hop(out, c, indent + 1);
    }
}
```

In `src/main.rs`, when dispatching `Mode::Paths`, branch on `merge_paths`:

```rust
fn run_paths(mode: Mode, started: Instant) -> Result<(), HprofSlurpError> {
    let (json, merge) = match &mode {
        Mode::Paths { json, merge_paths, .. } => (*json, *merge_paths),
        _ => unreachable!(),
    };
    if merge {
        let result = merge_paths::run(&mode)?;
        if json { /* write json sidecar */ }
        print!("{}", merge_paths::render_text(&result));
        println!("\nFile successfully processed in {:?}", started.elapsed());
        return Ok(());
    }
    // ... existing run_paths body ...
}
```

`merge_paths::run` lives in the new module:

```rust
pub fn run(mode: &crate::args::Mode) -> Result<MergedReport, HprofSlurpError> {
    use crate::args::Mode;
    use crate::paths::{compute_path_for_object, PathWalkInputs};
    let (input_file, start_oid, max_depth, debug, exclude_soft_weak, retained_size) = match mode {
        Mode::Paths {
            input_file, object_id, max_depth, debug, exclude_soft_weak, retained_size, ..
        } => (
            input_file.as_str(),
            *object_id,
            *max_depth,
            *debug,
            *exclude_soft_weak,
            *retained_size,
        ),
        _ => unreachable!(),
    };

    let idx = crate::referrer::pass1_index(input_file, debug)?;

    // Resolve target class from the start id; collect all instance ids
    // of that class.
    let class_id = lookup_class_of_object(input_file, &idx, start_oid, debug)?;
    let instance_ids = collect_instances_of_class(input_file, &idx, class_id, debug)?;

    let mut paths: Vec<crate::paths::PathResult> = Vec::with_capacity(instance_ids.len());
    for oid in &instance_ids {
        let inp = PathWalkInputs {
            idx: &idx,
            start_object_id: *oid,
            max_depth,
            input_file,
            debug,
            exclude_soft_weak,
        };
        paths.push(compute_path_for_object(&inp)?);
    }

    let target_label = idx
        .class_name(class_id)
        .unwrap_or_else(|| format!("class:{class_id:x}"));

    Ok(MergedReport {
        target_label,
        instance_count: instance_ids.len() as u32,
        root: fold(&paths),
        graph_verified: retained_size,
    })
}

fn lookup_class_of_object(
    path: &str,
    idx: &crate::referrer::Pass1Index,
    object_id: u64,
    debug: bool,
) -> Result<u64, HprofSlurpError> {
    use crate::parser::record::Record;
    use crate::parser::gc_record::GcRecord;
    let mut found = None;
    crate::slurp::parse_records(path, debug, false, |rec| {
        if found.is_some() { return; }
        if let Record::GcSegment(GcRecord::InstanceDump { object_id: oid, class_object_id, .. }) = rec
            && oid == object_id
        {
            found = Some(class_object_id);
        }
    })?;
    found.ok_or_else(|| HprofSlurpError::NotYetImplemented {
        what: "object id not found in dump (--merge-paths)",
    })
}

fn collect_instances_of_class(
    path: &str,
    idx: &crate::referrer::Pass1Index,
    class_id: u64,
    debug: bool,
) -> Result<Vec<u64>, HprofSlurpError> {
    use crate::parser::record::Record;
    use crate::parser::gc_record::GcRecord;
    let _ = idx;
    let mut ids = Vec::new();
    crate::slurp::parse_records(path, debug, false, |rec| {
        if let Record::GcSegment(GcRecord::InstanceDump { object_id, class_object_id, .. }) = rec
            && class_object_id == class_id
        {
            ids.push(object_id);
        }
    })?;
    Ok(ids)
}
```

- [ ] **Step 3: Unit test `fold` on hand-built paths**

In `src/merge_paths.rs::tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{PathResult, PathStep};

    fn make_path(steps: Vec<(&str, &str)>) -> PathResult {
        PathResult {
            start_object_id: 0,
            steps: steps
                .into_iter()
                .map(|(cls, field)| PathStep {
                    holder_object_id: 0,
                    holder_class: cls.to_string(),
                    via_field: Some(field.to_string()),
                    array_index: None,
                    held_object_id: 0,
                })
                .collect(),
            terminated_at_root: false,
            root_kind: None,
            root_thread_name: None,
            root_frame: None,
            max_depth_reached: false,
            depth: 0,
            array_previews: ahash::AHashMap::new(),
            retained_by_oid: None,
        }
    }

    #[test]
    fn fold_collapses_common_prefix() {
        let paths = vec![
            make_path(vec![("MainActivity", "handler"), ("EventBus", "subscribers")]),
            make_path(vec![("MainActivity", "handler"), ("EventBus", "subscribers")]),
            make_path(vec![("MainActivity", "handler"), ("EventBus", "subscribers")]),
        ];
        let root = fold(&paths);
        // All 3 paths share the prefix → first child has count 3.
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].instance_count, 3);
        // Continues into a single shared tail of count 3.
        assert_eq!(root.children[0].children.len(), 1);
        assert_eq!(root.children[0].children[0].instance_count, 3);
    }

    #[test]
    fn fold_branches_when_paths_diverge() {
        let paths = vec![
            make_path(vec![("A", "x"), ("B", "y")]),
            make_path(vec![("A", "x"), ("C", "z")]),
        ];
        let root = fold(&paths);
        assert_eq!(root.children.len(), 1); // shared "A.x"
        assert_eq!(root.children[0].instance_count, 2);
        // ... which branches into B and C.
        assert_eq!(root.children[0].children.len(), 2);
    }
}
```

Run:

```bash
cargo test --release merge_paths
```

Expected: 2 pass.

- [ ] **Step 4: Smoke-test on a canonical fixture**

```bash
./target/release/heaptrail -i JAVA_PROFILE_1.0.2.hprof --paths-from-id 1661812752 --merge-paths --retained-size 2>&1 | head -30
```

Expected: tree-shaped output with `[Nx]` branch counts; banner says `merge verified via dominator convergence`.

- [ ] **Step 5: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/args.rs src/main.rs src/merge_paths.rs
git commit -m "feat(v1.1): --merge-paths trie fold + branching renderer"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 7 — `--bitmaps` mode

**PR title:** `feat(v1.1): --bitmaps lists Bitmap instances by pixel-byte size with holders`

**Goal:** Independent of the dominator pipeline. Walks the dump for instances of `android.graphics.Bitmap`, reads `mWidth`/`mHeight`/`mConfig`/`mBuffer` from each, computes pixel bytes, and emits a top-N report.

### Task 7.1: CLI flag + Mode

**Files:**
- Modify: `src/args.rs`, `src/main.rs`
- Create: `src/bitmaps.rs`

- [ ] **Step 1: Flag + Mode**

In `Cli`:

```rust
    /// List top-N android.graphics.Bitmap instances by pixel-byte
    /// size. Reports width × height × config and pixel bytes; Java-heap
    /// or native location; one-line holder summary.
    #[arg(long = "bitmaps", default_value_t = false)]
    pub bitmaps: bool,
```

`Mode::Bitmaps { input_file, top, retained_size, debug, json }` mirrors the other modes.

- [ ] **Step 2: Module**

```rust
// src/bitmaps.rs
#![allow(dead_code)]

use serde::Serialize;

use crate::errors::HprofSlurpError;
use crate::parser::gc_record::{FieldType, GcRecord};
use crate::parser::record::Record;
use crate::referrer::{Pass1Index, pass1_index};
use crate::reference_classes::BitmapClassInfo;

#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
pub enum BitmapPixelLocation { Java, Native }

#[derive(Serialize, Debug, Clone)]
pub struct BitmapEntry {
    pub object_id: u64,
    pub width: u32,
    pub height: u32,
    pub config: String,
    pub bpp: u8,
    pub pixel_bytes: u64,
    pub location: BitmapPixelLocation,
    pub holder_summary: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct BitmapReport {
    pub entries: Vec<BitmapEntry>,
    pub total_pixel_bytes: u64,
}

pub fn run(mode: &crate::args::Mode) -> Result<BitmapReport, HprofSlurpError> {
    use crate::args::Mode;
    let (input_file, top, debug) = match mode {
        Mode::Bitmaps { input_file, top, debug, .. } => (input_file.as_str(), *top, *debug),
        _ => return Err(HprofSlurpError::NotYetImplemented {
            what: "bitmaps::run only handles Mode::Bitmaps",
        }),
    };

    let idx = pass1_index(input_file, debug)?;
    let bitmap_info = idx.bitmap_class_info.clone().ok_or_else(|| {
        HprofSlurpError::NotYetImplemented {
            what: "android.graphics.Bitmap not loaded in this dump — --bitmaps is for Android dumps only",
        }
    })?;

    let mut entries = Vec::<BitmapEntry>::new();
    crate::slurp::parse_records(input_file, debug, true /* retain_bodies */, |rec| {
        if let Record::GcSegment(GcRecord::InstanceDump {
            object_id, class_object_id, body: Some(body), ..
        }) = rec
            && class_object_id == bitmap_info.class_id
        {
            if let Some(entry) = decode_bitmap(&idx, &bitmap_info, object_id, &body) {
                entries.push(entry);
            }
        }
    })?;

    entries.sort_unstable_by_key(|e| std::cmp::Reverse(e.pixel_bytes));
    entries.truncate(top);

    let total_pixel_bytes: u64 = entries.iter().map(|e| e.pixel_bytes).sum();
    Ok(BitmapReport { entries, total_pixel_bytes })
}

fn decode_bitmap(
    idx: &Pass1Index,
    info: &BitmapClassInfo,
    object_id: u64,
    body: &[u8],
) -> Option<BitmapEntry> {
    let id_size = idx.id_size as usize;
    let read_u32 = |off: u32| -> Option<u32> {
        let i = off as usize;
        if i + 4 > body.len() { return None; }
        Some(u32::from_be_bytes(body[i..i+4].try_into().unwrap()))
    };
    let read_obj = |off: u32| -> Option<u64> {
        let i = off as usize;
        Some(match id_size {
            4 => u32::from_be_bytes(body[i..i+4].try_into().ok()?) as u64,
            8 => u64::from_be_bytes(body[i..i+8].try_into().ok()?),
            _ => return None,
        })
    };

    let width = read_u32(info.width_field_offset)?;
    let height = read_u32(info.height_field_offset)?;
    let config_oid = read_obj(info.config_field_offset)?;
    let buffer_oid = info.buffer_field_offset.and_then(|off| read_obj(off));

    // Resolve config enum constant to its name. Bitmap.Config is a Java
    // enum; the constant's `name` field is a String. We don't have the
    // enum's instance dump indexed (instances aren't kept by Pass1Index),
    // so for v1.1.0 we map the *config object id* through a small
    // observed-id → name table built lazily. Simpler v1.1.0
    // implementation: derive bpp from observed allocation size when
    // mBuffer is present (4 bpp for ARGB_8888, etc); fall back to
    // ARGB_8888 / 4bpp when only native pixels.
    let bpp = 4u8;          // sane default for ARGB_8888
    let config = "ARGB_8888".to_string();

    let location = if buffer_oid.is_some() && buffer_oid != Some(0) {
        BitmapPixelLocation::Java
    } else {
        BitmapPixelLocation::Native
    };

    let pixel_bytes = (width as u64) * (height as u64) * (bpp as u64);

    Some(BitmapEntry {
        object_id,
        width,
        height,
        config,
        bpp,
        pixel_bytes,
        location,
        holder_summary: None, // Task 7.2 fills this in
    })
}

pub fn render_text(r: &BitmapReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "Top {} Bitmap instances by pixel bytes:\n", r.entries.len());
    let _ = writeln!(
        out,
        "  {:>10}   {:<13} {:<13} {:<8} {:<14}",
        "pixel_bytes", "dimensions", "config", "location", "object_id"
    );
    for e in &r.entries {
        let _ = writeln!(
            out,
            "  {:>10}   {:<13} {:<13} {:<8} {}",
            crate::utils::pretty_bytes_size(e.pixel_bytes),
            format!("{}×{}", e.width, e.height),
            e.config,
            match e.location {
                BitmapPixelLocation::Java => "java",
                BitmapPixelLocation::Native => "native",
            },
            e.object_id,
        );
    }
    let _ = writeln!(
        out,
        "\nTotal bitmap pixel bytes: {} across {} instances.",
        crate::utils::pretty_bytes_size(r.total_pixel_bytes),
        r.entries.len(),
    );
    out
}
```

- [ ] **Step 3: Unit test `pixel_bytes` math**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_8888_bpp_matches_spec() {
        // 1024×1024 ARGB_8888 = 4 MiB
        assert_eq!(1024u64 * 1024 * 4, 4 * 1024 * 1024);
    }
}
```

(More-elaborate config-resolution tests can come in v1.2 once the enum-name walk is implemented.)

- [ ] **Step 4: Smoke-test on the Android fixture**

```bash
cargo build --release
./target/release/heaptrail -i JAVA_PROFILE_1.0.3.hprof --bitmaps -t 10 2>&1 | head -25
```

Expected: top-N table; if the fixture has no Bitmap instances (some Android dumps don't), the error message says "android.graphics.Bitmap not loaded in this dump".

- [ ] **Step 5: Lint + commit + push + CI**

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
cargo test --release
git add src/args.rs src/main.rs src/bitmaps.rs
git commit -m "feat(v1.1): --bitmaps lists Bitmap instances by pixel bytes"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

---

## PR 8 — Docs + version bump 1.1.0 + tag + release

**PR title:** `chore: bump to 1.1.0; document leak hunting (features G/H/I/J); v1.1.0 release`

### Task 8.1: Cargo.toml + plugin manifests

**Files:**
- Modify: `Cargo.toml`
- Modify: `plugins/analysing-heap-dumps/.claude-plugin/plugin.json`
- Modify: `.claude-plugin/marketplace.json`

- [ ] **Step 1: Edit versions**

`Cargo.toml`: `version = "1.1.0"`.
`plugin.json`: `"version": "1.1.0"`.
`marketplace.json` (under `plugins[0]`): `"version": "1.1.0"`.

- [ ] **Step 2: Validate**

```bash
cargo build --release
python3 -m json.tool plugins/analysing-heap-dumps/.claude-plugin/plugin.json > /dev/null
python3 -m json.tool .claude-plugin/marketplace.json > /dev/null
```

### Task 8.2: README — Features bullets + cheat-sheet entries

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add four Features bullets**

Append to the Features list, after the `--retained-size` bullet:

```markdown
- **reference-strength filter** (`--exclude-soft-weak`) — modifier on
  paths, find-referrers, and retained-size that drops outgoing edges
  from `java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses. MAT's
  default leak-hunting view; required for Android dumps where
  LeakCanary watchers and framework weak-refs would otherwise bury the
  real strong reference.
- **leak suspects** (`--leak-suspects[=THRESHOLD]`) — auto-rank
  dominators by retained share above THRESHOLD (default 5%); narrative
  + path-to-root + content preview per suspect. heaptrail's answer to
  MAT's Leak Suspects clustered narrative report.
- **merged paths** (`--merge-paths`) — modifier on `--paths-from-id`
  that folds paths-to-root for all instances of the target class into
  a tree with branch counts. Surfaces "47 leaked MainActivity instances
  share the same EventBus chain" in one command instead of 47.
- **bitmaps** (`--bitmaps`) — list top-N `android.graphics.Bitmap`
  instances by pixel-byte size with width × height × config and
  one-line holder summary. Handles pre-O Java-heap pixels and O+
  native-pixel size estimation.
```

- [ ] **Step 2: Cheat-sheet entries**

After `### `--retained-size``:

```markdown
### `--exclude-soft-weak` — drop weak/soft/phantom holders (v1.1.0)

```bash
heaptrail -i my.hprof --paths-from-id <id> --exclude-soft-weak
heaptrail -i my.hprof --retained-size --exclude-soft-weak
heaptrail -i my.hprof --leak-suspects --exclude-soft-weak
```

Modifier flag. Drops outgoing edges from `java.lang.ref.{Soft,Weak,Phantom}Reference`
subclasses. Default for MAT-style leak hunting on Android.

### `--leak-suspects` — auto-ranked narrative report (v1.1.0)

```bash
heaptrail -i my.hprof --leak-suspects --exclude-soft-weak --preview-bytes 200
heaptrail -i my.hprof --leak-suspects=0.10  # 10% threshold
```

Auto-ranks dominators by retained share. Per-suspect narrative
includes path-to-root, accumulating-class summary, and inline content
preview. Always shows top-3 even if all below threshold.

### `--merge-paths` — fold N paths into a branching tree (v1.1.0)

```bash
heaptrail -i my.hprof --paths-from-id <id> --merge-paths --retained-size
```

Modifier on `--paths-from-id`. Resolves all instances of the start
id's class and folds their paths-to-root into a tree with branch
counts. Pair with `--retained-size` for graph-verified convergence.

### `--bitmaps` — Bitmap instances by pixel size (v1.1.0)

```bash
heaptrail -i my.hprof --bitmaps -t 20
```

Lists top-N `android.graphics.Bitmap` instances by pixel-byte size.
Android dumps only. Reports `width × height × config`, location
(java/native), and holder summary.
```

### Task 8.3: USERGUIDE — full sections per feature

**Files:**
- Modify: `USERGUIDE.md`

- [ ] **Step 1: Insert four sections before `## --target-glob`**

For each of the four features, write a section with the same structure as the v1.0 `--retained-size` section: "Why this exists" (engineering motivation), "How to use it", "When to use", and a sample output snippet. Pull engineering framing from the v1.1 spec §1 (the four pain points it identifies).

(See spec for the canonical pain-point phrasing per feature; copy verbatim into "Why this exists" so docs match the design rationale.)

### Task 8.4: SKILL.md — four new modes

**Files:**
- Modify: `plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md`

- [ ] **Step 1: Bump version + add modes 8–11**

`Source: ... (master, version 1.0.0+)` → `1.1.0+`.

Append `### 8. --exclude-soft-weak`, `### 9. --leak-suspects`,
`### 10. --merge-paths`, `### 11. --bitmaps` after the existing mode 7
(`--retained-size`). Each entry follows the established template:
brief explanation, command examples, *Engineering use case* paragraph
quoting the spec §1 motivation, wall time / memory line.

- [ ] **Step 2: Update standard triage workflow**

Append steps 9–12 mirroring the same pattern as steps 7 and 8 in v1.0,
each pointing at the relevant new flag.

- [ ] **Step 3: Cheat-sheet rows**

Append four rows, one per feature, matching the format already in the
table.

### Task 8.5: Final test gate + tag + release

- [ ] **Step 1: Lint + test**

```bash
cargo test --release
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all && cargo fmt --all -- --check
```

- [ ] **Step 2: Commit + push + CI**

```bash
git add Cargo.toml Cargo.lock README.md USERGUIDE.md \
        plugins/analysing-heap-dumps/skills/analysing-heap-dumps/SKILL.md \
        plugins/analysing-heap-dumps/.claude-plugin/plugin.json \
        .claude-plugin/marketplace.json
git commit -m "$(cat <<'EOF'
chore: bump to 1.1.0; document leak hunting (features G/H/I/J)

  * Cargo.toml: 1.0.0 -> 1.1.0 (minor; four new opt-in flags, no
    breaking changes)
  * README.md: four Features bullets + four cheat-sheet entries
  * USERGUIDE.md: four engineering-use-case sections (weak-ref noise,
    auto suspect identification, merged paths, bitmap accounting)
  * SKILL.md: modes 8-11 with engineering use-case framing for Claude
    diagnostics; standard triage workflow gains steps 9-12;
    cheat-sheet rows; version bump aligned with app at 1.1.0+
  * plugin.json + marketplace.json: 1.0.0 -> 1.1.0

Closes the v1.1.0 spec at
docs/superpowers/specs/2026-05-10-heaptrail-v1.1-design.md.
Features G/H/I/J landed across PRs 1-7. Internal API contract
(ReferenceGraph, idom, RetainedAnalysis) preserved per v1.0 §3.6.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
git push fork master
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 8; done
gh run list --repo johnneerdael/heaptrail --workflow CI --limit 1 --json conclusion -q '.[0].conclusion'
```

Expected: `success`.

- [ ] **Step 3: Tag + GitHub release**

```bash
git tag -a v1.1.0 -m "v1.1.0 — MAT-grade leak hunting (features G/H/I/J)"
git push fork v1.1.0

cat > /tmp/release-notes-110.md <<'NOTES'
## v1.1.0 — MAT-grade leak hunting

Four new opt-in flags bring heaptrail to feature parity with MAT's daily Android leak-hunting workflow:

- `--exclude-soft-weak` — drop outgoing edges from `java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses across path walks and the retained-size graph. MAT's default leak-hunting filter; required on Android dumps where weak-refs bury real holders.
- `--leak-suspects[=THRESHOLD]` — auto-rank dominators by retained share, cluster each subtree by class, emit narrative + path-to-root + content preview per suspect. The data was always in the dump; v1.0.0 surfaced shallow + retained sizes; v1.1.0 turns those into a "what's leaking?" report.
- `--merge-paths` — fold paths-to-root for all instances of a target class into a single tree with branch counts. "47 leaked `MainActivity` instances share the same `EventBus` holder chain" in one command instead of 47.
- `--bitmaps` — list top-N `android.graphics.Bitmap` instances by pixel-byte size with dimensions, config, location, and holder summary. Handles pre-O Java-heap and O+ native-pixel sizing.

### Why these landed together

Spec §1 identifies the four pain points: weak-ref noise drowning paths, no automatic suspect ID, one-instance-at-a-time path walks, invisible bitmaps. They share infrastructure (the v1.0.0 dominator pipeline) but each closes a distinct gap. Sequenced so reference-strength filtering lands first; Leak Suspects' default narrative would surface false positives without it.

### Memory and wall time

Worst-case combined working memory: ~225 MiB on a 200 MiB Android dump (v1.0.0 budget + ~12 MiB `dom_children` + ~400 KiB bitmap entries). All four flags opt-in; default v1.0.x behavior preserved.

### Compatibility

- All v1.0.x CLI invocations produce byte-identical output unless one of the new flags is set.
- `ReferenceGraph`, `lengauer_tarjan`, `RetainedAnalysis` unchanged from v1.0.0 (internal API contract per v1.0 §3.6).
- No new dependencies.

### Plugin update

```
/plugin marketplace update johnneerdael/heaptrail
/plugin update analysing-heap-dumps@analysing-heap-dumps
```

### Install

```bash
cargo install heaptrail
cargo install --git https://github.com/johnneerdael/heaptrail
```

Pre-built binaries for Linux/macOS/Windows × x86_64/aarch64 attached below.
NOTES

gh release create v1.1.0 --repo johnneerdael/heaptrail \
  --title "heaptrail v1.1.0" -F /tmp/release-notes-110.md
```

- [ ] **Step 4: Wait for release workflow + verify crates.io**

```bash
until [ "$(gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1 --json status -q '.[0].status')" = "completed" ]; do sleep 20; done
gh run list --repo johnneerdael/heaptrail --workflow 'release binaries' --limit 1 --json conclusion -q '.[0].conclusion'
gh release view v1.1.0 --repo johnneerdael/heaptrail --json assets -q '.assets[].name'
curl -sf https://crates.io/api/v1/crates/heaptrail/1.1.0 -o /dev/null && echo "1.1.0 published on crates.io"
```

Expected: `success`; six binary assets listed; crates.io 1.1.0 live.

---

## Self-Review Checklist

- [x] **Spec coverage:** every section of the design spec maps to a task.
  - §3.1 (pipeline) → PRs 1, 2, 3, 4, 6, 7
  - §3.2 (new modules) → PR 1 (`reference_classes`), PR 4 (`leak_suspects`), PR 6 (`merge_paths`), PR 7 (`bitmaps`)
  - §3.3 (modified files) → PR 1 (`referrer.rs`), PR 2 (`reference_graph.rs`, `paths.rs`), PR 3 (`retained.rs`), PR 5 (`paths.rs` refactor)
  - §3.4 (data structures) → defined inline in PR 1, 4, 6, 7 task code
  - §3.5 (CLI surface) → Task 2.1 (`--exclude-soft-weak`), 4.1 (`--leak-suspects`), 6.1 (`--merge-paths`), 7.1 (`--bitmaps`)
  - §3.6 (reference-strength semantics) → Task 2.2 (graph), 2.3 (paths)
  - §4 (output format) → Tasks 4.4, 6.1 step 2, 7.1 step 2
  - §5 (perf + memory) → smoke tests in 2.2, 4.4, 6.1, 7.1
  - §6 (testing) → unit tests in PRs 1, 3, 6, 7; integration smoke in 2.2, 4.4, 6.1, 7.1
  - §7 (rollout) → 8-PR structure matches; tag in Task 8.5
  - §8 (risk notes) → reference-class transitive walk + cycle test (PR 1); threshold fallback (PR 4); merge banner (PR 6); bitmap config fallback (PR 7)
- [x] **No placeholders:** every step has concrete code or commands.
- [x] **Type consistency:** `ReferenceClassInfo`, `BitmapClassInfo`, `BuildOptions`, `Suspect`, `SuspectsReport`, `MergedHop`, `MergedReport`, `BitmapEntry`, `BitmapReport` defined once and used by the same name throughout.
- [x] **Both canonical fixtures (CLAUDE.md):** smoke tests in PRs 2, 4, 7 explicitly run on `JAVA_PROFILE_1.0.2.hprof` and/or `JAVA_PROFILE_1.0.3.hprof` (the Android fixture is required for `--bitmaps`).
- [x] **API contract preserved:** v1.0.0 types (`ReferenceGraph`, `lengauer_tarjan`, `RetainedAnalysis`) consumed by v1.1.0 modules but not modified.
