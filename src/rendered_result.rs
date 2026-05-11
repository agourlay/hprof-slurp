use std::{fmt::Write, fs::File, io::BufWriter};

use chrono::Utc;
use serde::Serialize;

use crate::{errors::HprofSlurpError, utils::pretty_bytes_size};

#[derive(Serialize, Clone)]
pub struct ClassAllocationStats {
    pub class_name: String,
    pub instance_count: u64,
    pub largest_allocation_bytes: u64,
    /// Object id of the largest single instance (arrays only). 0 = unset / not an
    /// array class. Useful for retainer tracing (`--target-object-id N`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub largest_object_id: u64,
    pub allocation_size_bytes: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl ClassAllocationStats {
    pub const fn new(
        class_name: String,
        instance_count: u64,
        largest_allocation_bytes: u64,
        allocation_size_bytes: u64,
    ) -> Self {
        Self {
            class_name,
            instance_count,
            largest_allocation_bytes,
            largest_object_id: 0,
            allocation_size_bytes,
        }
    }

    pub const fn with_largest_object_id(mut self, id: u64) -> Self {
        self.largest_object_id = id;
        self
    }
}

#[derive(Serialize)]
pub struct JsonResult {
    top_allocated_classes: Vec<ClassAllocationStats>,
    top_largest_instances: Vec<ClassAllocationStats>,
}

impl JsonResult {
    pub fn new(memory_usage: &mut [ClassAllocationStats], top: usize) -> Self {
        // top allocated
        memory_usage.sort_by_key(|b| std::cmp::Reverse(b.allocation_size_bytes));
        let top_allocated_classes = memory_usage.iter().take(top).cloned().collect();
        // Top largest instances
        memory_usage.sort_by_key(|b| std::cmp::Reverse(b.largest_allocation_bytes));
        let top_largest_instances = memory_usage.iter().take(top).cloned().collect();
        Self {
            top_allocated_classes,
            top_largest_instances,
        }
    }

    pub fn save_as_file(&self) -> Result<(), HprofSlurpError> {
        let file_path = format!("heaptrail-{}.json", Utc::now().timestamp_millis());
        let file = File::create(&file_path)?;
        let writer = BufWriter::new(file);
        // Serialize the struct directly to the file via the writer
        serde_json::to_writer(writer, &self)?;
        println!("Output JSON result file {file_path}");
        Ok(())
    }
}

pub struct RenderedResult {
    pub summary: String,
    pub thread_info: String,
    pub memory_usage: Vec<ClassAllocationStats>,
    pub duplicated_strings: Option<String>,
    pub captured_strings: Option<String>,
    /// Captured `AllocationSite` records (v0.8.0 feature C). Empty when the
    /// dump was not captured under allocation tracking. Consumed by the
    /// `--allocation-sites` mode renderer.
    pub allocation_sites: Vec<crate::parser::record::AllocationSite>,
    /// Number of `AllocationSites` records seen in the stream (each can
    /// carry many sites). Surfaced in the summary hint; not yet consumed
    /// programmatically — kept public for downstream JSON consumers.
    #[allow(dead_code)]
    pub allocation_sites_record_count: u32,
    /// `object_id -> ArrayPreview` for the largest primitive array of
    /// each element type. Empty when `--preview-bytes` was not set.
    /// (v0.9.0 feature B — consumed by the summary renderer to print
    /// preview lines under "Largest array instances".)
    pub array_previews: ahash::AHashMap<u64, crate::result_recorder::ArrayPreview>,
    /// `class_name -> retained_bytes` from the dominator tree.
    /// `None` unless `--retained-size` was set. (v1.0.0 feature E —
    /// consumed to add a retained column and re-sort the class table.)
    pub class_retained_by_name: Option<ahash::AHashMap<String, u64>>,
    /// Top-N (object_id, class_name, retained_bytes) by retained
    /// size, descending. `None` unless `--retained-size` was set.
    pub top_retained_instances: Option<Vec<(u64, String, u64)>>,
}

impl RenderedResult {
    pub fn serialize(self, top: usize) -> String {
        let Self {
            summary,
            thread_info,
            mut memory_usage,
            duplicated_strings,
            captured_strings,
            allocation_sites: _,
            allocation_sites_record_count: _,
            array_previews,
            class_retained_by_name,
            top_retained_instances,
        } = self;
        let memory = Self::render_memory_usage(
            &mut memory_usage,
            top,
            &array_previews,
            class_retained_by_name.as_ref(),
            top_retained_instances.as_deref(),
        );
        let mut result = format!("{summary}\n{thread_info}\n{memory}");
        if let Some(duplicated_strings) = duplicated_strings {
            writeln!(result, "{duplicated_strings}").expect("write should not fail");
        }
        if let Some(list_strings) = captured_strings {
            write!(result, "{list_strings}").expect("write should not fail");
        }
        result
    }

