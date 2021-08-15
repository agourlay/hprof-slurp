use std::collections::HashMap;
use std::sync::mpsc::Receiver;
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
    utf8_strings_by_id: HashMap<u64, String>,
    classes_loaded_by_id: HashMap<u64, u64>,
    classes_single_instance_size_by_id: HashMap<u64, u32>,
    classes_all_instance_total_size_by_id: HashMap<u64, ClassInstanceCounter>,
    primitive_array_counters: HashMap<FieldType, ArrayCounter>,
    object_array_counters: HashMap<u64, ArrayCounter>,
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
            utf8_strings_by_id: HashMap::new(),
            classes_loaded_by_id: HashMap::new(),
            classes_single_instance_size_by_id: HashMap::new(),
            classes_all_instance_total_size_by_id: HashMap::new(),
            primitive_array_counters: HashMap::new(),
            object_array_counters: HashMap::new(),
        }
    }

    fn get_class_name_string(&self, class_id: &u64) -> String {
        self.classes_loaded_by_id
            .get(class_id)
            .and_then(|class_id| self.utf8_strings_by_id.get(class_id))
            .expect("class_id must have an UTF-8 string representation available")
            .to_owned()
    }

    pub fn start_recorder(mut self, rx: Receiver<Vec<Record>>) -> JoinHandle<()> {
        thread::spawn(move || {
            loop {
                let records = rx.recv().expect("channel should not be closed");
                if records.is_empty() {
                    // empty Vec means we are done
                    break;
                } else {
                    self.record_records(records)
                }
            }
            // nothing more to pull, print results
            self.print_summary();
            self.print_analysis(self.top);

            if self.list_strings {
                self.print_strings()
            }
        })
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

    fn print_strings(&self) {
        let mut strings: Vec<_> = self.utf8_strings_by_id.values().collect();
        strings.sort();
        println!();
        println!("List of Strings");
        strings.iter().for_each(|s| println!("{}", s));
    }

    fn print_analysis(&self, top: usize) {
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

        println!();
        println!(
            "Top {} allocations for the {} heap total size:",
            top, display_total_size
        );
        println!();

        // TODO print table generically instead of this mess
        let all_formatted: Vec<_> = classes_dump_vec
            .iter()
            .take(top)
            .map(|(class_name, count, biggest_allocation, allocation_size)| {
                let display_allocation = pretty_bytes_size(*allocation_size as u64);
                let allocation_str_len = display_allocation.chars().count();

                let biggest_display_allocation = pretty_bytes_size(*biggest_allocation as u64);
                let biggest_allocation_str_len = biggest_display_allocation.chars().count();

                let class_name_str_len = class_name.chars().count();
                (
                    display_allocation,
                    allocation_str_len,
                    count,
                    biggest_display_allocation,
                    biggest_allocation_str_len,
                    class_name_str_len,
                    class_name,
                )
            })
            .collect();

        let total_size_header = "Total size";
        let max_length_size_label = {
            let max_element_length_size_label = all_formatted
                .iter()
                .max_by(|(_, l1, _, _, _, _, _), (_, l2, _, _, _, _, _)| l1.cmp(l2))
                .expect("Results can't be empty")
                .1;
            total_size_header
                .chars()
                .count()
                .max(max_element_length_size_label)
        };
        let total_size_header_padding =
            " ".repeat(max_length_size_label - total_size_header.chars().count());

        let instance_count_header = "Instances";
        let max_count_size_label = {
            let max_element_count_size = all_formatted
                .iter()
                .max_by(|(_, _, l1, _, _, _, _), (_, _, l2, _, _, _, _)| l1.cmp(l2))
                .expect("Results can't be empty")
                .2;
            instance_count_header
                .chars()
                .count()
                .max(max_element_count_size.to_string().chars().count())
        };
        let instance_count_header_padding =
            " ".repeat(max_count_size_label - instance_count_header.chars().count());

        let biggest_instance_header = "Largest";
        let max_biggest_length_size_label = {
            let max_element_biggest_length_size_label = all_formatted
                .iter()
                .max_by(|(_, _, _, _, l1, _, _), (_, _, _, _, l2, _, _)| l1.cmp(l2))
                .expect("Results can't be empty")
                .4;
            biggest_instance_header
                .chars()
                .count()
                .max(max_element_biggest_length_size_label)
        };
        let biggest_instance_padding =
            " ".repeat(max_biggest_length_size_label - biggest_instance_header.chars().count());

        let class_name_header = "Class name";
        let class_name_padding_size = {
            let longest_class_name = all_formatted
                .iter()
                .max_by(|(_, _, _, _, _, l1, _), (_, _, _, _, _, l2, _)| l1.cmp(l2))
                .expect("Results can't be empty")
                .5;
            class_name_header.chars().count().max(longest_class_name)
        };
        let class_name_padding =
            " ".repeat(class_name_padding_size - class_name_header.chars().count());

        let header = format!(
            "{}{} | {}{} | {}{} | {}{}",
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
        println!("{}", header);
        println!("{}", "-".repeat(header_len));

        all_formatted.iter().for_each(
            |(
                allocation_size,
                allocation_str_len,
                count,
                biggest_allocation_size,
                biggest_allocation_str_len,
                _,
                class_name,
            )| {
                let padding_size = max_length_size_label - allocation_str_len;
                let padding_size_str = " ".repeat(padding_size);

                let padding_count = max_count_size_label - count.to_string().chars().count();
                let padding_count_str = " ".repeat(padding_count);

                let padding_biggest_size =
                    max_biggest_length_size_label - biggest_allocation_str_len;
                let padding_biggest_size_str = " ".repeat(padding_biggest_size);

                println!(
                    "{}{} | {}{} | {}{} | {}",
                    padding_size_str,
                    allocation_size,
                    padding_count_str,
                    count,
                    padding_biggest_size_str,
                    biggest_allocation_size,
                    class_name
                );
            },
        );
    }

    pub fn print_summary(&self) {
        println!();
        println!("File content summary:");
        println!();
        println!("UTF-8 Strings: {}", self.utf8_strings_by_id.len());
        println!("Classes loaded: {}", self.classes_loaded_by_id.len());
        println!("Classes unloaded: {}", self.classes_unloaded);
        println!("Stack traces: {}", self.stack_traces);
        println!("Stack frames: {}", self.stack_frames);
        println!("Start threads: {}", self.start_threads);
        println!("Allocation sites: {}", self.allocation_sites);
        println!("End threads: {}", self.end_threads);
        println!("Control settings: {}", self.control_settings);
        println!("CPU samples: {}", self.cpu_samples);
        println!("Heap summaries: {}", self.heap_summaries);
        println!(
            "{} heap dumps containing in total {} segments:",
            self.heap_dumps, self.heap_dump_segments_all_sub_records
        );
        println!(
            "..GC root unknown: {}",
            self.heap_dump_segments_gc_root_unknown
        );
        println!(
            "..GC root thread objects: {}",
            self.heap_dump_segments_gc_root_thread_object
        );
        println!(
            "..GC root JNI global: {}",
            self.heap_dump_segments_gc_root_jni_global
        );
        println!(
            "..GC root JNI local: {}",
            self.heap_dump_segments_gc_root_jni_local
        );
        println!(
            "..GC root Java frame: {}",
            self.heap_dump_segments_gc_root_java_frame
        );
        println!(
            "..GC root native stack: {}",
            self.heap_dump_segments_gc_root_native_stack
        );
        println!(
            "..GC root sticky class: {}",
            self.heap_dump_segments_gc_root_sticky_class
        );
        println!(
            "..GC root thread block: {}",
            self.heap_dump_segments_gc_root_thread_block
        );
        println!(
            "..GC root monitor used: {}",
            self.heap_dump_segments_gc_root_monitor_used
        );
        println!(
            "..GC primitive array dump: {}",
            self.heap_dump_segments_gc_primitive_array_dump
        );
        println!(
            "..GC object array dump: {}",
            self.heap_dump_segments_gc_object_array_dump
        );
        println!(
            "..GC root class dump: {}",
            self.heap_dump_segments_gc_class_dump
        );
        println!(
            "..GC root instance dump: {}",
            self.classes_all_instance_total_size_by_id.len()
        );
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
