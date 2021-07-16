extern crate nom;

use crate::gc_record::*;
use crate::primitive_parsers::*;
use crate::record::{AllocationSite, CpuSample, Record, RecordHeader};
use crate::record_parser::GcRecord::*;
use crate::record_parser::Record::*;
use nom::combinator::{flat_map, map};
use nom::error::{ErrorKind, ParseError};
use nom::multi::count;
use nom::sequence::{preceded, tuple};
use nom::Parser;
use nom::{bytes, IResult};

const TAG_STRING: u8 = 0x01;
const TAG_LOAD_CLASS: u8 = 0x02;
const TAG_UNLOAD_CLASS: u8 = 0x03;
const TAG_STACK_FRAME: u8 = 0x04;
const TAG_STACK_TRACE: u8 = 0x05;
const TAG_ALLOC_SITES: u8 = 0x06;
const TAG_HEAP_SUMMARY: u8 = 0x07;
const TAG_START_THREAD: u8 = 0x0A;
const TAG_END_THREAD: u8 = 0x0B;
const TAG_HEAP_DUMP: u8 = 0x0C;
const TAG_HEAP_DUMP_SEGMENT: u8 = 0x1C;
const TAG_HEAP_DUMP_END: u8 = 0x2C;
const TAG_CONTROL_SETTING: u8 = 0x0E;
const TAG_CPU_SAMPLES: u8 = 0x0D;

const TAG_GC_ROOT_UNKNOWN: u8 = 0xFF;
const TAG_GC_ROOT_JNI_GLOBAL: u8 = 0x01;
const TAG_GC_ROOT_JNI_LOCAL: u8 = 0x02;
const TAG_GC_ROOT_JAVA_FRAME: u8 = 0x03;
const TAG_GC_ROOT_NATIVE_STACK: u8 = 0x04;
const TAG_GC_ROOT_STICKY_CLASS: u8 = 0x05;
const TAG_GC_ROOT_THREAD_BLOCK: u8 = 0x06;
const TAG_GC_ROOT_MONITOR_USED: u8 = 0x07;
const TAG_GC_ROOT_THREAD_OBJ: u8 = 0x08;
const TAG_GC_CLASS_DUMP: u8 = 0x20;
const TAG_GC_INSTANCE_DUMP: u8 = 0x21;
const TAG_GC_OBJ_ARRAY_DUMP: u8 = 0x22;
const TAG_GC_PRIM_ARRAY_DUMP: u8 = 0x23;

pub struct HprofRecordParser {
    debug_mode: bool,
    id_size_u64: bool, // TODO use to change impl. of parse_id
    heap_dump_remaining_len: u32,
}

impl<'p> HprofRecordParser {
    pub fn new(debug_mode: bool, id_size_u64: bool) -> Self {
        HprofRecordParser {
            debug_mode,
            id_size_u64,
            heap_dump_remaining_len: 0,
        }
    }

    pub fn parse_hprof_record(&'p mut self) -> impl FnMut(&'p[u8]) -> IResult<&'p[u8], Record> {
         move |i| {
            if self.heap_dump_remaining_len == 0 {
                let (r1, tag) = parse_u8(i)?;
                if self.debug_mode {
                    println!("Found record tag:{} remaining bytes:{}", tag, i.len());
                }
                match tag {
                    TAG_STRING => parse_utf8_string(r1),
                    TAG_LOAD_CLASS => parse_load_class(r1),
                    TAG_UNLOAD_CLASS => parse_unload_class(r1),
                    TAG_STACK_FRAME => parse_stack_frame(r1),
                    TAG_STACK_TRACE => parse_stack_trace(r1),
                    TAG_ALLOC_SITES => parse_allocation_sites(r1),
                    TAG_HEAP_SUMMARY => parse_heap_summary(r1),
                    TAG_START_THREAD => parse_start_thread(r1),
                    TAG_END_THREAD => parse_end_thread(r1),
                    TAG_CONTROL_SETTING => parse_control_settings(r1),
                    TAG_CPU_SAMPLES => parse_cpu_samples(r1),
                    TAG_HEAP_DUMP_END => parse_heap_dump_end(r1),
                    TAG_HEAP_DUMP | TAG_HEAP_DUMP_SEGMENT => {
                        let (r2, hr) = parse_header_record(r1)?;
                        // record expected GC segments length
                        self.heap_dump_remaining_len = hr.length;
                        Ok((r2, HeapDumpStart { length: hr.length }))
                    },
                    x => panic!("{}", format!("unhandled record tag {}", x)),
                }
            } else {
                // GC record mode
                let (r1, gc_sub) = parse_gc_record(i)?;
                let gc_sub_len = i.len() - r1.len();
                self.heap_dump_remaining_len -= gc_sub_len as u32;
                Ok((r1, GcSegment(gc_sub)))
            }
        }
    }

    pub fn parse_streaming(&'p mut self, i: &'p[u8]) -> IResult<&'p[u8], Vec<Record>> {
        many1_streaming(self.parse_hprof_record())(i)
    }
}