    fn render_memory_usage(
        memory_usage: &mut Vec<ClassAllocationStats>,
        top: usize,
        array_previews: &ahash::AHashMap<u64, crate::result_recorder::ArrayPreview>,
        class_retained_by_name: Option<&ahash::AHashMap<String, u64>>,
        top_retained_instances: Option<&[(u64, String, u64)]>,
    ) -> String {
        // Holds the final result
        let mut analysis = String::new();

        // Total heap size found banner
        let total_size = memory_usage
            .iter()
            .map(|class_allocation_stats| class_allocation_stats.allocation_size_bytes)
            .sum();
        let display_total_size = pretty_bytes_size(total_size);
        writeln!(
            analysis,
            "Found a total of {display_total_size} of raw shallow heap objects in the dump."
        )
        .expect("Could not write to analysis");

        // Top allocated classes analysis. When --retained-size is set,
        // sort by retained size and render the retained column; otherwise
        // sort by shallow as before.
        if let Some(class_retained) = class_retained_by_name {
            writeln!(
                analysis,
                "\nTop {top} classes by retained heap (dominator tree):\n"
            )
            .expect("Could not write to analysis");
            memory_usage.sort_by_key(|b| {
                std::cmp::Reverse(class_retained.get(&b.class_name).copied().unwrap_or(0))
            });
            Self::render_table_with_retained(
                top,
                &mut analysis,
                memory_usage.as_slice(),
                class_retained,
            );
        } else {
            writeln!(analysis, "\nTop {top} raw shallow heap classes:\n")
                .expect("Could not write to analysis");
            memory_usage.sort_by_key(|b| std::cmp::Reverse(b.allocation_size_bytes));
            Self::render_table(top, &mut analysis, memory_usage.as_slice());
        }

        // Top largest instances analysis
        writeln!(analysis, "\nTop {top} largest instances:\n")
            .expect("Could not write to analysis");
        memory_usage.sort_by_key(|b| std::cmp::Reverse(b.largest_allocation_bytes));
        Self::render_table(top, &mut analysis, memory_usage.as_slice());

        // Object ids of the largest array instances per class. Useful for retainer
        // tracing in a follow-up tool: "what holds the 54 MiB char[]?". Only
        // populated for array classes (primitive + object); zero/unset entries are
        // suppressed.
        let largest_with_ids: Vec<&ClassAllocationStats> = memory_usage
            .iter()
            .take(top)
            .filter(|s| s.largest_object_id != 0)
            .collect();
        if !largest_with_ids.is_empty() {
            writeln!(
                analysis,
                "\nLargest array instances object ids (for retainer tracing):"
            )
            .expect("Could not write to analysis");
            for s in &largest_with_ids {
                let display_size = pretty_bytes_size(s.largest_allocation_bytes);
                writeln!(
                    analysis,
                    "  {:>10} object_id={} {}",
                    display_size, s.largest_object_id, s.class_name
                )
                .expect("Could not write to analysis");
                // v0.9.0 (feature B): preview line for the largest
                // primitive array of this class, when --preview-bytes was
                // set. Indented two extra spaces under the class row.
                if let Some(preview) = array_previews.get(&s.largest_object_id) {
                    use crate::preview::{PreviewKind, render_preview};
                    let kind = render_preview(
                        &preview.bytes,
                        preview.element_type,
                        preview.total_bytes as usize,
                    );
                    match kind {
                        PreviewKind::Text { snippet, truncated } => {
                            let trimmed: String = snippet.chars().take(140).collect();
                            let suffix = if truncated || snippet.chars().count() > 140 {
                                "..."
                            } else {
                                ""
                            };
                            writeln!(analysis, "             {trimmed}{suffix}")
                                .expect("Could not write to analysis");
                        }
                        PreviewKind::Hex { lines, total_bytes } => {
                            writeln!(analysis, "             (binary, {total_bytes} bytes total)")
                                .expect("Could not write to analysis");
                            for line in lines.iter().take(2) {
                                writeln!(analysis, "             {line}")
                                    .expect("Could not write to analysis");
                            }
                        }
                    }
                }
            }
        }

        // v1.0.0 (feature E): "Largest retained instances" hot list,
        // mirroring the array-instances block above.
        if let Some(top_retained) = top_retained_instances
            && !top_retained.is_empty()
        {
            writeln!(analysis, "\nLargest retained instances object ids:")
                .expect("Could not write to analysis");
            for (oid, class_name, retained) in top_retained.iter().take(top) {
                let display_size = pretty_bytes_size(*retained);
                writeln!(
                    analysis,
                    "  {display_size:>10} object_id={oid} {class_name}"
                )
                .expect("Could not write to analysis");
            }
        }

        analysis
    }

