use ahash::AHashMap;
use crossbeam_channel::{Receiver, Sender};
use indoc::formatdoc;
use std::fmt::Write;
use std::thread::JoinHandle;
use std::{mem, thread};

use crate::parser::gc_record::{FieldType, GcRecord};
use crate::parser::record::Record::{
    AllocationSites, ControlSettings, CpuSamples, EndThread, GcSegment, HeapDumpEnd, HeapDumpStart,
    HeapSummary, LoadClass, StackFrame, StackTrace, StartThread, UnloadClass, Utf8String,
};
use crate::parser::record::{LoadClassData, Record, StackFrameData, StackTraceData};
use crate::rendered_result::{ClassAllocationStats, RenderedResult};

#[derive(Debug, Copy, Clone)]
pub struct ClassInfo {
    super_class_object_id: u64,
    instance_size: u32,
}

impl ClassInfo {
    const fn new(super_class_object_id: u64, instance_size: u32) -> Self {
        Self {
            super_class_object_id,
            instance_size,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ClassInstanceCounter {
    number_of_instances: u64,
}

impl ClassInstanceCounter {
    pub const fn add_instance(&mut self) {
        self.number_of_instances += 1;
    }

    pub const fn empty() -> Self {
        Self {
            number_of_instances: 0,
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
        self.total_number_of_elements += u64::from(elements);
        if elements > self.max_size_seen {
            self.max_size_seen = elements;
        }
    }

    pub const fn empty() -> Self {
        Self {
            number_of_arrays: 0,
            total_number_of_elements: 0,
            max_size_seen: 0,
        }
    }
}

pub struct ResultRecorder {
    // Recorder's params
    id_size: u32,
    list_strings: bool,
    // Tag counters
    classes_unloaded: u32,
    stack_frames: u32,
    stack_traces: u32,
    start_threads: u32,
    end_threads: u32,
    heap_summaries: u32,
    heap_dumps: u32,
    allocation_sites: u32,
    control_settings: u32,
    cpu_samples: u32,
    // GC tag counters
    heap_dump_segments_all_sub_records: u32,
    heap_dump_segments_gc_root_unknown: u32,
    heap_dump_segments_gc_root_thread_object: u32,
    heap_dump_segments_gc_root_jni_global: u32,
    heap_dump_segments_gc_root_jni_local: u32,
    heap_dump_segments_gc_root_java_frame: u32,
    heap_dump_segments_gc_root_native_stack: u32,
    heap_dump_segments_gc_root_sticky_class: u32,
    heap_dump_segments_gc_root_thread_block: u32,
    heap_dump_segments_gc_root_monitor_used: u32,
    heap_dump_segments_gc_object_array_dump: u32,
    heap_dump_segments_gc_instance_dump: u32,
    heap_dump_segments_gc_primitive_array_dump: u32,
    heap_dump_segments_gc_class_dump: u32,
    // Captured state
    // "object_id" -> "class_id" -> "class_name_id" -> "utf8_string"
    utf8_strings_by_id: AHashMap<u64, Box<str>>,
    class_data: Vec<LoadClassData>,         // holds class_data
    class_data_by_id: AHashMap<u64, usize>, // value is index into class_data
    class_data_by_serial_number: AHashMap<u32, usize>, // value is index into class_data
    classes_single_instance_size_by_id: AHashMap<u64, ClassInfo>,
    classes_all_instance_total_size_by_id: AHashMap<u64, ClassInstanceCounter>,
    primitive_array_counters: AHashMap<FieldType, ArrayCounter>,
    object_array_counters: AHashMap<u64, ArrayCounter>,
    stack_trace_by_serial_number: AHashMap<u32, StackTraceData>,
    stack_frame_by_id: AHashMap<u64, StackFrameData>,
}

impl ResultRecorder {
    pub fn new(id_size: u32, list_strings: bool) -> Self {
        Self {
            id_size,
            list_strings,
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
            heap_dump_segments_gc_instance_dump: 0,
            heap_dump_segments_gc_class_dump: 0,
            utf8_strings_by_id: AHashMap::new(),
            class_data: vec![],
            class_data_by_id: AHashMap::new(),
            class_data_by_serial_number: AHashMap::default(),
            classes_single_instance_size_by_id: AHashMap::new(),
            classes_all_instance_total_size_by_id: AHashMap::new(),
            primitive_array_counters: AHashMap::new(),
            object_array_counters: AHashMap::new(),
            stack_trace_by_serial_number: AHashMap::default(),
            stack_frame_by_id: AHashMap::default(),
        }
    }

    fn get_class_name_string(&self, class_id: u64) -> String {
        self.class_data_by_id
            .get(&class_id)
            .and_then(|data_index| self.class_data.get(*data_index))
            .and_then(|class_data| self.utf8_strings_by_id.get(&class_data.class_name_id))
            .expect("class_id must have an UTF-8 string representation available")
            .replace('/', ".")
    }

    pub fn start(
        mut self,
        receive_records: Receiver<Vec<Record>>,
        send_result: Sender<RenderedResult>,
        send_pooled_vec: Sender<Vec<Record>>,
    ) -> std::io::Result<JoinHandle<()>> {
        thread::Builder::new()
            .name("hprof-recorder".to_string())
            .spawn(move || {
                loop {
                    if let Ok(mut records) = receive_records.recv() {
                        self.record_records(&mut records);
                        // clear values but retain underlying storage
                        records.clear();
                        // send back pooled vec (swallow errors as it is possible the receiver was already dropped)
                        send_pooled_vec.send(records).unwrap_or_default();
                    } else {
                        // no more Record to pull, generate and send back results
                        let rendered_result = RenderedResult {
                            summary: self.render_summary(),
                            thread_info: self.render_thread_info(),
                            memory_usage: self.aggregate_memory_usage(),
                            duplicated_strings: self.render_duplicated_strings(),
                            captured_strings: if self.list_strings {
                                Some(self.render_captured_strings())
                            } else {
                                None
                            },
                        };
                        send_result
                            .send(rendered_result)
                            .expect("channel should not be closed");
                        break;
                    }
                }
            })
    }

    fn record_records(&mut self, records: &mut [Record]) {
        records.iter_mut().for_each(|record| match record {
            Utf8String { id, str } => {
                self.utf8_strings_by_id.insert(*id, mem::take(str));
            }
            LoadClass(load_class_data) => {
                let class_object_id = load_class_data.class_object_id;
                let class_serial_number = load_class_data.serial_number;
                self.class_data.push(mem::take(load_class_data));
                let data_index = self.class_data.len() - 1;
                self.class_data_by_id.insert(class_object_id, data_index);
                self.class_data_by_serial_number
                    .insert(class_serial_number, data_index);
            }
            UnloadClass { .. } => self.classes_unloaded += 1,
            StackFrame(stack_frame_data) => {
                self.stack_frames += 1;
                self.stack_frame_by_id
                    .insert(stack_frame_data.stack_frame_id, mem::take(stack_frame_data));
            }
            StackTrace(stack_trace_data) => {
                self.stack_traces += 1;
                self.stack_trace_by_serial_number
                    .insert(stack_trace_data.serial_number, mem::take(stack_trace_data));
            }
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
                    GcRecord::RootUnknown { .. } => self.heap_dump_segments_gc_root_unknown += 1,
                    GcRecord::RootThreadObject { .. } => {
                        self.heap_dump_segments_gc_root_thread_object += 1;
                    }
                    GcRecord::RootJniGlobal { .. } => {
                        self.heap_dump_segments_gc_root_jni_global += 1;
                    }
                    GcRecord::RootJniLocal { .. } => self.heap_dump_segments_gc_root_jni_local += 1,
                    GcRecord::RootJavaFrame { .. } => {
                        self.heap_dump_segments_gc_root_java_frame += 1;
                    }
                    GcRecord::RootNativeStack { .. } => {
                        self.heap_dump_segments_gc_root_native_stack += 1;
                    }
                    GcRecord::RootStickyClass { .. } => {
                        self.heap_dump_segments_gc_root_sticky_class += 1;
                    }
                    GcRecord::RootThreadBlock { .. } => {
                        self.heap_dump_segments_gc_root_thread_block += 1;
                    }
                    GcRecord::RootMonitorUsed { .. } => {
                        self.heap_dump_segments_gc_root_monitor_used += 1;
                    }
                    GcRecord::InstanceDump {
                        class_object_id, ..
                    } => {
                        self.classes_all_instance_total_size_by_id
                            .entry(*class_object_id)
                            .or_insert_with(ClassInstanceCounter::empty)
                            .add_instance();

                        self.heap_dump_segments_gc_instance_dump += 1;
                    }
                    GcRecord::ObjectArrayDump {
                        number_of_elements,
                        array_class_id,
                        ..
                    } => {
                        self.object_array_counters
                            .entry(*array_class_id)
                            .or_insert_with(ArrayCounter::empty)
                            .add_elements_from_array(*number_of_elements);

                        self.heap_dump_segments_gc_object_array_dump += 1;
                    }
                    GcRecord::PrimitiveArrayDump {
                        number_of_elements,
                        element_type,
                        ..
                    } => {
                        self.primitive_array_counters
                            .entry(*element_type)
                            .or_insert_with(ArrayCounter::empty)
                            .add_elements_from_array(*number_of_elements);

                        self.heap_dump_segments_gc_primitive_array_dump += 1;
                    }
                    GcRecord::ClassDump(class_dump_fields) => {
                        let class_object_id = class_dump_fields.class_object_id;
                        self.classes_single_instance_size_by_id
                            .entry(class_object_id)
                            .or_insert_with(|| {
                                let instance_size = class_dump_fields.instance_size;
                                let super_class_object_id = class_dump_fields.super_class_object_id;
                                ClassInfo::new(super_class_object_id, instance_size)
                            });

                        self.heap_dump_segments_gc_class_dump += 1;
                    }
                }
            }
        });
    }

    fn render_captured_strings(&self) -> String {
        let mut strings: Vec<_> = self.utf8_strings_by_id.values().collect();
        strings.sort_unstable();
        let mut result = String::from("\nList of Strings\n");
        for s in strings {
            result.push_str(s);
            result.push('\n');
        }
        result
    }

    fn render_duplicated_strings(&self) -> Option<String> {
        let mut strings: Vec<_> = self.utf8_strings_by_id.values().collect();
        strings.sort_unstable();
        let all_len = strings.len();
        strings.dedup();
        let dedup_len = strings.len();
        if all_len == dedup_len {
            None
        } else {
            Some(format!(
                "\nFound {} duplicated strings out of {} unique strings\n",
                all_len - dedup_len,
                all_len
            ))
        }
    }

    fn render_thread_info(&self) -> String {
        let mut thread_info = String::new();

        // for each stacktrace
        let mut stack_traces: Vec<_> = self
            .stack_trace_by_serial_number
            .iter()
            .filter(|(_, stack)| !stack.stack_frame_ids.is_empty()) // omit empty stacktraces
            .collect();

        stack_traces.sort_by_key(|(serial_number, _)| **serial_number);

        writeln!(
            thread_info,
            "\nFound {} threads with stacktraces:",
            stack_traces.len()
        )
        .expect("Could not write to thread info");

        for (index, (_id, stack_data)) in stack_traces.iter().enumerate() {
            write!(thread_info, "\nThread {}\n", index + 1)
                .expect("Could not write to thread info");

            //  for each stack frames
            for stack_frame_id in &stack_data.stack_frame_ids {
                let stack_frame = self.stack_frame_by_id.get(stack_frame_id).unwrap();
                let class_object_id = self
                    .class_data_by_serial_number
                    .get(&stack_frame.class_serial_number)
                    .and_then(|index| self.class_data.get(*index))
                    .expect("Class not found")
                    .class_object_id;
                let class_name = self.get_class_name_string(class_object_id);
                let method_name = self
                    .utf8_strings_by_id
                    .get(&stack_frame.method_name_id)
                    .map_or("unknown method name", |b| &**b);
                let file_name = self
                    .utf8_strings_by_id
                    .get(&stack_frame.source_file_name_id)
                    .map_or("unknown source file", |b| &**b);

                // >0: normal
                // -1: unknown
                // -2: compiled method
                // -3: native method
                let pretty_line_number = match stack_frame.line_number {
                    -1 => "unknown line number".to_string(),
                    -2 => "compiled method".to_string(),
                    -3 => "native method".to_string(),
                    number => format!("{number}"),
                };

                // pretty frame output
                writeln!(
                    thread_info,
                    "  at {class_name}.{method_name} ({file_name}:{pretty_line_number})"
                )
                .expect("Could not write to thread info");
            }
        }
        thread_info
    }

    fn aggregate_memory_usage(&self) -> Vec<ClassAllocationStats> {
        // https://www.baeldung.com/java-memory-layout
        // total_size = object_header + data
        // on a 64-bit arch.
        // object_header = mark(ref_size) + klass(4) + padding_gap(4) = 16 bytes
        // data = instance_size + padding_next(??)
        let object_header = self.id_size + 4 + 4;

        let mut classes_dump_vec: Vec<_> = self
            .classes_all_instance_total_size_by_id
            .iter()
            .map(|(class_id, v)| {
                let class_name = self.get_class_name_string(*class_id);
                let mut size = 0;

                let ClassInfo {
                    super_class_object_id,
                    instance_size,
                } = self
                    .classes_single_instance_size_by_id
                    .get(class_id)
                    .unwrap();
                let mut parent_class_id = *super_class_object_id;
                size += instance_size;

                // recursively add sizes from parent classes
                while parent_class_id != 0 {
                    let ClassInfo {
                        super_class_object_id,
                        instance_size,
                    } = self
                        .classes_single_instance_size_by_id
                        .get(&parent_class_id)
                        .unwrap();
                    size += instance_size;
                    parent_class_id = *super_class_object_id;
                }
                // add object header
                size += object_header;
                // add extra padding if any
                size += size.rem_euclid(8);
                let total_size = u64::from(size) * v.number_of_instances;
                ClassAllocationStats::new(
                    class_name,
                    v.number_of_instances,
                    u64::from(size), // all instances have the same size
                    total_size,
                )
            })
            .collect();

        // https://www.baeldung.com/java-memory-layout
        // the array's `elements` size is already accounted for via `GcInstanceDump` for objects
        // unlike primitives which are packed in the array itself
        // array headers already aligned for 64-bit arch - no need for padding
        // array_header = mark(ref_size) + klass(4) + array_length(4) = 16 bytes
        // data_primitive = primitive_size * length + padding(??)
        // data_object = ref_size * length (no padding because the ref size is already aligned!)
        let ref_size = u64::from(self.id_size);
        let array_header_size = ref_size + 4 + 4;

        let array_primitives_dump_vec =
            self.primitive_array_counters
                .iter()
                .map(|(field_type, &ac)| {
                    let primitive_type = format!("{field_type:?}").to_lowercase();
                    let primitive_array_label = format!("{primitive_type}[]");
                    let primitive_size = primitive_byte_size(*field_type);

                    let cost_of_all_array_headers = array_header_size * ac.number_of_arrays;
                    let cost_of_all_values = primitive_size * ac.total_number_of_elements;
                    // info lost at this point to compute the real padding for each array
                    // assume mid-value of 4 bytes per array for an estimation
                    let estimated_cost_of_all_padding = ac.number_of_arrays * 4;

                    let cost_data_largest_array = primitive_size * u64::from(ac.max_size_seen);
                    let cost_padding_largest_array =
                        (array_header_size + cost_data_largest_array).rem_euclid(8);
                    ClassAllocationStats::new(
                        primitive_array_label,
                        ac.number_of_arrays,
                        array_header_size + cost_data_largest_array + cost_padding_largest_array,
                        cost_of_all_array_headers
                            + cost_of_all_values
                            + estimated_cost_of_all_padding,
                    )
                });

        // For array of objects we are interested in the total size of the array headers and outgoing elements references
        let array_objects_dump_vec = self.object_array_counters.iter().map(|(class_id, &ac)| {
            let raw_class_name = self.get_class_name_string(*class_id);
            let cleaned_class_name: String = if raw_class_name.starts_with("[L") {
                // remove '[L' prefix and ';' suffix
                raw_class_name
                    .chars()
                    .skip(2)
                    .take(raw_class_name.chars().count() - 3)
                    .collect()
            } else if raw_class_name.starts_with("[[L") {
                // remove '[[L' prefix and ';' suffix
                raw_class_name
                    .chars()
                    .skip(3)
                    .take(raw_class_name.chars().count() - 4)
                    .collect()
            } else {
                // TODO: what are those ([[C, [[D, [[B, [[S ...)? boxed primitives are already present
                raw_class_name
            };

            let object_array_label = format!("{cleaned_class_name}[]");

            let cost_of_all_refs = ref_size * ac.total_number_of_elements;
            let cost_of_all_array_headers = array_header_size * ac.number_of_arrays;
            let cost_of_largest_array_refs = ref_size * u64::from(ac.max_size_seen);
            ClassAllocationStats::new(
                object_array_label,
                ac.number_of_arrays,
                array_header_size + cost_of_largest_array_refs,
                cost_of_all_array_headers + cost_of_all_refs,
            )
        });

        // Merge results
        classes_dump_vec.extend(array_primitives_dump_vec);
        classes_dump_vec.extend(array_objects_dump_vec);
        // Sort by class name first for stability in test results :s
        classes_dump_vec.sort_unstable_by(|a, b| b.class_name.cmp(&a.class_name));
        classes_dump_vec
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
            self.class_data_by_id.len(),
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
            ..GC class dump: {}
            ..GC instance dump: {}",
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
            self.heap_dump_segments_gc_instance_dump,
        );

        format!("{top_summary}\n{heap_summary}")
    }
}

fn primitive_byte_size(field_type: FieldType) -> u64 {
    match field_type {
        FieldType::Byte | FieldType::Bool => 1,
        FieldType::Char | FieldType::Short => 2,
        FieldType::Float | FieldType::Int => 4,
        FieldType::Double | FieldType::Long => 8,
        FieldType::Object => panic!("object type in primitive array"),
    }
}