// TODO change to u32 depending on id_size in header
fn parse_id(i: &[u8]) -> IResult<&[u8], u64> {
    parse_u64(i)
}

// copy of nom's many1 but returns values accumulated so far on `nom::Err::Incomplete(_)`
pub fn many1_streaming<I, O, E, F>(mut f: F) -> impl FnMut(I) -> IResult<I, Vec<O>, E>
where
    I: Clone + PartialEq,
    F: Parser<I, O, E>,
    E: ParseError<I>,
{
    move |mut i: I| match f.parse(i.clone()) {
        Err(nom::Err::Error(err)) => Err(nom::Err::Error(E::append(i, ErrorKind::Many1, err))),
        Err(e) => Err(e),
        Ok((i1, o)) => {
            let mut acc = Vec::with_capacity(4);
            acc.push(o);
            i = i1;

            loop {
                match f.parse(i.clone()) {
                    Err(nom::Err::Error(_)) => return Ok((i, acc)),
                    // magic line here!
                    // return Ok(acc) if we have seen at least one element, otherwise fail
                    Err(nom::Err::Incomplete(_)) => return Ok((i, acc)),
                    Err(e) => return Err(e),
                    Ok((i1, o)) => {
                        if i1 == i {
                            return Err(nom::Err::Error(E::from_error_kind(i, ErrorKind::Many1)));
                        }

                        i = i1;
                        acc.push(o);
                    }
                }
            }
        }
    }
}

fn parse_gc_record(i: &[u8]) -> IResult<&[u8], GcRecord> {
    let (rest, tag) = parse_u8(i)?;
    //println!("GC Tag:{} Remaining:{}", tag, i.len());
    match tag {
        TAG_GC_ROOT_UNKNOWN => parse_gc_root_unknown(rest),
        TAG_GC_ROOT_JNI_GLOBAL => parse_gc_root_jni_global(rest),
        TAG_GC_ROOT_JNI_LOCAL => parse_gc_root_jni_local(rest),
        TAG_GC_ROOT_JAVA_FRAME => parse_gc_root_java_frame(rest),
        TAG_GC_ROOT_NATIVE_STACK => parse_gc_root_native_stack(rest),
        TAG_GC_ROOT_STICKY_CLASS => parse_gc_root_sticky_class(rest),
        TAG_GC_ROOT_THREAD_BLOCK => parse_gc_root_thread_block(rest),
        TAG_GC_ROOT_MONITOR_USED => parse_gc_root_monitor_used(rest),
        TAG_GC_ROOT_THREAD_OBJ => parse_gc_root_thread_object(rest),
        TAG_GC_CLASS_DUMP => parse_gc_class_dump(rest),
        TAG_GC_INSTANCE_DUMP => parse_gc_instance_dump(rest),
        TAG_GC_OBJ_ARRAY_DUMP => parse_gc_object_array_dump(rest),
        TAG_GC_PRIM_ARRAY_DUMP => parse_gc_primitive_array_dump(rest),
        x => panic!("{}", format!("unhandled gc record tag {}", x)),
    }
}

fn parse_gc_root_unknown(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(parse_id, |object_id| GcRootUnknown { object_id })(i)
}

fn parse_gc_root_thread_object(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(
        tuple((parse_id, parse_u32, parse_u32)),
        |(thread_object_id, thread_sequence_number, stack_sequence_number)| GcRootThreadObject {
            thread_object_id,
            thread_sequence_number,
            stack_sequence_number,
        },
    )(i)
}

fn parse_gc_root_jni_global(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(
        tuple((parse_id, parse_id)),
        |(object_id, jni_global_ref_id)| GcRootJniGlobal {
            object_id,
            jni_global_ref_id,
        },
    )(i)
}

fn parse_gc_root_jni_local(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(
        tuple((parse_id, parse_u32, parse_u32)),
        |(object_id, thread_serial_number, frame_number_in_stack_trace)| GcRootJniLocal {
            object_id,
            thread_serial_number,
            frame_number_in_stack_trace,
        },
    )(i)
}

