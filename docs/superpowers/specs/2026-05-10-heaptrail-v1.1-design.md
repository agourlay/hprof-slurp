# heaptrail v1.1.0 — MAT-grade leak hunting (features G/H/I/J)

**Status:** Draft. Depends on v1.0.0 retained-size + dominator infrastructure.

**Decisions:**
- Four flags, individually opt-in: `--exclude-soft-weak` (modifier), `--leak-suspects` (mode), `--merge-paths` (modifier on `--paths-from-id`), `--bitmaps` (mode). No mega-flag.
- Three of four sit on the v1.0.0 graph + dominator infrastructure; `--bitmaps` is fully independent.
- Sequenced so reference-strength filtering lands before Leak Suspects, ensuring the default narrative isn't polluted by weak-ref false positives.

---

## 1. Problem

v1.0.0 lands retained-size and dominators — the hard infrastructure work — but leaves the user one step short of MAT's actual leak-hunting workflow:

- **Weak-reference noise** drowns paths-to-root on Android. LeakCanary's own watchers, the framework's `WeakReference` to every `Activity`, and `Reference.discovered` chains all show up as holders, burying the real strong reference.
- **No automatic suspect identification.** `summary --retained-size` requires the user to already know what class to investigate. Anyone running it on an unfamiliar dump still has to eyeball the table — exactly the diagnosis MAT's Leak Suspects automates.
- **One-instance-at-a-time path walks** lose the most diagnostic Android signal: when 47 leaked `MainActivity` instances share the same holder chain, the *common prefix* tells you "it's the EventBus" — but `--paths-from-id` walks one instance at a time.
- **Bitmaps are invisible** to the class-name view. A 12 MiB `byte[]` is just "another big primitive array" until you see it's a 4096×4096 ARGB_8888 bitmap held by a `RecyclerView.ViewHolder`.

These four gaps are what v1.1.0 closes. Together they bring heaptrail to feature parity with MAT's daily Android leak-hunting workflow.

## 2. Goals & non-goals

**Goals:**
- `--exclude-soft-weak`: drop outgoing edges from `java.lang.ref.{Soft,Weak,Phantom}Reference` subclasses across path walks and retained-size graph build.
- `--leak-suspects`: auto-rank top-K dominators by retained share above a threshold, cluster dominated objects by class, emit narrative + path-to-root + content preview per suspect.
- `--merge-paths`: given a target class (via `--target-glob` or explicit name), fold paths-to-root for all (or top-K) instances into a tree showing common prefixes with branch counts.
- `--bitmaps`: list top-N Bitmap instances by pixel-byte size, with width/height/config and holder summary. Handle both pre-O (Java-heap pixel data via `mBuffer`) and O+ (native pixel data sized via `width × height × bpp`).
- Each flag opt-in; default behavior byte-identical to v1.0.x.
- Both canonical fixtures covered by smoke tests.

**Non-goals:**
- No incremental dominator updates as filters change. `--exclude-soft-weak --retained-size` rebuilds the graph from scratch (cost: same as v1.0.0, ~1.5 s on the 200 MiB fixture; edge count typically drops 5–15 %).
- No `--exclude-finalizer` for `java.lang.ref.FinalReference` / `Finalizer` chains. Defer; MAT exposes it as a separate option.
- No multi-suspect cross-correlation ("these two suspects share an accumulator"). Deferred.
- No native-bitmap content extraction or hashing. Pixel bytes are computed from `width × height × config`; we don't try to render or hash native pixels.
- No retained-size deltas in `--diff-from`/`--diff-to`. Out of scope (also out of scope in v1.0.0).

## 3. Architecture

### 3.1 Pipeline

```
   pass 1 (existing in v1.0.0, extended in v1.1.0):
      utf8 + classes + layouts + GC roots → Pass1Index
      NEW: reference_subclass_set (class_ids that are subclasses of
           java.lang.ref.{Soft,Weak,Phantom}Reference)
      NEW: bitmap_class_info (resolved if android.graphics.Bitmap is
           loaded in the dump: class_id, field offsets for
           mWidth/mHeight/mConfig/mBuffer)

   reference_graph build (v1.0.0 module, extended):
      NEW: BuildOptions { exclude_soft_weak: bool }. When set, skip
           outgoing edges from any source node whose class_id is in
           reference_subclass_set. idom + retained then computed on the
           filtered graph as in v1.0.0.

   dominators::lengauer_tarjan          (unchanged from v1.0.0)
   retained::compute                    (unchanged from v1.0.0)
   retained::dom_children(idom)         (NEW: derived structure for
                                         leak_suspects + merge_paths)

   NEW analysis modes:
      leak_suspects::run(graph, idom, retained, dom_children) → SuspectsReport
      merge_paths::run(target_class_ids, [optional idom])     → MergedPathTree
      bitmaps::run(Pass1Index, instance_stream)               → BitmapReport
```

