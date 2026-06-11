use std::time::{SystemTime, UNIX_EPOCH};
use std::{fmt::Write, fs::File, io::BufWriter};

use serde::Serialize;

use crate::{
    errors::HprofSlurpError,
    utils::{pretty_bytes_size, pretty_timestamp_utc},
};

#[derive(Serialize, Clone)]
pub struct ClassAllocationStats {
    pub class_name: String,
    pub instance_count: u64,
    pub largest_allocation_bytes: u64,
    pub allocation_size_bytes: u64,
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
            allocation_size_bytes,
        }
    }
}

// Bump on any breaking change of the JSON output structure.
const JSON_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
struct ToolInfo {
    name: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
pub struct DumpInfo {
    file: String,
    file_size_bytes: u64,
    format: String,
    id_size_bytes: u32,
    captured_at_epoch_millis: Option<u64>,
    captured_at_utc: Option<String>,
}

impl DumpInfo {
    pub fn new(
        file: String,
        file_size_bytes: u64,
        format: String,
        id_size_bytes: u32,
        timestamp_epoch_millis: u64,
    ) -> Self {
        // `0` means the dumper did not record a capture time
        let captured_at_epoch_millis =
            (timestamp_epoch_millis != 0).then_some(timestamp_epoch_millis);
        let captured_at_utc = captured_at_epoch_millis.map(pretty_timestamp_utc);
        Self {
            file,
            file_size_bytes,
            format,
            id_size_bytes,
            captured_at_epoch_millis,
            captured_at_utc,
        }
    }
}

#[derive(Serialize)]
struct HeapInfo {
    total_shallow_bytes: u64,
    class_count: usize,
    top_allocated_classes: Vec<ClassAllocationStats>,
    top_largest_instances: Vec<ClassAllocationStats>,
}

#[derive(Serialize)]
pub struct JsonResult {
    schema_version: u32,
    tool: ToolInfo,
    dump: DumpInfo,
    heap: HeapInfo,
}

impl JsonResult {
    pub fn new(dump: DumpInfo, memory_usage: &mut [ClassAllocationStats], top: usize) -> Self {
        // totals over all classes, not only the top entries
        let total_shallow_bytes = memory_usage
            .iter()
            .map(|stats| stats.allocation_size_bytes)
            .sum();
        let class_count = memory_usage.len();
        // top allocated
        memory_usage.sort_by_key(|b| std::cmp::Reverse(b.allocation_size_bytes));
        let top_allocated_classes = memory_usage.iter().take(top).cloned().collect();
        // Top largest instances
        memory_usage.sort_by_key(|b| std::cmp::Reverse(b.largest_allocation_bytes));
        let top_largest_instances = memory_usage.iter().take(top).cloned().collect();
        Self {
            schema_version: JSON_SCHEMA_VERSION,
            tool: ToolInfo {
                name: env!("CARGO_PKG_NAME"),
                version: env!("CARGO_PKG_VERSION"),
            },
            dump,
            heap: HeapInfo {
                total_shallow_bytes,
                class_count,
                top_allocated_classes,
                top_largest_instances,
            },
        }
    }

    pub fn save_as_file(&self, output_path: Option<&str>) -> Result<(), HprofSlurpError> {
        let file_path = output_path.map_or_else(
            || {
                let millis = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock should be set after 1970")
                    .as_millis();
                format!("hprof-slurp-{millis}.json")
            },
            str::to_string,
        );
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
    pub warnings: Option<String>,
}

impl RenderedResult {
    pub fn serialize(self, top: usize) -> String {
        let Self {
            summary,
            thread_info,
            mut memory_usage,
            duplicated_strings,
            captured_strings,
            warnings,
        } = self;
        let memory = Self::render_memory_usage(&mut memory_usage, top);
        let mut result = format!("{summary}\n{thread_info}\n{memory}");
        if let Some(duplicated_strings) = duplicated_strings {
            writeln!(result, "{duplicated_strings}").expect("write should not fail");
        }
        if let Some(list_strings) = captured_strings {
            write!(result, "{list_strings}").expect("write should not fail");
        }
        // last so it stays visible even when `--list-strings` floods the output
        if let Some(warnings) = warnings {
            write!(result, "{warnings}").expect("write should not fail");
        }
        result
    }