    /// v1.0.0 (feature E): like `render_table` but adds a `retained`
    /// column populated from `class_retained_by_name`. Missing entries
    /// render as `0.00bytes` (which is correct — class wasn't reached
    /// from any GC root in the dominator tree).
    fn render_table_with_retained(
        top: usize,
        analysis: &mut String,
        rows: &[ClassAllocationStats],
        class_retained: &ahash::AHashMap<String, u64>,
    ) {
        let rows_formatted: Vec<_> = rows
            .iter()
            .take(top)
            .map(|s| {
                let shallow = pretty_bytes_size(s.allocation_size_bytes);
                let retained =
                    pretty_bytes_size(class_retained.get(&s.class_name).copied().unwrap_or(0));
                let largest = pretty_bytes_size(s.largest_allocation_bytes);
                (
                    shallow,
                    s.instance_count,
                    retained,
                    largest,
                    s.class_name.clone(),
                )
            })
            .collect();

        let header = ["Shallow", "Instances", "Retained", "Largest", "Class name"];
        let widths: [usize; 5] = [
            header[0]
                .len()
                .max(rows_formatted.iter().map(|r| r.0.len()).max().unwrap_or(0)),
            header[1].len().max(
                rows_formatted
                    .iter()
                    .map(|r| r.1.to_string().len())
                    .max()
                    .unwrap_or(0),
            ),
            header[2]
                .len()
                .max(rows_formatted.iter().map(|r| r.2.len()).max().unwrap_or(0)),
            header[3]
                .len()
                .max(rows_formatted.iter().map(|r| r.3.len()).max().unwrap_or(0)),
            header[4]
                .len()
                .max(rows_formatted.iter().map(|r| r.4.len()).max().unwrap_or(0)),
        ];

        let line = format!(
            "+-{:-<w0$}-+-{:-<w1$}-+-{:-<w2$}-+-{:-<w3$}-+-{:-<w4$}-+",
            "",
            "",
            "",
            "",
            "",
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4]
        );
        writeln!(analysis, "{line}").expect("write");
        writeln!(
            analysis,
            "| {:>w0$} | {:>w1$} | {:>w2$} | {:>w3$} | {:<w4$} |",
            header[0],
            header[1],
            header[2],
            header[3],
            header[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4]
        )
        .expect("write");
        writeln!(analysis, "{line}").expect("write");
        for (shallow, count, retained, largest, name) in rows_formatted {
            writeln!(
                analysis,
                "| {:>w0$} | {:>w1$} | {:>w2$} | {:>w3$} | {:<w4$} |",
                shallow,
                count,
                retained,
                largest,
                name,
                w0 = widths[0],
                w1 = widths[1],
                w2 = widths[2],
                w3 = widths[3],
                w4 = widths[4]
            )
            .expect("write");
        }
        writeln!(analysis, "{line}").expect("write");
    }

