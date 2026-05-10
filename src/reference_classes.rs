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
//!    `mWidth`, `mHeight`, `mConfig`, and (pre-O) `mBuffer` instance
//!    fields. Used by `--bitmaps`.

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
    let info = ReferenceClassInfo {
        soft_weak_phantom: collect_soft_weak_phantom(idx),
    };
    let bitmap = detect_bitmap_class(idx);
    (info, bitmap)
}

fn collect_soft_weak_phantom(idx: &Pass1Index) -> AHashSet<u64> {
    // Find the three marker classes by name. HPROF stores class names
    // slash-form ("java/lang/ref/SoftReference"); compare against that
    // form since `Pass1Index.utf8_by_id` keeps the raw string.
    // HPROF dumps differ on class-name format: JVM dumps store slash
    // form (`java/lang/ref/WeakReference`), Android stores dotted form
    // (`java.lang.ref.WeakReference`). Match both.
    let mut markers = AHashSet::<u64>::new();
    for (&class_id, &name_id) in &idx.class_name_id_by_class_id {
        if let Some(name) = idx.utf8_by_id.get(&name_id) {
            let s = name.as_ref();
            if s == "java/lang/ref/SoftReference"
                || s == "java/lang/ref/WeakReference"
                || s == "java/lang/ref/PhantomReference"
                || s == "java.lang.ref.SoftReference"
                || s == "java.lang.ref.WeakReference"
                || s == "java.lang.ref.PhantomReference"
            {
                markers.insert(class_id);
            }
        }
    }
    if markers.is_empty() {
        return AHashSet::new();
    }

    // For each class, walk up super_class_by_id. If we hit a marker,
    // include the class. Memoize to keep the worst case linear.
    let mut subclass_set: AHashSet<u64> = AHashSet::new();
    let mut memo: ahash::AHashMap<u64, bool> = ahash::AHashMap::new();
    for &cid in idx.class_name_id_by_class_id.keys() {
        if is_subclass_of_any(cid, &markers, &idx.super_class_by_id, &mut memo) {
            subclass_set.insert(cid);
        }
    }
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

fn detect_bitmap_class(idx: &Pass1Index) -> Option<BitmapClassInfo> {
    // Match both slash and dotted forms (see collect_soft_weak_phantom).
    let class_id = find_class_id_by_name(idx, "android/graphics/Bitmap")
        .or_else(|| find_class_id_by_name(idx, "android.graphics.Bitmap"))?;

    // Walk the (own + super) field layout to compute byte offsets.
    // HPROF instance dump body order is: own-class fields, then super-class
    // fields, walking up. We mirror that by traversing the chain top-down.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::gc_record::FieldInfo;
    use crate::referrer::Pass1Index;

    fn make_idx() -> Pass1Index {
        let mut idx = Pass1Index {
            id_size: 4,
            ..Default::default()
        };
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
        for (cid, name_id) in [
            (1u64, 101u64),
            (2, 102),
            (3, 103),
            (4, 104),
            (5, 105),
            (6, 106),
        ] {
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
        idx.utf8_by_id
            .insert(101, "java/lang/ref/WeakReference".into());
        idx.utf8_by_id.insert(102, "com/example/Cyclic".into());
        idx.class_name_id_by_class_id.insert(1, 101);
        idx.class_name_id_by_class_id.insert(2, 102);
        idx.super_class_by_id.insert(2, 2); // self-cycle
        let (info, _) = derive(&idx);
        assert!(info.soft_weak_phantom.contains(&1));
        assert!(!info.soft_weak_phantom.contains(&2));
    }

    #[test]
    fn bitmap_class_info_offsets_with_no_super() {
        let mut idx = Pass1Index {
            id_size: 4,
            ..Default::default()
        };
        idx.utf8_by_id
            .insert(1u64, "android/graphics/Bitmap".into());
        idx.class_name_id_by_class_id.insert(100u64, 1u64);
        // Field layout: mNativeBitmap: long (offset 0..8),
        // mBuffer: Object (8..12), mWidth: int (12..16),
        // mHeight: int (16..20), mConfig: Object (20..24).
        idx.utf8_by_id.insert(11, "mNativeBitmap".into());
        idx.utf8_by_id.insert(12, "mBuffer".into());
        idx.utf8_by_id.insert(13, "mWidth".into());
        idx.utf8_by_id.insert(14, "mHeight".into());
        idx.utf8_by_id.insert(15, "mConfig".into());
        idx.fields_by_class_id.insert(
            100,
            vec![
                FieldInfo {
                    name_id: 11,
                    field_type: FieldType::Long,
                },
                FieldInfo {
                    name_id: 12,
                    field_type: FieldType::Object,
                },
                FieldInfo {
                    name_id: 13,
                    field_type: FieldType::Int,
                },
                FieldInfo {
                    name_id: 14,
                    field_type: FieldType::Int,
                },
                FieldInfo {
                    name_id: 15,
                    field_type: FieldType::Object,
                },
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
}