    fn render_memory_usage(memory_usage: &mut Vec<ClassAllocationStats>, top: usize) -> String {
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

        // Top allocated classes analysis
        writeln!(analysis, "\nTop {top} raw shallow heap classes:\n")
            .expect("Could not write to analysis");
        memory_usage.sort_by_key(|b| std::cmp::Reverse(b.allocation_size_bytes));
        Self::render_table(top, &mut analysis, memory_usage.as_slice());

        // Top largest instances analysis
        writeln!(analysis, "\nTop {top} largest instances:\n")
            .expect("Could not write to analysis");
        memory_usage.sort_by_key(|b| std::cmp::Reverse(b.largest_allocation_bytes));
        Self::render_table(top, &mut analysis, memory_usage.as_slice());

        analysis
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

        let output = RenderedResult::render_memory_usage(&mut memory_usage, 1);

        assert!(output.contains("raw shallow heap objects in the dump"));
        assert!(output.contains("Top 1 raw shallow heap classes:"));
        assert!(!output.contains("instances allocated on the heap"));
    }

    #[test]
    fn json_result_includes_metadata_and_totals() {
        let mut memory_usage = vec![
            ClassAllocationStats::new("A".to_string(), 1, 16, 16),
            ClassAllocationStats::new("B".to_string(), 2, 8, 24),
        ];
        let dump_info = DumpInfo::new(
            "heap.hprof".to_string(),
            1234,
            "JAVA PROFILE 1.0.1".to_string(),
            8,
            1_608_192_273_831,
        );

        let json_result = JsonResult::new(dump_info, &mut memory_usage, 1);
        let json = serde_json::to_value(&json_result).expect("should serialize");

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["tool"]["name"], "hprof-slurp");
        assert_eq!(json["dump"]["file"], "heap.hprof");
        assert_eq!(json["dump"]["file_size_bytes"], 1234);
        assert_eq!(json["dump"]["format"], "JAVA PROFILE 1.0.1");
        assert_eq!(json["dump"]["id_size_bytes"], 8);
        assert_eq!(
            json["dump"]["captured_at_epoch_millis"],
            1_608_192_273_831_u64
        );
        assert_eq!(json["dump"]["captured_at_utc"], "2020-12-17 08:04:33 UTC");
        // totals cover all classes while the top lists are truncated
        assert_eq!(json["heap"]["total_shallow_bytes"], 40);
        assert_eq!(json["heap"]["class_count"], 2);
        assert_eq!(
            json["heap"]["top_allocated_classes"]
                .as_array()
                .expect("should be an array")
                .len(),
            1
        );
    }

    #[test]
    fn json_capture_time_is_null_when_absent() {
        let dump_info = DumpInfo::new("heap.hprof".to_string(), 1, "F".to_string(), 4, 0);

        let json = serde_json::to_value(&dump_info).expect("should serialize");

        assert!(json["captured_at_epoch_millis"].is_null());
        assert!(json["captured_at_utc"].is_null());
    }

    #[test]
    fn serialize_appends_warnings_last() {
        let rendered_result = RenderedResult {
            summary: "summary".to_string(),
            thread_info: "threads".to_string(),
            memory_usage: vec![ClassAllocationStats::new("Thing".to_string(), 1, 16, 16)],
            duplicated_strings: None,
            captured_strings: Some("strings".to_string()),
            warnings: Some("\nWarning: something was off\n".to_string()),
        };

        let output = rendered_result.serialize(1);

        assert!(output.ends_with("\nWarning: something was off\n"));
    }
}