### 3.2 New modules

| File | Responsibility |
|------|----------------|
| `src/reference_classes.rs` | Walk class hierarchy in Pass1Index; build `reference_subclass_set` and `bitmap_class_info` lookups. Pure derivation; no parsing. |
| `src/leak_suspects.rs` | Rank dominator subtrees by retained share, cluster by class, emit narrative report. |
| `src/merge_paths.rs` | Fold N paths-to-root into a trie; render as branching tree with counts. |
| `src/bitmaps.rs` | Identify Bitmap instances; compute pixel bytes (Java-heap or native); render report with holder summary. |

### 3.3 Modified files

| File | Change |
|------|--------|
| `src/referrer.rs` | `Pass1Index` gains `reference_subclass_set: AHashSet<u64>` and `bitmap_class_info: Option<BitmapClassInfo>`. Path walk respects `exclude_soft_weak` (treats Reference subclass nodes as walk terminators with `[soft/weak/phantom — excluded]` annotation). |
| `src/reference_graph.rs` | Builder accepts `BuildOptions { exclude_soft_weak: bool }`; skips outgoing edges from Reference subclass source nodes when set. |
| `src/retained.rs` | New public helper `dom_children(idom: &[u32]) -> Vec<Vec<u32>>` for top-down dominator-tree traversal. |
| `src/paths.rs` | Refactor: extract `compute_path_for_object(idx, oid, opts) -> Path` so `merge_paths` can call it N times. Walk respects `exclude_soft_weak`. |
| `src/args.rs` | Four new flags. New `Mode::LeakSuspects`, `Mode::Bitmaps`. `--exclude-soft-weak` is a *modifier* compatible with `Mode::Summary`/`Paths`/`FindReferrers`/`LeakSuspects`. `--merge-paths` is a *modifier* on `Mode::Paths`. |
| `src/main.rs` | Dispatch on the two new modes; thread modifier flags into existing dispatchers. |

### 3.4 Data structures

```rust
// src/reference_classes.rs
pub struct ReferenceClassInfo {
    pub soft_weak_phantom: AHashSet<u64>,    // class_ids
}
pub struct BitmapClassInfo {
    pub class_id: u64,
    pub width_field_offset: u32,
    pub height_field_offset: u32,
    pub config_field_offset: u32,
    pub buffer_field_offset: Option<u32>,    // mBuffer (pre-O); None on O+
}

// src/leak_suspects.rs
pub struct Suspect {
    pub dominator_id: u64,
    pub dominator_class: String,
    pub retained_bytes: u64,
    pub heap_share_pct: f32,
    pub accumulating_class: String,          // most-common class in subtree
    pub accumulating_count: u32,
    pub accumulating_total_bytes: u64,
    pub path_to_root: Path,                  // reuses paths.rs Path type
    pub preview_snippet: Option<String>,     // populated when --preview-bytes set
}
pub struct SuspectsReport {
    pub total_heap_bytes: u64,
    pub suspects: Vec<Suspect>,
    pub threshold_pct: f32,
}

// src/merge_paths.rs
pub struct MergedHop {
    pub source_class: String,
    pub field_name: Option<String>,
    pub instance_count: u32,
    pub children: Vec<MergedHop>,
}

// src/bitmaps.rs
pub enum BitmapPixelLocation { Java, Native }
pub struct BitmapEntry {
    pub object_id: u64,
    pub width: u32,
    pub height: u32,
    pub config: String,                      // "ARGB_8888", "RGB_565", ...
    pub bpp: u8,
    pub pixel_bytes: u64,
    pub location: BitmapPixelLocation,
    pub holder_summary: Option<String>,      // truncated 1-line path
}
pub struct BitmapReport {
    pub entries: Vec<BitmapEntry>,           // sorted by pixel_bytes desc
    pub total_pixel_bytes: u64,
}
```

