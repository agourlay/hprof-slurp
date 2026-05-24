// Dead-code allowance until main.rs dispatch + smoke runs exercise it.
#![allow(dead_code)]

//! `--bitmaps` (v1.1.0 feature J) — independent of the dominator
//! pipeline. Walks the dump for instances of `android.graphics.Bitmap`,
//! reads `mWidth`/`mHeight`/`mConfig`/`mBuffer` from each, computes
//! pixel bytes, and emits a top-N report.

use serde::Serialize;

use crate::errors::HprofSlurpError;
use crate::parser::gc_record::GcRecord;
use crate::parser::record::Record;
use crate::reference_classes::BitmapClassInfo;
use crate::referrer::Pass1Index;

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitmapPixelLocation {
    Java,
    Native,
}

#[derive(Serialize, Debug, Clone)]
pub struct BitmapEntry {
    pub object_id: u64,
    pub width: u32,
    pub height: u32,
    pub config: String,
    pub bpp: u8,
    pub pixel_bytes: u64,
    pub location: BitmapPixelLocation,
    /// One-line holder summary; `None` in v1.1.0 (would require a
    /// path-to-root walk per bitmap, deferred to v1.2 for cost reasons).
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
        Mode::Bitmaps {
            input_file,
            top,
            debug,
            ..
        } => (input_file.as_str(), *top, *debug),
        _ => {
            return Err(HprofSlurpError::NotYetImplemented {
                what: "bitmaps::run only handles Mode::Bitmaps",
            });
        }
    };

    let idx = crate::referrer::pass1_index(input_file, debug)?;
    let bitmap_info = idx
        .bitmap_class_info
        .clone()
        .ok_or(HprofSlurpError::BitmapClassNotLoaded)?;

    let mut entries = Vec::<BitmapEntry>::new();
    crate::slurp::parse_records(input_file, debug, true, |rec| {
        if let Record::GcSegment(GcRecord::InstanceDump {
            object_id,
            class_object_id,
            body: Some(body),
            ..
        }) = rec
            && class_object_id == bitmap_info.class_id
            && let Some(entry) = decode_bitmap(&idx, &bitmap_info, object_id, &body)
        {
            entries.push(entry);
        }
    })?;

    entries.sort_unstable_by_key(|e| std::cmp::Reverse(e.pixel_bytes));
    entries.truncate(top);

    let total_pixel_bytes: u64 = entries.iter().map(|e| e.pixel_bytes).sum();
    Ok(BitmapReport {
        entries,
        total_pixel_bytes,
    })
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
        if i + 4 > body.len() {
            return None;
        }
        Some(u32::from_be_bytes(body[i..i + 4].try_into().ok()?))
    };
    let read_obj = |off: u32| -> Option<u64> {
        let i = off as usize;
        match id_size {
            4 => {
                if i + 4 > body.len() {
                    return None;
                }
                Some(u32::from_be_bytes(body[i..i + 4].try_into().ok()?) as u64)
            }
            8 => {
                if i + 8 > body.len() {
                    return None;
                }
                Some(u64::from_be_bytes(body[i..i + 8].try_into().ok()?))
            }
            _ => None,
        }
    };

    let width = read_u32(info.width_field_offset)?;
    let height = read_u32(info.height_field_offset)?;
    let _config_oid = read_obj(info.config_field_offset)?;
    let buffer_oid = info.buffer_field_offset.and_then(read_obj);

    // v1.1.0: assume ARGB_8888 (4 bpp). Resolving Bitmap.Config enum
    // names requires an extra pass to look up the enum constant's
    // `name` field; deferred to v1.2.
    let bpp: u8 = 4;
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
        holder_summary: None,
    })
}

pub fn render_text(r: &BitmapReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Top {} Bitmap instances by pixel bytes:\n",
        r.entries.len()
    );
    let _ = writeln!(
        out,
        "  {:>10}   {:<13} {:<13} {:<8} object_id",
        "pixel_bytes", "dimensions", "config", "location",
    );
    for e in &r.entries {
        let dim = format!("{}×{}", e.width, e.height);
        let loc = match e.location {
            BitmapPixelLocation::Java => "java",
            BitmapPixelLocation::Native => "native",
        };
        let pb = crate::utils::pretty_bytes_size(e.pixel_bytes);
        let cfg = &e.config;
        let oid = e.object_id;
        let _ = writeln!(out, "  {pb:>10}   {dim:<13} {cfg:<13} {loc:<8} {oid}");
    }
    let _ = writeln!(
        out,
        "\nTotal bitmap pixel bytes: {} across {} instances.",
        crate::utils::pretty_bytes_size(r.total_pixel_bytes),
        r.entries.len(),
    );
    out
}