fn parse_gc_root_java_frame(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(
        tuple((parse_id, parse_u32, parse_u32)),
        |(object_id, thread_serial_number, frame_number_in_stack_trace)| GcRootJavaFrame {
            object_id,
            thread_serial_number,
            frame_number_in_stack_trace,
        },
    )(i)
}

fn parse_gc_root_native_stack(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(
        tuple((parse_id, parse_u32)),
        |(object_id, thread_serial_number)| GcRootNativeStack {
            object_id,
            thread_serial_number,
        },
    )(i)
}

fn parse_gc_root_sticky_class(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(parse_id, |object_id| GcRootStickyClass { object_id })(i)
}

fn parse_gc_root_thread_block(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(
        tuple((parse_id, parse_u32)),
        |(object_id, thread_serial_number)| GcRootThreadBlock {
            object_id,
            thread_serial_number,
        },
    )(i)
}

fn parse_gc_root_monitor_used(i: &[u8]) -> IResult<&[u8], GcRecord> {
    map(parse_id, |object_id| GcRootMonitorUsed { object_id })(i)
}

fn parse_field_value(ty: FieldType) -> impl Fn(&[u8]) -> IResult<&[u8], FieldValue> {
    move |i| match ty {
        FieldType::Object => map(parse_id, FieldValue::Object)(i),
        FieldType::Bool => map(parse_u8, |bu8| FieldValue::Bool(bu8 != 0))(i),
        FieldType::Char => map(parse_u16, FieldValue::Char)(i),
        FieldType::Float => map(parse_f32, FieldValue::Float)(i),
        FieldType::Double => map(parse_f64, FieldValue::Double)(i),
        FieldType::Byte => map(parse_i8, FieldValue::Byte)(i),
        FieldType::Short => map(parse_i16, FieldValue::Short)(i),
        FieldType::Int => map(parse_i32, FieldValue::Int)(i),
        FieldType::Long => map(parse_i64, FieldValue::Long)(i),
    }
}

fn parse_field_type(i: &[u8]) -> IResult<&[u8], FieldType> {
    map(parse_i8, FieldType::from_value)(i)
}

fn parse_const_pool_item(i: &[u8]) -> IResult<&[u8], (ConstFieldInfo, FieldValue)> {
    flat_map(
        tuple((parse_u16, parse_field_type)),
        |(const_pool_idx, const_type)| {
            map(parse_field_value(const_type), move |fv| {
                let const_field_info = ConstFieldInfo {
                    const_pool_idx,
                    const_type,
                };
                (const_field_info, fv)
            })
        },
    )(i)
}

fn parse_static_field_item(i: &[u8]) -> IResult<&[u8], (FieldInfo, FieldValue)> {
    flat_map(
        tuple((parse_id, parse_field_type)),
        |(name_id, field_type)| {
            map(parse_field_value(field_type), move |fv| {
                let field_info = FieldInfo {
                    name_id,
                    field_type,
                };
                (field_info, fv)
            })
        },
    )(i)
}

fn parse_instance_field_item(i: &[u8]) -> IResult<&[u8], FieldInfo> {
    map(
        tuple((parse_id, parse_field_type)),
        |(name_id, field_type)| FieldInfo {
            name_id,
            field_type,
        },
    )(i)
}

// TODO use combinators
fn parse_gc_class_dump(i: &[u8]) -> IResult<&[u8], GcRecord> {
    let (
        r1,
        (
            class_object_id,
            stack_trace_serial_number,
            super_class_object_id,
            class_loader_object_id,
            signers_object_id,
            protection_domain_object_id,
            _reserved_1,
            _reserved_2,
            instance_size,
        ),
    ) = tuple((
        parse_id, parse_u32, parse_id, parse_id, parse_id, parse_id, parse_id, parse_id, parse_u32,
    ))(i)?;

    let (r3, constant_pool_size) = parse_u16(r1)?;
    let (r4, const_fields) = count(parse_const_pool_item, constant_pool_size as usize)(r3)?;

    let (r5, static_fields_number) = parse_u16(r4)?;
    let (r6, static_fields) = count(parse_static_field_item, static_fields_number as usize)(r5)?;

    let (r7, instance_field_number) = parse_u16(r6)?;
    let (r8, instance_fields) =
        count(parse_instance_field_item, instance_field_number as usize)(r7)?;

    let gcd = GcClassDump {
        class_object_id,
        stack_trace_serial_number,
        super_class_object_id,
        class_loader_object_id,
        signers_object_id,
        protection_domain_object_id,
        instance_size,
        constant_pool_size,
        const_fields,
        static_fields,
        instance_fields,
    };
    Ok((r8, gcd))
}