### 3.5 CLI surface

```
--exclude-soft-weak   Modifier. Drop outgoing edges from
                      java.lang.ref.{Soft,Weak,Phantom}Reference
                      subclasses across path walks and retained-size
                      graph. Compatible with --paths-from-id,
                      --find-referrers, --retained-size, --leak-suspects,
                      --merge-paths.

--leak-suspects[=THRESHOLD]
                      Implies --retained-size. Auto-rank dominators
                      with retained share ≥ THRESHOLD (default 0.05 =
                      5 %); emit narrative + path-to-root + content
                      preview per suspect. Top-N suspects bounded by
                      --top. Always shows at least top-3 (flagged
                      "below-threshold" if applicable).

--merge-paths         Modifier on --paths-from-id <target>. When set,
                      fold paths-to-root for all instances of the
                      target class (resolved via --target-glob or
                      explicit name) into a single tree with branch
                      counts. With --retained-size, branches verified
                      via dominator convergence; without, textual
                      prefix matching (banner emitted).

--bitmaps             List top-N Bitmap instances by pixel-byte size.
                      Bounded by --top. Reports width × height × config
                      and pixel bytes; Java-heap or native location;
                      one-line holder summary. With --retained-size,
                      adds retained column.
```

### 3.6 Reference-strength filter — semantics

`--exclude-soft-weak` drops *all outgoing edges* from any node whose class is in the soft/weak/phantom subclass set. Rationale (locked in v1.0.0 §3.5): `Reference.referent` is the only object-typed field that matters from a Reference instance, and it's always weak by definition. Suppressing the entire fan is equivalent to per-edge filtering and avoids per-edge memory cost.

The filter applies symmetrically:
- **Graph build (with `--retained-size`):** Reference-subclass source nodes have 0 outgoing edges, so retained sums no longer credit those nodes' children to anything dominated through them. An object reachable *only* through a `WeakReference` becomes unreachable in the filtered graph (retained == 0). This matches MAT's leak-hunting default.
- **Path walks (`--paths-from-id`, `--find-referrers`, `--merge-paths`):** when a hop lands on a Reference-subclass node, walk terminates there with annotation `[soft/weak/phantom — excluded]`. Path is truncated; user can re-run without the flag to see the soft-held chain.

The two surfaces share the same `reference_subclass_set` lookup but apply it independently — `--exclude-soft-weak --paths-from-id <id>` works without `--retained-size`.

## 4. Output format

### 4.1 `--leak-suspects`

```
Heap: 234.18 MiB total, 187.42 MiB retained-reachable.
Threshold: 5.0 % retained share. Showing top 3 suspects (1 above threshold).

Suspect 1 — 47.32 MiB (25.2 % of heap)
  dominator: com.example.app.AppCache (object_id=4097812752)
  accumulating: 234 instances of java.lang.String, total 42.10 MiB
  preview: {"airsDays":["Mon","Wed","Fri"],"seasonNumber":3, ...
  path to GC root:
    [id=4097812752] com.example.app.AppCache (retained=47.32 MiB)
      ↑ static field INSTANCE in AppCache
        (root: System Class)

Suspect 2 — 4.14 MiB (2.2 % of heap, below threshold)
  dominator: com.example.tvdb.SeriesParser (object_id=2097446928)
  accumulating: 3 instances of char[], total 3.10 MiB
  preview: <?xml version="1.0" ?><map><string name="alias::en::policy ...
  path to GC root:
    [id=2097446928] com.example.tvdb.SeriesParser (retained=4.14 MiB)
      ↑ field reader in StringReader (retained=3.10 MiB)
        ↑ field locals in Thread "GsonParser-3"
          (root: Thread "GsonParser-3", top frame: Gson.fromJson(String,Type) at Gson.java:932)
```

The narrative format is the v1.1.0 differentiator vs MAT: content previews are inline per suspect, and thread/frame context is resolved at root terminators (already present from v0.8.0 path walks).

### 4.2 `--merge-paths`

```
Target: com.example.app.MainActivity — 37 instances, 142.18 MiB retained.
(merge verified via dominator convergence)

  ↑ field handler in MainActivity                              [37×]
    ↑ field this$0 in EventBus$SubscriberHolder                 [37×]
      ↑ key in ConcurrentHashMap$Node (subscribers)             [37×]
        ↑ static EventBus.INSTANCE                              [37×]
          (root: System Class)
```

