mod analysis;
mod args;
mod errors;
mod file_header_parser;
mod gc_record;
mod primitive_parsers;
mod record;
mod record_parser;
mod utils;

use std::fs::File;
use std::io::{BufReader, Read};

use nom::Err;
use nom::Needed::Size;
use nom::Needed::Unknown;

use indicatif::{ProgressBar, ProgressStyle};

use crate::analysis::{analysis, ArrayCounter, ClassInstanceCounter};
use crate::args::get_args;
use crate::errors::HprofSlurpError;
use crate::errors::HprofSlurpError::*;
use crate::file_header_parser::parse_file_header;
use crate::gc_record::{FieldType, GcRecord};
use crate::record::Record::*;
use crate::record_parser::parse_hprof_record;
use crate::utils::pretty_bytes_size;
use std::collections::HashMap;

fn main() -> Result<(), HprofSlurpError> {
    let (file_path, top, debug, list_strings) = get_args()?;

    let file = File::open(file_path)?;
    let meta = file.metadata()?;
    let file_len = meta.len() as usize;

    // Parse file header
    let mut reader = BufReader::new(file);
    let file_header_length = 31; // read the exact size of the file header (31 bytes)
    let mut header_buffer = vec![0; file_header_length];
    reader.read_exact(&mut header_buffer)?;
    let res = parse_file_header(&header_buffer).unwrap();
    // Invariants
    let header = res.1;
    let id_size = header.size_pointers;
    if id_size != 4 && id_size != 8 {
        return Err(InvalidIdSize);
    }
    if id_size == 4 {
        panic!("32 bits heap dumps are not supported yet")
    }
    if !res.0.is_empty() {
        return Err(InvalidHeaderSize);
    }

    println!(
        "Processing {} binary hprof file in '{}' format.",
        pretty_bytes_size(file_len as u64),
        header.format
    );

    // Progress bar
    let pb = ProgressBar::new(file_len as u64);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} (speed:{bytes_per_sec}) (eta:{eta})")
        .progress_chars("#>-"));

    // Captured state
    // "object_id" -> "class_id" -> "class_name_id" -> "utf8_string"
    let mut utf8_strings_by_id: HashMap<u64, String> = HashMap::new();
    let mut classes_loaded_by_id: HashMap<u64, u64> = HashMap::new();
    let mut classes_single_instance_size_by_id: HashMap<u64, u32> = HashMap::new();
    let mut classes_all_instance_total_size_by_id: HashMap<u64, ClassInstanceCounter> =
        HashMap::new();
    let mut primitive_array_counters: HashMap<FieldType, ArrayCounter> = HashMap::new();
    let mut object_array_counters: HashMap<u64, ArrayCounter> = HashMap::new();

    // Tag counters
    let mut classes_unloaded = 0;
    let mut stack_frames = 0;
    let mut stack_traces = 0;
    let mut start_threads = 0;
    let mut end_threads = 0;
    let mut heap_summaries = 0;
    let mut heap_dumps = 0;
    let mut allocation_sites = 0;
    let mut control_settings = 0;
    let mut cpu_samples = 0;

    // GC tag counters
    let mut heap_dump_segments_all_sub_records = 0;
    let mut heap_dump_segments_gc_root_unknown = 0;
    let mut heap_dump_segments_gc_root_thread_object = 0;
    let mut heap_dump_segments_gc_root_jni_global = 0;
    let mut heap_dump_segments_gc_root_jni_local = 0;
    let mut heap_dump_segments_gc_root_java_frame = 0;
    let mut heap_dump_segments_gc_root_native_stack = 0;
    let mut heap_dump_segments_gc_root_sticky_class = 0;
    let mut heap_dump_segments_gc_root_thread_block = 0;
    let mut heap_dump_segments_gc_root_monitor_used = 0;
    let mut heap_dump_segments_gc_object_array_dump = 0;
    let mut heap_dump_segments_gc_primitive_array_dump = 0;
    let mut heap_dump_segments_gc_class_dump = 0;

    // Iteration state
    let mut loop_buffer = Vec::new();
    let mut eof = false;
    let mut processed = file_header_length;
    // 2 MB
    const OPTIMISTIC_BUFFER_SIZE: usize = 2 * 1024 * 1024;

    while !eof {
        pb.set_position(processed as u64);
        match parse_hprof_record(debug, &loop_buffer) {
            Ok((rest, record)) => {
                match record {
                    Utf8String { id, str } => {
                        utf8_strings_by_id.insert(id, str);
                    }
                    LoadClass {
                        class_object_id,
                        class_name_id,
                        ..
                    } => {
                        classes_loaded_by_id.insert(class_object_id, class_name_id);
                    }
                    UnloadClass { .. } => classes_unloaded += 1,
                    StackFrame { .. } => stack_frames += 1,
                    StackTrace { .. } => stack_traces += 1,
                    StartThread { .. } => start_threads += 1,
                    EndThread { .. } => end_threads += 1,
                    AllocationSites { .. } => allocation_sites += 1,
                    HeapSummary { .. } => heap_summaries += 1,
                    ControlSettings { .. } => control_settings += 1,
                    CpuSamples { .. } => cpu_samples += 1,
                    HeapDumpEnd { .. } => (),
                    HeapDumpSegment {
                        length: _,
                        segments,
                    } => {
                        heap_dumps += 1;
                        heap_dump_segments_all_sub_records += segments.len();
                        segments.iter().for_each(|gc_record| match gc_record {
                            GcRecord::GcRootUnknown { .. } => {
                                heap_dump_segments_gc_root_unknown += 1
                            }
                            GcRecord::GcRootThreadObject { .. } => {
                                heap_dump_segments_gc_root_thread_object += 1
                            }
                            GcRecord::GcRootJniGlobal { .. } => {
                                heap_dump_segments_gc_root_jni_global += 1
                            }
                            GcRecord::GcRootJniLocal { .. } => {
                                heap_dump_segments_gc_root_jni_local += 1
                            }
                            GcRecord::GcRootJavaFrame { .. } => {
                                heap_dump_segments_gc_root_java_frame += 1
                            }
                            GcRecord::GcRootNativeStack { .. } => {
                                heap_dump_segments_gc_root_native_stack += 1
                            }
                            GcRecord::GcRootStickyClass { .. } => {
                                heap_dump_segments_gc_root_sticky_class += 1
                            }
                            GcRecord::GcRootThreadBlock { .. } => {
                                heap_dump_segments_gc_root_thread_block += 1
                            }
                            GcRecord::GcRootMonitorUsed { .. } => {
                                heap_dump_segments_gc_root_monitor_used += 1
                            }
                            GcRecord::GcInstanceDump {
                                class_object_id,
                                data_size,
                                ..
                            } => {
                                // no need to perform a lookup in `classes_instance_size_by_id`
                                // data_size is available in the record
                                // total_size = data_size + id_size + mark(4) + padding(4)
                                classes_all_instance_total_size_by_id
                                    .entry(*class_object_id)
                                    .or_insert_with(ClassInstanceCounter::empty)
                                    .add_instance((*data_size + id_size + 8) as u64);
                            }
                            GcRecord::GcObjectArrayDump {
                                number_of_elements,
                                array_class_id,
                                ..
                            } => {
                                object_array_counters
                                    .entry(*array_class_id)
                                    .or_insert_with(ArrayCounter::empty)
                                    .add_elements_from_array(*number_of_elements);

                                heap_dump_segments_gc_object_array_dump += 1
                            }
                            GcRecord::GcPrimitiveArrayDump {
                                number_of_elements,
                                element_type,
                                ..
                            } => {
                                primitive_array_counters
                                    .entry(*element_type)
                                    .or_insert_with(ArrayCounter::empty)
                                    .add_elements_from_array(*number_of_elements);

                                heap_dump_segments_gc_primitive_array_dump += 1
                            }
                            GcRecord::GcClassDump {
                                class_object_id,
                                instance_size,
                                ..
                            } => {
                                // Unused for now, remove it???
                                classes_single_instance_size_by_id
                                    .entry(*class_object_id)
                                    .or_insert(*instance_size);

                                heap_dump_segments_gc_class_dump += 1
                            }
                        });
                    }
                }
                processed += loop_buffer.len() - rest.len();
                // TODO remove rest.to_vec() allocations
                loop_buffer = rest.to_vec();
                if processed == file_len {
                    eof = true;
                }
            }
            Err(Err::Incomplete(Size(nzu))) => {
                let needed = nzu.get();
                // Preload bigger buffer if possible to avoid parsing failure overhead
                let next_size = if needed > OPTIMISTIC_BUFFER_SIZE {
                    needed
                } else if (file_len - processed) > OPTIMISTIC_BUFFER_SIZE {
                    OPTIMISTIC_BUFFER_SIZE
                } else {
                    needed
                };
                if debug {
                    println!(
                        "Need more data {:?}, pull {}",
                        needed,
                        pretty_bytes_size(next_size as u64)
                    );
                }
                let mut extra_buffer = vec![0; next_size];
                reader.read_exact(&mut extra_buffer)?;
                loop_buffer.extend_from_slice(&extra_buffer);
            }
            Err(Err::Incomplete(Unknown)) => {
                if debug {
                    println!("Need more data 'Unknown'");
                }
                let mut extra_buffer = [0; 512];
                reader.read_exact(&mut extra_buffer)?;
                loop_buffer.extend_from_slice(&extra_buffer);
            }
            Err(Err::Failure(e)) => {
                panic!("parsing failed with {:?}", e)
            }
            Err(Err::Error(e)) => {
                panic!("parsing failed with {:?}", e)
            }
        };
    }
    // Finish and remove progress bar
    pb.finish_and_clear();

    println!();
    println!("File content summary:");
    println!();
    println!("Utf8 Strings: {}", utf8_strings_by_id.len());
    println!("Classes loaded: {}", classes_loaded_by_id.len());
    println!("Classes unloaded: {}", classes_unloaded);
    println!("Stack traces: {}", stack_traces);
    println!("Stack frames: {}", stack_frames);
    println!("Start threads: {}", start_threads);
    println!("Allocation sites: {}", allocation_sites);
    println!("End threads: {}", end_threads);
    println!("Control settings: {}", control_settings);
    println!("CPU samples: {}", cpu_samples);
    println!("Heap summaries: {}", heap_summaries);
    println!(
        "{} heap dumps containing in total {} segments:",
        heap_dumps, heap_dump_segments_all_sub_records
    );
    println!("..GC root unknown: {}", heap_dump_segments_gc_root_unknown);
    println!(
        "..GC root thread objects: {}",
        heap_dump_segments_gc_root_thread_object
    );
    println!(
        "..GC root JNI global: {}",
        heap_dump_segments_gc_root_jni_global
    );
    println!(
        "..GC root JNI local: {}",
        heap_dump_segments_gc_root_jni_local
    );
    println!(
        "..GC root Java frame: {}",
        heap_dump_segments_gc_root_java_frame
    );
    println!(
        "..GC root native stack: {}",
        heap_dump_segments_gc_root_native_stack
    );
    println!(
        "..GC root sticky class: {}",
        heap_dump_segments_gc_root_sticky_class
    );
    println!(
        "..GC root thread block: {}",
        heap_dump_segments_gc_root_thread_block
    );
    println!(
        "..GC root monitor used: {}",
        heap_dump_segments_gc_root_monitor_used
    );
    println!(
        "..GC primitive array dump: {}",
        heap_dump_segments_gc_primitive_array_dump
    );
    println!(
        "..GC object array dump: {}",
        heap_dump_segments_gc_object_array_dump
    );
    println!("..GC root class dump: {}", heap_dump_segments_gc_class_dump);
    println!(
        "..GC root instance dump: {}",
        classes_all_instance_total_size_by_id.len()
    );

    analysis(
        top,
        id_size as u64,
        &utf8_strings_by_id,
        &classes_loaded_by_id,
        &classes_all_instance_total_size_by_id,
        &primitive_array_counters,
        &object_array_counters,
    );

    if list_strings {
        let mut strings: Vec<_> = utf8_strings_by_id.values().collect();
        strings.sort();
        println!();
        println!("List of Strings");
        strings.iter().for_each(|s| println!("{}", s));
    }

    Ok(())
}
