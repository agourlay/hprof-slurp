use ahash::AHashMap;
use indoc::formatdoc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::thread::JoinHandle;

use crate::gc_record::*;
use crate::record::Record;
use crate::record::Record::*;
use crate::utils::pretty_bytes_size;

#[derive(Debug, Copy, Clone)]
pub struct ClassInstanceCounter {
    number_of_instance: u64,
    max_size_seen: u64,
    total_size: u64,
}

impl ClassInstanceCounter {
    pub fn add_instance(&mut self, size: u64) {
        self.number_of_instance += 1;
        self.total_size += size;
        if size > self.max_size_seen {
            self.max_size_seen = size
        }
    }

    pub fn empty() -> ClassInstanceCounter {
        ClassInstanceCounter {
            number_of_instance: 0,
            total_size: 0,
            max_size_seen: 0,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ArrayCounter {
    number_of_arrays: u64,
    max_size_seen: u32,
    total_number_of_elements: u64,
}

impl ArrayCounter {
    pub fn add_elements_from_array(&mut self, elements: u32) {
        self.number_of_arrays += 1;
        self.total_number_of_elements += elements as u64;
        if elements > self.max_size_seen {
            self.max_size_seen = elements
        }
    }

    pub fn empty() -> ArrayCounter {
        ArrayCounter {
            number_of_arrays: 0,
            total_number_of_elements: 0,
            max_size_seen: 0,
        }
    }
}

pub struct RenderedResult {
    pub summary: String,
    pub analysis: String,
    pub captured_strings: Option<String>,
}

pub struct ResultRecorder {
    id_size: u32,
    list_strings: bool,
    top: usize,
    // Tag counters
    classes_unloaded: i32,
    stack_frames: i32,
    stack_traces: i32,
    start_threads: i32,
    end_threads: i32,
    heap_summaries: i32,
    heap_dumps: i32,
    allocation_sites: i32,
    control_settings: i32,
    cpu_samples: i32,
    // GC tag counters
    heap_dump_segments_all_sub_records: i32,
    heap_dump_segments_gc_root_unknown: i32,
    heap_dump_segments_gc_root_thread_object: i32,
    heap_dump_segments_gc_root_jni_global: i32,
    heap_dump_segments_gc_root_jni_local: i32,
    heap_dump_segments_gc_root_java_frame: i32,
    heap_dump_segments_gc_root_native_stack: i32,
    heap_dump_segments_gc_root_sticky_class: i32,
    heap_dump_segments_gc_root_thread_block: i32,
    heap_dump_segments_gc_root_monitor_used: i32,
    heap_dump_segments_gc_object_array_dump: i32,
    heap_dump_segments_gc_primitive_array_dump: i32,
    heap_dump_segments_gc_class_dump: i32,
    // Captured state
    // "object_id" -> "class_id" -> "class_name_id" -> "utf8_string"
    utf8_strings_by_id: AHashMap<u64, String>,
    classes_loaded_by_id: AHashMap<u64, u64>,
    classes_single_instance_size_by_id: AHashMap<u64, u32>,
    classes_all_instance_total_size_by_id: AHashMap<u64, ClassInstanceCounter>,
    primitive_array_counters: AHashMap<FieldType, ArrayCounter>,
    object_array_counters: AHashMap<u64, ArrayCounter>,
}

impl ResultRecorder {
    pub fn new(id_size: u32, list_strings: bool, top: usize) -> Self {
        ResultRecorder {
            id_size,
            list_strings,
            top,
            classes_unloaded: 0,
            stack_frames: 0,
            stack_traces: 0,
            start_threads: 0,
            end_threads: 0,
            heap_summaries: 0,
            heap_dumps: 0,
            allocation_sites: 0,
            control_settings: 0,
            cpu_samples: 0,
            heap_dump_segments_all_sub_records: 0,
            heap_dump_segments_gc_root_unknown: 0,
            heap_dump_segments_gc_root_thread_object: 0,
            heap_dump_segments_gc_root_jni_global: 0,
            heap_dump_segments_gc_root_jni_local: 0,
            heap_dump_segments_gc_root_java_frame: 0,
            heap_dump_segments_gc_root_native_stack: 0,
            heap_dump_segments_gc_root_sticky_class: 0,
            heap_dump_segments_gc_root_thread_block: 0,
            heap_dump_segments_gc_root_monitor_used: 0,
            heap_dump_segments_gc_object_array_dump: 0,
            heap_dump_segments_gc_primitive_array_dump: 0,
            heap_dump_segments_gc_class_dump: 0,
            utf8_strings_by_id: AHashMap::new(),
            classes_loaded_by_id: AHashMap::new(),
            classes_single_instance_size_by_id: AHashMap::new(),
            classes_all_instance_total_size_by_id: AHashMap::new(),
            primitive_array_counters: AHashMap::new(),
            object_array_counters: AHashMap::new(),
        }
    }

    fn get_class_name_string(&self, class_id: &u64) -> String {
        self.classes_loaded_by_id
            .get(class_id)
            .and_then(|class_id| self.utf8_strings_by_id.get(class_id))
            .expect("class_id must have an UTF-8 string representation available")
            .to_owned()
    }

    pub fn start_recorder(
        mut self,
        rx: Receiver<Vec<Record>>,
        tx: Sender<RenderedResult>,
    ) -> JoinHandle<()> {
        thread::Builder::new()
            .name("hprof-recorder".to_string())
            .spawn(move || {
                loop {
                    let records = rx.recv().expect("channel should not be closed");
                    if records.is_empty() {
                        // empty Vec means we are done
                        break;
                    } else {
                        self.record_records(records)
                    }
                }
                // no more Record to pull, generate and send back results
                let rendered_result = RenderedResult {
                    summary: self.render_summary(),
                    analysis: self.render_analysis(self.top),
                    captured_strings: if self.list_strings {
                        Some(self.render_captured_strings())
                    } else {
                        None
                    },
                };
                tx.send(rendered_result)
                    .expect("channel should not be closed");
            })
            .unwrap()
    }

    fn record_records(&mut self, records: Vec<Record>) {
        records.into_iter().for_each(|record| {
            match record {
                Utf8String { id, str } => {
                    self.utf8_strings_by_id.insert(id, str);
                }
                LoadClass {
                    class_object_id,
                    class_name_id,
                    ..
                } => {
                    self.classes_loaded_by_id
                        .insert(class_object_id, class_name_id);
                }
                UnloadClass { .. } => self.classes_unloaded += 1,
                StackFrame { .. } => self.stack_frames += 1,
                StackTrace { .. } => self.stack_traces += 1,
                StartThread { .. } => self.start_threads += 1,
                EndThread { .. } => self.end_threads += 1,
                AllocationSites { .. } => self.allocation_sites += 1,
                HeapSummary { .. } => self.heap_summaries += 1,
                ControlSettings { .. } => self.control_settings += 1,
                CpuSamples { .. } => self.cpu_samples += 1,
                HeapDumpEnd { .. } => (),
                HeapDumpStart { .. } => self.heap_dumps += 1,
                GcSegment(gc_record) => {
                    self.heap_dump_segments_all_sub_records += 1;
                    match gc_record {
                        GcRecord::GcRootUnknown { .. } => {
                            self.heap_dump_segments_gc_root_unknown += 1
                        }
                        GcRecord::GcRootThreadObject { .. } => {
                            self.heap_dump_segments_gc_root_thread_object += 1
                        }
                        GcRecord::GcRootJniGlobal { .. } => {
                            self.heap_dump_segments_gc_root_jni_global += 1
                        }
                        GcRecord::GcRootJniLocal { .. } => {
                            self.heap_dump_segments_gc_root_jni_local += 1
                        }
                        GcRecord::GcRootJavaFrame { .. } => {
                            self.heap_dump_segments_gc_root_java_frame += 1
                        }
                        GcRecord::GcRootNativeStack { .. } => {
                            self.heap_dump_segments_gc_root_native_stack += 1
                        }
                        GcRecord::GcRootStickyClass { .. } => {
                            self.heap_dump_segments_gc_root_sticky_class += 1
                        }
                        GcRecord::GcRootThreadBlock { .. } => {
                            self.heap_dump_segments_gc_root_thread_block += 1
                        }
                        GcRecord::GcRootMonitorUsed { .. } => {
                            self.heap_dump_segments_gc_root_monitor_used += 1
                        }
                        GcRecord::GcInstanceDump {
                            class_object_id,
                            data_size,
                            ..
                        } => {
                            // no need to perform a lookup in `classes_instance_size_by_id`
                            // data_size is available in the record
                            // total_size = data_size + id_size + mark(4) + padding(4)
                            self.classes_all_instance_total_size_by_id
                                .entry(class_object_id)
                                .or_insert_with(ClassInstanceCounter::empty)
                                .add_instance((data_size + self.id_size + 8) as u64);
                        }
                        GcRecord::GcObjectArrayDump {
                            number_of_elements,
                            array_class_id,
                            ..
                        } => {
                            self.object_array_counters
                                .entry(array_class_id)
                                .or_insert_with(ArrayCounter::empty)
                                .add_elements_from_array(number_of_elements);

                            self.heap_dump_segments_gc_object_array_dump += 1
                        }
                        GcRecord::GcPrimitiveArrayDump {
                            number_of_elements,
                            element_type,
                            ..
                        } => {
                            self.primitive_array_counters
                                .entry(element_type)
                                .or_insert_with(ArrayCounter::empty)
                                .add_elements_from_array(number_of_elements);

                            self.heap_dump_segments_gc_primitive_array_dump += 1
                        }
                        GcRecord::GcClassDump {
                            class_object_id,
                            instance_size,
                            ..
                        } => {
                            // Unused for now, remove it???
                            self.classes_single_instance_size_by_id
                                .entry(class_object_id)
                                .or_insert(instance_size);

                            self.heap_dump_segments_gc_class_dump += 1
                        }
                    }
                }
            }
        });
    }

    fn render_captured_strings(&self) -> String {
        let mut strings: Vec<_> = self.utf8_strings_by_id.values().collect();
        strings.sort();
        let mut result = String::new();
        result.push_str("\nList of Strings\n");
        strings.iter().for_each(|s| {
            result.push_str(s);
            result.push('\n')
        });
        result
    }

    fn render_analysis(&self, top: usize) -> String {
        let mut classes_dump_vec: Vec<_> = self
            .classes_all_instance_total_size_by_id
            .iter()
            .map(|(class_id, v)| {
                let class_name = self.get_class_name_string(class_id);
                (
                    class_name,
                    v.number_of_instance,
                    v.max_size_seen,
                    v.total_size,
                )
            })
            .collect();

        // https://www.baeldung.com/java-memory-layout
        // the array's `elements` size are already computed via `GcInstanceDump`
        // here we are interested in the total size of the array headers and outgoing elements references
        let ref_size = self.id_size as u64;
        let array_header_size = ref_size + 4 + 4; // 4 bytes of klass + 4 bytes for the array length.

        let array_primitives_dump_vec = self.primitive_array_counters.iter().map(|(ft, &ac)| {
            let primitive_array_label = format!("{:?}[]", ft);
            let cost_of_all_array_headers = array_header_size * ac.number_of_arrays;
            let cost_of_all_values = primitive_byte_size(ft) * ac.total_number_of_elements;
            let cost_of_biggest_array = primitive_byte_size(ft) * ac.max_size_seen as u64;
            (
                primitive_array_label,
                ac.number_of_arrays,
                cost_of_biggest_array,
                cost_of_all_array_headers + cost_of_all_values,
            )
        });

        let array_objects_dump_vec = self.object_array_counters.iter().map(|(class_id, &ac)| {
            let raw_class_name = self.get_class_name_string(class_id);
            // remove '[L' prefix and ';' suffix
            let cleaned_class_name: String = raw_class_name
                .chars()
                .skip(2)
                .take(raw_class_name.chars().count() - 3)
                .collect();
            let class_name = format!("{}[]", cleaned_class_name);

            let cost_of_all_refs = ref_size * ac.total_number_of_elements;
            let cost_of_all_array_headers = array_header_size * ac.number_of_arrays;

            let cost_of_biggest_array_refs = ref_size * ac.max_size_seen as u64;
            let cost_of_biggest_array_header = array_header_size;

            (
                class_name,
                ac.number_of_arrays,
                cost_of_biggest_array_refs + cost_of_biggest_array_header,
                cost_of_all_array_headers + cost_of_all_refs,
            )
        });

        // Merge results
        classes_dump_vec.extend(array_primitives_dump_vec);
        classes_dump_vec.extend(array_objects_dump_vec);
        // reverse sort by size
        classes_dump_vec.sort_by(|a, b| b.3.cmp(&a.3));

        let total_size = classes_dump_vec.iter().map(|(_, _, _, s)| *s as u64).sum();
        let display_total_size = pretty_bytes_size(total_size);

        let mut analysis = String::new();
        let title = format!(
            "\nTop {} allocations for the {} heap total size:\n\n",
            top, display_total_size
        );
        analysis.push_str(&title);

        let rows_formatted: Vec<_> = classes_dump_vec
            .into_iter()
            .take(top)
            .map(|(class_name, count, biggest_allocation, allocation_size)| {
                let display_allocation = pretty_bytes_size(allocation_size as u64);
                let biggest_display_allocation = pretty_bytes_size(biggest_allocation as u64);
                (
                    display_allocation,
                    count,
                    biggest_display_allocation,
                    class_name,
                )
            })
            .collect();

        let total_size_header = "Total size";
        let total_size_header_padding = ResultRecorder::padding_for_header(
            &rows_formatted,
            |r| r.0.to_string(),
            total_size_header,
        );
        let total_size_len =
            total_size_header.chars().count() + total_size_header_padding.chars().count();

        let instance_count_header = "Instances";
        let instance_count_header_padding = ResultRecorder::padding_for_header(
            &rows_formatted,
            |r| r.1.to_string(),
            instance_count_header,
        );
        let instance_len =
            instance_count_header.chars().count() + instance_count_header_padding.chars().count();

        let biggest_instance_header = "Largest";
        let biggest_instance_padding = ResultRecorder::padding_for_header(
            &rows_formatted,
            |r| r.2.to_string(),
            biggest_instance_header,
        );
        let biggest_len =
            biggest_instance_header.chars().count() + biggest_instance_padding.chars().count();

        let class_name_header = "Class name";
        let class_name_padding = ResultRecorder::padding_for_header(
            &rows_formatted,
            |r| r.3.to_string(),
            class_name_header,
        );

        let header = format!(
            "{}{} | {}{} | {}{} | {}{}\n",
            total_size_header_padding,
            total_size_header,
            instance_count_header_padding,
            instance_count_header,
            biggest_instance_padding,
            biggest_instance_header,
            class_name_header,
            class_name_padding
        );
        let header_len = header.chars().count();
        analysis.push_str(&header);
        analysis.push_str(&("-".repeat(header_len)));
        analysis.push('\n');

        rows_formatted.into_iter().for_each(
            |(allocation_size, count, biggest_allocation_size, class_name)| {
                let padding_size_str =
                    ResultRecorder::column_padding(&allocation_size, total_size_len);
                let padding_count_str =
                    ResultRecorder::column_padding(&count.to_string(), instance_len);
                let padding_biggest_size_str =
                    ResultRecorder::column_padding(&biggest_allocation_size, biggest_len);

                let row = format!(
                    "{}{} | {}{} | {}{} | {}\n",
                    padding_size_str,
                    allocation_size,
                    padding_count_str,
                    count,
                    padding_biggest_size_str,
                    biggest_allocation_size,
                    class_name
                );
                analysis.push_str(&row);
            },
        );
        analysis
    }

    fn padding_for_header<F>(
        rows: &[(String, u64, String, String)],
        field_selector: F,
        header_label: &str,
    ) -> String
    where
        F: Fn(&(String, u64, String, String)) -> String,
    {
        let max_elem_size = rows
            .iter()
            .map(|d| field_selector(d).chars().count())
            .max_by(|x, y| x.cmp(y))
            .expect("Results can't be empty");

        ResultRecorder::column_padding(header_label, max_elem_size)
    }

    fn column_padding(column_name: &str, max_item_length: usize) -> String {
        let column_label_len = column_name.chars().count();
        let padding_size = if max_item_length > column_label_len {
            max_item_length - column_label_len
        } else {
            0
        };
        " ".repeat(padding_size)
    }

    pub fn render_summary(&self) -> String {
        let top_summary = formatdoc!(
            "\nFile content summary:\n
            UTF-8 Strings: {}
            Classes loaded: {}
            Classes unloaded: {}
            Stack traces: {}
            Stack frames: {}
            Start threads: {}
            Allocation sites: {}
            End threads: {}
            Control settings: {}
            CPU samples: {}",
            self.utf8_strings_by_id.len(),
            self.classes_loaded_by_id.len(),
            self.classes_unloaded,
            self.stack_traces,
            self.stack_frames,
            self.start_threads,
            self.allocation_sites,
            self.end_threads,
            self.control_settings,
            self.cpu_samples
        );

        let heap_summary = formatdoc!(
            "Heap summaries: {}
            {} heap dumps containing in total {} segments:
            ..GC root unknown: {}
            ..GC root thread objects: {}
            ..GC root JNI global: {}
            ..GC root JNI local: {}
            ..GC root Java frame: {}
            ..GC root native stack: {}
            ..GC root sticky class: {}
            ..GC root thread block: {}
            ..GC root monitor used: {}
            ..GC primitive array dump: {}
            ..GC object array dump: {}
            ..GC root class dump: {}
            ..GC root instance dump: {}",
            self.heap_summaries,
            self.heap_dumps,
            self.heap_dump_segments_all_sub_records,
            self.heap_dump_segments_gc_root_unknown,
            self.heap_dump_segments_gc_root_thread_object,
            self.heap_dump_segments_gc_root_jni_global,
            self.heap_dump_segments_gc_root_jni_local,
            self.heap_dump_segments_gc_root_java_frame,
            self.heap_dump_segments_gc_root_native_stack,
            self.heap_dump_segments_gc_root_sticky_class,
            self.heap_dump_segments_gc_root_thread_block,
            self.heap_dump_segments_gc_root_monitor_used,
            self.heap_dump_segments_gc_primitive_array_dump,
            self.heap_dump_segments_gc_object_array_dump,
            self.heap_dump_segments_gc_class_dump,
            self.classes_all_instance_total_size_by_id.len()
        );

        format!("{}\n{}", top_summary, heap_summary)
    }
}

fn primitive_byte_size(field_type: &FieldType) -> u64 {
    match field_type {
        FieldType::Byte | FieldType::Bool => 1,
        FieldType::Char | FieldType::Short => 2,
        FieldType::Float | FieldType::Int => 4,
        FieldType::Double | FieldType::Long => 8,
        FieldType::Object => panic!("object type in primitive array"),
    }
}
