use std::{fmt::Write, fs::File, io::BufWriter};

use chrono::Utc;
use serde::Serialize;

use crate::{errors::HprofSlurpError, utils::pretty_bytes_size};

#[derive(Serialize, Clone)]
pub struct ClassAllocationStats {
    pub class_name: String,
    pub instance_count: u64,
    pub largest_allocation_bytes: u64,
    pub allocation_size_bytes: u64,
}

impl ClassAllocationStats {
    pub fn new(
        class_name: String,
        instance_count: u64,
        largest_allocation_bytes: u64,
        allocation_size_bytes: u64,
    ) -> Self {
        ClassAllocationStats {
            class_name,
            instance_count,
            largest_allocation_bytes,
            allocation_size_bytes,
        }
    }
}

#[derive(Serialize)]
pub struct JsonResult {
    top_allocated_classes: Vec<ClassAllocationStats>,
    top_largest_instances: Vec<ClassAllocationStats>,
}

impl JsonResult {
    pub fn new(memory_usage: &mut [ClassAllocationStats], top: usize) -> JsonResult {
        // top allocated
        memory_usage.sort_by(|a, b| b.allocation_size_bytes.cmp(&a.allocation_size_bytes));
        let top_allocated_classes = memory_usage.iter().take(top).cloned().collect();
        // Top largest instances
        memory_usage.sort_by(|a, b| b.largest_allocation_bytes.cmp(&a.largest_allocation_bytes));
        let top_largest_instances = memory_usage.iter().take(top).cloned().collect();
        JsonResult {
            top_allocated_classes,
            top_largest_instances,
        }
    }

    pub fn save_as_file(&self) -> Result<(), HprofSlurpError> {
        let file_path = format!("hprof-slurp-{}.json", Utc::now().timestamp_millis());
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
}

impl RenderedResult {
    pub fn serialize(self, top: usize) -> String {
        let RenderedResult {
            summary,
            thread_info,
            mut memory_usage,
            duplicated_strings,
            captured_strings,
        } = self;
        let memory = Self::render_memory_usage(&mut memory_usage, top);
        let mut result = format!("{summary}\n{thread_info}\n{memory}");
        if let Some(duplicated_strings) = duplicated_strings {
            writeln!(result, "{duplicated_strings}").expect("write should not fail");
        }
        if let Some(list_strings) = captured_strings {
            write!(result, "{list_strings}").expect("write should not fail");
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
            "Found a total of {display_total_size} of instances allocated on the heap."
        )
        .expect("Could not write to analysis");

        // Top allocated classes analysis
        writeln!(analysis, "\nTop {top} allocated classes:\n")
            .expect("Could not write to analysis");
        memory_usage.sort_by(|a, b| b.allocation_size_bytes.cmp(&a.allocation_size_bytes));
        Self::render_table(top, &mut analysis, memory_usage.as_slice());

        // Top largest instances analysis
        writeln!(analysis, "\nTop {top} largest instances:\n")
            .expect("Could not write to analysis");
        memory_usage.sort_by(|a, b| b.largest_allocation_bytes.cmp(&a.largest_allocation_bytes));
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
            |r| r.0.to_string(),
            total_size_header,
        );
        let total_size_len =
            total_size_header.chars().count() + total_size_header_padding.chars().count();

        let instance_count_header = "Instances";
        let instance_count_header_padding = Self::padding_for_header(
            rows_formatted.as_slice(),
            |r| r.1.to_string(),
            instance_count_header,
        );
        let instance_len =
            instance_count_header.chars().count() + instance_count_header_padding.chars().count();

        let largest_instance_header = "Largest";
        let largest_instance_padding = Self::padding_for_header(
            rows_formatted.as_slice(),
            |r| r.2.to_string(),
            largest_instance_header,
        );
        let largest_len =
            largest_instance_header.chars().count() + largest_instance_padding.chars().count();

        let class_name_header = "Class name";
        let class_name_padding = Self::padding_for_header(
            rows_formatted.as_slice(),
            |r| r.3.to_string(),
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

    pub fn render_table_vertical_line(
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
        field_selector: F,
        header_label: &str,
    ) -> String
    where
        F: Fn(&(String, u64, String, &String)) -> String,
    {
        let max_elem_size = rows
            .iter()
            .map(|d| field_selector(d).chars().count())
            .max_by(std::cmp::Ord::cmp)
            .expect("Results can't be empty");

        Self::column_padding(header_label, max_elem_size)
    }

    fn column_padding(column_name: &str, max_item_length: usize) -> String {
        let column_label_len = column_name.chars().count();
        let padding_size = max_item_length.saturating_sub(column_label_len);
        " ".repeat(padding_size)
    }
}
