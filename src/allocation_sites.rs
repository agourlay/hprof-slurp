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

    // Slurp the file once for the AllocationSite list (lives on
    // RenderedResult after v0.8.0's recorder enhancement) and again for
    // the Pass1Index that has the class+frame resolution maps.
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
        .map(|s| resolve_site(s, &idx))
        .collect();

    Ok(AllocationSitesResult {
        total_sites: sites.len(),
        top: resolved,
    })
}

fn resolve_site(s: &AllocationSite, idx: &Pass1Index) -> ResolvedAllocSite {
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
            json_out: None,
        };
        match run(&mode) {
            Err(HprofSlurpError::NoAllocationSites) => {}
            other => panic!("expected NoAllocationSites, got {other:?}"),
        }
    }
}