When paths fork, branches render as their own subtrees:

```
  ↑ field handler in MainActivity                              [25×]
    ↑ ... continues ...
  ↑ field activityRef in NavHostController                     [12×]
    ↑ ... continues ...
```

Without `--retained-size`, the second line of the header is `(textual merge — re-run with --retained-size for graph-verified convergence)`.

### 4.3 `--bitmaps`

```
Top 20 Bitmap instances by pixel bytes:

   pixel_bytes   dimensions    config         location  object_id
     64.00 MiB   4096×4096     ARGB_8888      native    4097812752
     12.00 MiB   1024×3072     ARGB_8888      java      2097446928
      4.00 MiB   1024×1024     ARGB_8888      java      1723142144
      ...

Holder summaries (top 5):
  4097812752 ← RecyclerView$ViewHolder.itemView ← FragmentManager
  2097446928 ← Drawable.mBitmap ← ImageView ← MainActivity
  ...

Total bitmap pixel bytes: 96.50 MiB across 247 instances.
```

With `--retained-size`, an additional `retained` column appears (sums the bitmap object + its pixel array on pre-O; bitmap object only on O+ since native pixels aren't in the dump).

## 5. Performance & memory

### 5.1 `--exclude-soft-weak`

| Phase | Cost |
|-------|------|
| Class-hierarchy walk to build `reference_subclass_set` | ~10 ms (one-time, cached in Pass1Index) |
| Reference-graph rebuild | same as v1.0.0 (~1.5 s on 200 MiB fixture); typically slightly faster — edge count drops 5–15 % on Android dumps |
| Working memory | ~0; no extra structures |

### 5.2 `--leak-suspects`

| Phase | Cost |
|-------|------|
| `dom_children` derivation from `idom` | ~12 MiB `Vec<Vec<u32>>` on 3M-node graph |
| Suspect ranking | linear scan of `retained[]` — negligible |
| Per-suspect class clustering | walk dominated subtree, count per class; bounded by suspect's subtree size (tens of thousands of nodes for top suspects) |
| Per-suspect path-to-root | existing `--paths-from-id` cost (~5–20 ms each) |
| **End-to-end after `--retained-size` paid** | **~200 ms** |

### 5.3 `--merge-paths`

| Phase | Cost |
|-------|------|
| Path walks for all instances | N × per-instance path cost. 50 leaked `MainActivity`s = ~1 s |
| Trie insertion + collapse | negligible (paths are short, ~10–50 hops) |
| Working memory | trie bounded by total unique hops; <1 MiB realistic |

### 5.4 `--bitmaps`

| Phase | Cost |
|-------|------|
| Single instance scan filtered by `bitmap_class_info.class_id` | similar to existing summary pass |
| Per-bitmap field reads (width/height/config/buffer) | ~8 bytes/inst × N — negligible |
| Working memory | O(N_bitmaps × 80 bytes for `BitmapEntry`); 5000 bitmaps = ~400 KiB |

### 5.5 Combined ceiling

Worst-case concurrent flag set: `summary --retained-size --exclude-soft-weak --leak-suspects --bitmaps`. Working memory = v1.0.0 budget (210 MiB on the 200 MiB fixture) + ~12 MiB for `dom_children` + ~400 KiB for bitmap entries ≈ **225 MiB**. Within v1.0.0's stated headroom.

## 6. Testing strategy

### Unit tests
- `reference_classes::soft_weak_phantom_set`: synthetic class hierarchy fixture; assert `WeakReference`, `SoftReference`, `PhantomReference`, and a hand-rolled `WeakReference` subclass all appear; abstract `Reference` itself does *not*.
- `leak_suspects::rank_and_cluster`: synthetic `RetainedAnalysis` + `dom_children`; assert top-K suspects ordered by retained desc; threshold filter applied; below-threshold top-3 fallback works.
- `merge_paths::fold`: hand-built path list with known overlap; assert trie shape, branch counts, and dominator-verification banner match.
- `bitmaps::pixel_bytes_for_config`: assert ARGB_8888 → 4 bpp, RGB_565 → 2, ALPHA_8 → 1, RGBA_F16 → 8.

### Integration tests
- `summary --retained-size --exclude-soft-weak` on `JAVA_PROFILE_1.0.3.hprof`: assert retained for at least one class drops vs. without the flag (proves filter is active).
- `--leak-suspects` on `JAVA_PROFILE_1.0.3.hprof`: assert at least one suspect surfaces; assert path-to-root has a resolved frame at its terminator.
- `--paths-from-id <id> --merge-paths --target-glob "java.lang.String"`: assert merged tree shape with branch counts.
- `--bitmaps` on `JAVA_PROFILE_1.0.3.hprof` (Android fixture, expected to contain Bitmap instances): assert at least one entry with non-zero pixel_bytes; assert ordering by pixel_bytes desc.
- Regression: every v1.0.x command without the new flags is byte-identical.

## 7. Rollout

Eight sequential PRs onto `master`, single `v1.1.0` tag.

| PR | Title | Lands |
|----|-------|-------|
| 1 | `reference_classes`: soft/weak/phantom subclass + bitmap class detection | Pass1Index extension + tests, no user-visible effect |
| 2 | `--exclude-soft-weak` modifier across paths + find-referrers + graph build | walk respects flag; reference-graph builder skips fans |
| 3 | `dom_children` derivation in `retained.rs` | helper exposed for `--leak-suspects`; tested |
| 4 | `--leak-suspects` mode | ranking + clustering + narrative + path resolution + content-preview integration |
| 5 | `paths.rs` refactor — extract `compute_path_for_object` | API-only PR; no behavior change |
| 6 | `--merge-paths` modifier | trie fold + branch-count rendering + dominator-verified banner |
| 7 | `--bitmaps` mode | bitmap class detection + pixel-byte computation + holder summary |
| 8 | docs + version 1.1.0 + tag + release | README, USERGUIDE, SKILL bumps; binaries |

PR ordering rationale:
- PR 1 lays the Pass1Index foundation; PR 2 consumes it.
- PRs 1–2 must land before PR 4: Leak Suspects' default narrative is computed on a soft/weak-filtered graph implicitly (we recommend running `--leak-suspects --exclude-soft-weak` together; document as the default workflow). Without PR 2, weak-ref-only-held objects would surface as suspects.
- PR 5 is a pure refactor and could swap places with PR 6, but landing it standalone makes review trivial.
- PR 7 is independent and could ship anywhere after PR 1, but lands last to keep the v1.1.0 tag tied to the full feature set.

## 8. Risk notes

- **Reference-class detection accuracy.** The filter relies on detecting *all* subclasses of `java.lang.ref.{Soft,Weak,Phantom}Reference`. App-defined subclasses (LeakCanary's `KeyedWeakReference`, framework `FinalizerReference`) must be detected via transitive class-hierarchy walk. Mitigation: walk up `super_class_id` until a known reference base or null; cache per class. Test on the 1.0.3 fixture where LeakCanary watchers are likely present.
- **Leak Suspects threshold tuning.** 5 % default matches MAT. On dumps with one massive cache (>50 % retained), every other suspect falls below threshold and the report shows only one entry — accurate but visually thin. Mitigation: always show top-3 even if below threshold; flag them as `(below threshold)` in output. Adjustable via `--leak-suspects=THRESHOLD`.
- **Merge-paths false convergence without `--retained-size`.** Textual prefix matching can fold two paths that share a hop class+field but are not graph-converging at the same dominator. Mitigation: when `idom` is available (i.e., `--retained-size` is also set), verify each merged branch is rooted at a true dominator; otherwise emit `(textual merge — re-run with --retained-size for graph-verified convergence)` banner.
- **Bitmap config enum resolution.** `mConfig` is a reference to a Bitmap.Config enum constant. Resolving it to a name requires reading the enum's `name` field. Mitigation: resolve once per distinct `mConfig` object id; cache. Fall back to literal id if name field is absent.
- **Native bitmap pixel-byte estimation.** For O+ bitmaps we compute `width × height × bpp` from config; native heap is not in the dump. If Android stores the bitmap with a stride > width × bpp (rare; row alignment), our number is a slight under-estimate. Mitigation: document as approximate; users wanting exact native sizes use `dumpsys meminfo`. Out of scope for hprof analysis.
- **Crate API surface.** `ReferenceGraph`, `lengauer_tarjan`, and `RetainedAnalysis` are the v1.x internal contract (locked in v1.0.0 §3.6). v1.1.0 *consumes* them but does not modify them — only adds derived structures (`dom_children`) and new top-level analysis modules. No breaking changes to v1.0.x's contract.