// TODO analyze bytes_segment to extract real values?
fn parse_gc_instance_dump(i: &[u8]) -> IResult<&[u8], GcRecord> {
    flat_map(
        tuple((parse_id, parse_u32, parse_id, parse_u32)),
        |(object_id, stack_trace_serial_number, class_object_id, data_size)| {
            map(bytes::streaming::take(data_size), move |_bytes_segment| {
                GcInstanceDump {
                    object_id,
                    stack_trace_serial_number,
                    class_object_id,
                    data_size,
                }
            })
        },
    )(i)
}

fn parse_gc_object_array_dump(i: &[u8]) -> IResult<&[u8], GcRecord> {
    flat_map(
        tuple((parse_id, parse_u32, parse_u32, parse_id)),
        |(object_id, stack_trace_serial_number, number_of_elements, array_class_id)| {
            map(
                count(parse_id, number_of_elements as usize),
                move |elements| GcObjectArrayDump {
                    object_id,
                    stack_trace_serial_number,
                    number_of_elements,
                    array_class_id,
                    elements,
                },
            )
        },
    )(i)
}

// TODO use combinators
fn parse_gc_primitive_array_dump(i: &[u8]) -> IResult<&[u8], GcRecord> {
    let (r1, (object_id, stack_trace_serial_number, number_of_elements, element_type)) =
        tuple((parse_id, parse_u32, parse_u32, parse_field_type))(i)?;

    let (r2, array_value) = match element_type {
        FieldType::Object => panic!("object type in primitive array"),
        FieldType::Bool => map(count(parse_u8, number_of_elements as usize), |res| {
            ArrayValue::Bool(res.iter().map(|b| *b != 0).collect())
        })(r1)?,
        FieldType::Char => map(count(parse_u16, number_of_elements as usize), |res| {
            ArrayValue::Char(res)
        })(r1)?,
        FieldType::Float => map(count(parse_f32, number_of_elements as usize), |res| {
            ArrayValue::Float(res)
        })(r1)?,
        FieldType::Double => map(count(parse_f64, number_of_elements as usize), |res| {
            ArrayValue::Double(res)
        })(r1)?,
        FieldType::Byte => map(count(parse_i8, number_of_elements as usize), |res| {
            ArrayValue::Byte(res)
        })(r1)?,
        FieldType::Short => map(count(parse_i16, number_of_elements as usize), |res| {
            ArrayValue::Short(res)
        })(r1)?,
        FieldType::Int => map(count(parse_i32, number_of_elements as usize), |res| {
            ArrayValue::Int(res)
        })(r1)?,
        FieldType::Long => map(count(parse_i64, number_of_elements as usize), |res| {
            ArrayValue::Long(res)
        })(r1)?,
    };
    let gpad = GcPrimitiveArrayDump {
        object_id,
        stack_trace_serial_number,
        number_of_elements,
        element_type,
        array_value,
    };
    Ok((r2, gpad))
}

fn parse_header_record(i: &[u8]) -> IResult<&[u8], RecordHeader> {
    map(tuple((parse_u32, parse_u32)), |(timestamp, length)| {
        RecordHeader { timestamp, length }
    })(i)
}

// TODO inject real id_size instead of '8'
fn parse_utf8_string(i: &[u8]) -> IResult<&[u8], Record> {
    flat_map(parse_header_record, |header_record| {
        map(
            tuple((parse_id, bytes::streaming::take(header_record.length - 8))),
            |(id, b)| {
                let str = String::from_utf8_lossy(b).to_string();
                Utf8String { id, str }
            },
        )
    })(i)
}

fn parse_load_class(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            tuple((parse_u32, parse_id, parse_u32, parse_id)),
            |(serial_number, class_object_id, stack_trace_serial_number, class_name_id)| {
                LoadClass {
                    serial_number,
                    class_object_id,
                    stack_trace_serial_number,
                    class_name_id,
                }
            },
        ),
    )(i)
}

fn parse_unload_class(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(parse_u32, |serial_number| UnloadClass { serial_number }),
    )(i)
}

fn parse_stack_frame(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            tuple((parse_id, parse_id, parse_id, parse_id, parse_u32, parse_u32)),
            |(
                stack_frame_id,
                method_name_id,
                method_signature_id,
                source_file_name_id,
                class_serial_number,
                line_number,
            )| {
                StackFrame {
                    stack_frame_id,
                    method_name_id,
                    method_signature_id,
                    source_file_name_id,
                    class_serial_number,
                    line_number,
                }
            },
        ),
    )(i)
}