    // Render table from [(class_name, count, largest_allocation, instance_size)]
    fn render_table(top: usize, analysis: &mut String, rows: &[ClassAllocationStats]) {
        let rows_formatted: Vec<_> = rows
            .iter()
            .take(top)
            .map(|class_allocation_stats| {
                let display_allocation =
                    pretty_bytes_size(class_allocation_stats.allocation_size_bytes);
                let largest_display_allocation =
                    pretty_bytes_size(class_allocation_stats.largest_allocation_bytes);
                (
                    display_allocation,
                    class_allocation_stats.instance_count,
                    largest_display_allocation,
                    &class_allocation_stats.class_name,
                )
            })
            .collect();

        let total_size_header = "Total size";
        let total_size_header_padding = Self::padding_for_header(
            rows_formatted.as_slice(),
            |r| r.0.chars().count(),
            total_size_header,
        );
        let total_size_len =
            total_size_header.chars().count() + total_size_header_padding.chars().count();

        let instance_count_header = "Instances";
        let instance_count_header_padding = Self::padding_for_header(
            rows_formatted.as_slice(),
            |r| r.1.to_string().chars().count(),
            instance_count_header,
        );
        let instance_len =
            instance_count_header.chars().count() + instance_count_header_padding.chars().count();

        let largest_instance_header = "Largest";
        let largest_instance_padding = Self::padding_for_header(
            rows_formatted.as_slice(),
            |r| r.2.chars().count(),
            largest_instance_header,
        );
        let largest_len =
            largest_instance_header.chars().count() + largest_instance_padding.chars().count();

        let class_name_header = "Class name";
        let class_name_padding = Self::padding_for_header(
            rows_formatted.as_slice(),
            |r| r.3.chars().count(),
            class_name_header,
        );
        let class_name_len = class_name_header.chars().count() + class_name_padding.chars().count();

        // headers with padding
        let total_size_header = format!(" {total_size_header_padding}{total_size_header} ");
        let instance_count_header =
            format!(" {instance_count_header_padding}{instance_count_header} ");
        let largest_instance_header =
            format!(" {largest_instance_padding}{largest_instance_header} ",);
        let class_name_header = format!(" {class_name_header}{class_name_padding} ");

        // render line before header
        Self::render_table_vertical_line(
            analysis,
            &total_size_header,
            &instance_count_header,
            &largest_instance_header,
            &class_name_header,
        );

        // render header
        writeln!(analysis, "|{total_size_header}|{instance_count_header}|{largest_instance_header}|{class_name_header}|").expect("Could not write to analysis");

        // render line after header
        Self::render_table_vertical_line(
            analysis,
            &total_size_header,
            &instance_count_header,
            &largest_instance_header,
            &class_name_header,
        );

        // render rows
        for (allocation_size, count, largest_allocation_size, class_name) in rows_formatted {
            let padding_size_str = Self::column_padding(&allocation_size, total_size_len);
            let padding_count_str = Self::column_padding(&count.to_string(), instance_len);
            let padding_largest_size_str =
                Self::column_padding(&largest_allocation_size, largest_len);
            let padding_largest_class_name_str = Self::column_padding(class_name, class_name_len);

            writeln!(analysis, "| {padding_size_str}{allocation_size} | {padding_count_str}{count} | {padding_largest_size_str}{largest_allocation_size} | {class_name}{padding_largest_class_name_str} |").expect("Could not write to analysis");
        }

        // render line after rows
        Self::render_table_vertical_line(
            analysis,
            &total_size_header,
            &instance_count_header,
            &largest_instance_header,
            &class_name_header,
        );
    }

    fn render_table_vertical_line(
        analysis: &mut String,
        total_size_header: &str,
        instance_count_header: &str,
        largest_instance_header: &str,
        class_name_header: &str,
    ) {
        analysis.push('+');
        analysis.push_str(&("-".repeat(total_size_header.chars().count())));
        analysis.push('+');
        analysis.push_str(&("-".repeat(instance_count_header.chars().count())));
        analysis.push('+');
        analysis.push_str(&("-".repeat(largest_instance_header.chars().count())));
        analysis.push('+');
        analysis.push_str(&("-".repeat(class_name_header.chars().count())));
        analysis.push('+');
        analysis.push('\n');
    }

    fn padding_for_header<F>(
        rows: &[(String, u64, String, &String)],
        field_len: F,
        header_label: &str,
    ) -> String
    where
        F: Fn(&(String, u64, String, &String)) -> usize,
    {
        let max_elem_size = rows
            .iter()
            .map(field_len)
            .max()
            .expect("Results can't be empty");

        Self::column_padding(header_label, max_elem_size)
    }

    fn column_padding(column_name: &str, max_item_length: usize) -> String {
        let column_label_len = column_name.chars().count();
        let padding_size = max_item_length.saturating_sub(column_label_len);
        " ".repeat(padding_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_output_describes_raw_shallow_dump_objects() {
        let mut memory_usage = vec![ClassAllocationStats::new("Thing".to_string(), 1, 16, 16)];

        let output = RenderedResult::render_memory_usage(
            &mut memory_usage,
            1,
            &ahash::AHashMap::new(),
            None,
            None,
        );

        assert!(output.contains("raw shallow heap objects in the dump"));
        assert!(output.contains("Top 1 raw shallow heap classes:"));
        assert!(!output.contains("instances allocated on the heap"));
    }
}