// TODO inject correct id_size instead of '8'
fn parse_stack_trace(i: &[u8]) -> IResult<&[u8], Record> {
    flat_map(parse_header_record, |header_record| {
        // (header_record.length - (3 * parse_u32)) / id_size = (header_record.length - 12) / 8
        let stack_frame_ids_len = (header_record.length - 12) / 8;
        map(
            tuple((
                parse_u32,
                parse_u32,
                parse_u32,
                count(parse_id, stack_frame_ids_len as usize),
            )),
            |(serial_number, thread_serial_number, number_of_frames, stack_frame_ids)| StackTrace {
                serial_number,
                thread_serial_number,
                number_of_frames,
                stack_frame_ids,
            },
        )
    })(i)
}

fn parse_start_thread(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            tuple((parse_u32, parse_id, parse_u32, parse_id, parse_id, parse_id)),
            |(
                thread_serial_number,
                thread_object_id,
                stack_trace_serial_number,
                thread_name_id,
                thread_group_name_id,
                thread_group_parent_name_id,
            )| StartThread {
                thread_serial_number,
                thread_object_id,
                stack_trace_serial_number,
                thread_name_id,
                thread_group_name_id,
                thread_group_parent_name_id,
            },
        ),
    )(i)
}

fn parse_heap_summary(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            tuple((parse_u32, parse_u32, parse_u64, parse_u64)),
            |(
                total_live_bytes,
                total_live_instances,
                total_bytes_allocated,
                total_instances_allocated,
            )| HeapSummary {
                total_live_bytes,
                total_live_instances,
                total_bytes_allocated,
                total_instances_allocated,
            },
        ),
    )(i)
}

fn parse_end_thread(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(parse_u32, |thread_serial_number| EndThread {
            thread_serial_number,
        }),
    )(i)
}

fn parse_allocation_site(i: &[u8]) -> IResult<&[u8], AllocationSite> {
    map(
        tuple((
            parse_u8, parse_u32, parse_u32, parse_u32, parse_u32, parse_u32, parse_u32,
        )),
        |(
            is_array,
            class_serial_number,
            stack_trace_serial_number,
            bytes_alive,
            instances_alive,
            bytes_allocated,
            instances_allocated,
        )| {
            AllocationSite {
                is_array,
                class_serial_number,
                stack_trace_serial_number,
                bytes_alive,
                instances_alive,
                bytes_allocated,
                instances_allocated,
            }
        },
    )(i)
}

fn parse_allocation_sites(i: &[u8]) -> IResult<&[u8], Record> {
    flat_map(
        preceded(
            parse_header_record,
            tuple((
                parse_u16, parse_u32, parse_u32, parse_u32, parse_u64, parse_u64, parse_u32,
            )),
        ),
        |(
            flags,
            cutoff_ratio,
            total_live_bytes,
            total_live_instances,
            total_bytes_allocated,
            total_instances_allocated,
            number_of_sites,
        )| {
            map(
                count(parse_allocation_site, number_of_sites as usize),
                move |allocation_sites| AllocationSites {
                    flags,
                    cutoff_ratio,
                    total_live_bytes,
                    total_live_instances,
                    total_bytes_allocated,
                    total_instances_allocated,
                    number_of_sites,
                    allocation_sites,
                },
            )
        },
    )(i)
}

fn parse_heap_dump_end(i: &[u8]) -> IResult<&[u8], Record> {
    map(parse_header_record, |rb| HeapDumpEnd { length: rb.length })(i)
}

fn parse_control_settings(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            tuple((parse_u32, parse_u16)),
            |(flags, stack_trace_depth)| ControlSettings {
                flags,
                stack_trace_depth,
            },
        ),
    )(i)
}

fn parse_cpu_sample(i: &[u8]) -> IResult<&[u8], CpuSample> {
    map(
        tuple((parse_u32, parse_u32)),
        |(number_of_samples, stack_trace_serial_number)| CpuSample {
            number_of_samples,
            stack_trace_serial_number,
        },
    )(i)
}

fn parse_cpu_samples(i: &[u8]) -> IResult<&[u8], Record> {
    flat_map(
        preceded(parse_header_record, tuple((parse_u32, parse_u32))),
        |(total_number_of_samples, number_of_traces)| {
            map(
                count(parse_cpu_sample, total_number_of_samples as usize),
                move |cpu_samples| CpuSamples {
                    total_number_of_samples,
                    number_of_traces,
                    cpu_samples,
                },
            )
        },
    )(i)
}
