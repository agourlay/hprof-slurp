use crate::parser::gc_record::{
    ArrayValue, ClassDumpFields, ConstFieldInfo, FieldInfo, FieldType, FieldValue, GcRecord,
};
use crate::parser::primitive_parsers::{
    parse_f32, parse_f64, parse_i8, parse_i16, parse_i32, parse_i64, parse_u8, parse_u16,
    parse_u32, parse_u64,
};
use crate::parser::record::{
    AllocationSite, CpuSample, LoadClassData, Record, RecordHeader, StackFrameData, StackTraceData,
};
use crate::parser::record_parser::GcRecord::{
    ClassDump, InstanceDump, ObjectArrayDump, PrimitiveArrayDump, RootJavaFrame, RootJniGlobal,
    RootJniLocal, RootMonitorUsed, RootNativeStack, RootStickyClass, RootThreadBlock,
    RootThreadObject, RootUnknown,
};
use crate::parser::record_parser::Record::{
    AllocationSites, ControlSettings, CpuSamples, EndThread, GcSegment, HeapDumpEnd, HeapDumpStart,
    HeapSummary, LoadClass, StackFrame, StackTrace, StartThread, UnloadClass, Utf8String,
};
use nom::Parser;
use nom::combinator::{flat_map, map};
use nom::error::{ErrorKind, ParseError};
use nom::multi::count;
use nom::sequence::preceded;
use nom::{IResult, bytes};

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

// Android HPROF extension tags (JAVA PROFILE 1.0.3, emitted by `am dumpheap`
// on modern ART). Source: art/runtime/hprof/hprof.cc in AOSP.
const TAG_GC_ROOT_INTERNED_STRING: u8 = 0x89;
const TAG_GC_ROOT_FINALIZING: u8 = 0x8A; // deprecated but still emitted
const TAG_GC_ROOT_DEBUGGER: u8 = 0x8B;
const TAG_GC_ROOT_REFERENCE_CLEANUP: u8 = 0x8C; // deprecated
const TAG_GC_ROOT_VM_INTERNAL: u8 = 0x8D;
const TAG_GC_ROOT_JNI_MONITOR: u8 = 0x8E;
const TAG_GC_UNREACHABLE: u8 = 0x90; // deprecated
const TAG_GC_PRIM_ARRAY_NODATA_DUMP: u8 = 0xC3;
const TAG_GC_HEAP_DUMP_INFO: u8 = 0xFE;

pub struct HprofRecordParser {
    debug_mode: bool,
    id_size: u32,
    heap_dump_remaining_len: u32,
    /// When true, instance bodies and object-array element ids are retained on
    /// `GcRecord::InstanceDump.body` / `GcRecord::ObjectArrayDump.elements`.
    /// Used by `--find-referrers`, `--paths-from-id`, and any other mode that
    /// needs to walk reference graphs. Default false; the summary path leaves
    /// these as `None` to preserve streaming throughput.
    retain_bodies: bool,
    /// v0.9.0: when true, primitive array bodies are retained on
    /// `PrimitiveArrayDump.body`, truncated to `preview_bytes_limit` bytes.
    retain_primitive_bodies: bool,
    /// Cap per primitive array when `retain_primitive_bodies` is true.
    /// 0 means "no cap" (retain full body — discouraged for large dumps).
    preview_bytes_limit: u32,
}

impl HprofRecordParser {
    /// Construct a parser with all current opt-in modes spelled out.
    /// External callers (`HprofRecordStreamParser::with_modes`,
    /// `slurp::parse_records_with_modes`) delegate to this. There's no
    /// shorter constructor — passing the flags explicitly keeps the
    /// cost ladder visible at every call site.
    pub const fn with_modes(
        debug_mode: bool,
        id_size: u32,
        retain_bodies: bool,
        retain_primitive_bodies: bool,
        preview_bytes_limit: u32,
    ) -> Self {
        Self {
            debug_mode,
            id_size,
            heap_dump_remaining_len: 0,
            retain_bodies,
            retain_primitive_bodies,
            preview_bytes_limit,
        }
    }

    // TODO use nom combinators (instead of Result's)
    pub fn parse_hprof_record(&mut self) -> impl FnMut(&[u8]) -> IResult<&[u8], Record> + '_ {
        |i| {
            let id_size = self.id_size;
            if self.heap_dump_remaining_len == 0 {
                parse_u8(i).and_then(|(r1, tag)| {
                    if self.debug_mode {
                        println!("Found record tag:{} remaining bytes:{}", tag, i.len());
                    }
                    match tag {
                        TAG_STRING => parse_utf8_string(r1, id_size),
                        TAG_LOAD_CLASS => parse_load_class(r1, id_size),
                        TAG_UNLOAD_CLASS => parse_unload_class(r1),
                        TAG_STACK_FRAME => parse_stack_frame(r1, id_size),
                        TAG_STACK_TRACE => parse_stack_trace(r1, id_size),
                        TAG_ALLOC_SITES => parse_allocation_sites(r1),
                        TAG_HEAP_SUMMARY => parse_heap_summary(r1),
                        TAG_START_THREAD => parse_start_thread(r1, id_size),
                        TAG_END_THREAD => parse_end_thread(r1),
                        TAG_CONTROL_SETTING => parse_control_settings(r1),
                        TAG_CPU_SAMPLES => parse_cpu_samples(r1),
                        TAG_HEAP_DUMP_END => parse_heap_dump_end(r1),
                        TAG_HEAP_DUMP | TAG_HEAP_DUMP_SEGMENT => {
                            map(parse_header_record, |hr| {
                                // record expected GC segments length
                                self.heap_dump_remaining_len = hr.length;
                                HeapDumpStart { length: hr.length }
                            })
                            .parse(r1)
                        }
                        x => panic!("unhandled record tag {x}"),
                    }
                })
            } else {
                // GC record mode
                parse_gc_record(
                    i,
                    id_size,
                    self.retain_bodies,
                    self.retain_primitive_bodies,
                    self.preview_bytes_limit,
                )
                .map(|(r1, gc_sub)| {
                    let gc_sub_len = i.len() - r1.len();
                    self.heap_dump_remaining_len = self
                        .heap_dump_remaining_len
                        .saturating_sub(gc_sub_len as u32);
                    (r1, GcSegment(gc_sub))
                })
            }
        }
    }

    pub fn parse_streaming<'a>(
        &mut self,
        i: &'a [u8],
        pooled_vec: &mut Vec<Record>,
    ) -> IResult<&'a [u8], ()> {
        lazy_many1(self.parse_hprof_record(), pooled_vec)(i)
    }
}

fn parse_id(i: &[u8], id_size: u32) -> IResult<&[u8], u64> {
    match id_size {
        4 => map(parse_u32, u64::from).parse(i),
        8 => parse_u64(i),
        x => panic!("unsupported id size {x}"),
    }
}

fn id(id_size: u32) -> impl FnMut(&[u8]) -> IResult<&[u8], u64> {
    move |i| parse_id(i, id_size)
}

// copy of nom's many1 but
// - returns values accumulated so far on `nom::Err::Incomplete(_)` if any
// - take a `&mut vector` as input to enable pooling at the call site
pub fn lazy_many1<'a, I, O, E, F>(
    mut f: F,
    pooled_vec: &'a mut Vec<O>,
) -> impl FnMut(I) -> IResult<I, (), E> + 'a
where
    I: Clone + PartialEq,
    F: Parser<I, Output = O, Error = E> + 'a,
    E: ParseError<I>,
{
    move |mut i: I| match f.parse(i.clone()) {
        Err(nom::Err::Error(err)) => Err(nom::Err::Error(E::append(i, ErrorKind::Many1, err))),
        Err(e) => Err(e),
        Ok((i1, o)) => {
            pooled_vec.push(o);
            i = i1;
            loop {
                match f.parse(i.clone()) {
                    Err(nom::Err::Error(_)) => return Ok((i, ())),
                    // magic line here!
                    // return Ok(acc) if we have seen at least one element, otherwise fail
                    Err(nom::Err::Incomplete(_)) => return Ok((i, ())),
                    Err(e) => return Err(e),
                    Ok((i1, o)) => {
                        if i1 == i {
                            return Err(nom::Err::Error(E::from_error_kind(i, ErrorKind::Many1)));
                        }

                        i = i1;
                        pooled_vec.push(o);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)] // dispatch arity tracks the parser modes; folding into a struct hides intent
fn parse_gc_record(
    i: &[u8],
    id_size: u32,
    retain_bodies: bool,
    retain_primitive_bodies: bool,
    preview_bytes_limit: u32,
) -> IResult<&[u8], GcRecord> {
    let (r1, tag) = parse_u8(i)?;
    match tag {
        TAG_GC_ROOT_UNKNOWN => parse_gc_root_unknown(r1, id_size),
        TAG_GC_ROOT_JNI_GLOBAL => parse_gc_root_jni_global(r1, id_size),
        TAG_GC_ROOT_JNI_LOCAL => parse_gc_root_jni_local(r1, id_size),
        TAG_GC_ROOT_JAVA_FRAME => parse_gc_root_java_frame(r1, id_size),
        TAG_GC_ROOT_NATIVE_STACK => parse_gc_root_native_stack(r1, id_size),
        TAG_GC_ROOT_STICKY_CLASS => parse_gc_root_sticky_class(r1, id_size),
        TAG_GC_ROOT_THREAD_BLOCK => parse_gc_root_thread_block(r1, id_size),
        TAG_GC_ROOT_MONITOR_USED => parse_gc_root_monitor_used(r1, id_size),
        TAG_GC_ROOT_THREAD_OBJ => parse_gc_root_thread_object(r1, id_size),
        TAG_GC_CLASS_DUMP => parse_gc_class_dump(r1, id_size),
        TAG_GC_INSTANCE_DUMP if retain_bodies => parse_gc_instance_dump_full(r1, id_size),
        TAG_GC_INSTANCE_DUMP => parse_gc_instance_dump_lite(r1, id_size),
        TAG_GC_OBJ_ARRAY_DUMP if retain_bodies => parse_gc_object_array_dump_full(r1, id_size),
        TAG_GC_OBJ_ARRAY_DUMP => parse_gc_object_array_dump_lite(r1, id_size),
        TAG_GC_PRIM_ARRAY_DUMP if retain_primitive_bodies => {
            parse_gc_primitive_array_dump_full(r1, id_size, preview_bytes_limit)
        }
        TAG_GC_PRIM_ARRAY_DUMP => parse_gc_primitive_array_dump_lite(r1, id_size),
        // Android HPROF 1.0.3 extensions (am dumpheap on modern ART).
        TAG_GC_ROOT_INTERNED_STRING => parse_gc_root_interned_string(r1, id_size),
        TAG_GC_ROOT_FINALIZING => parse_gc_root_finalizing(r1, id_size),
        TAG_GC_ROOT_DEBUGGER => parse_gc_root_debugger(r1, id_size),
        TAG_GC_ROOT_REFERENCE_CLEANUP => parse_gc_root_reference_cleanup(r1, id_size),
        TAG_GC_ROOT_VM_INTERNAL => parse_gc_root_vm_internal(r1, id_size),
        TAG_GC_ROOT_JNI_MONITOR => parse_gc_root_jni_monitor(r1, id_size),
        TAG_GC_UNREACHABLE => parse_gc_unreachable(r1, id_size),
        TAG_GC_PRIM_ARRAY_NODATA_DUMP => parse_gc_primitive_array_nodata_dump(r1, id_size),
        TAG_GC_HEAP_DUMP_INFO => parse_gc_heap_dump_info(r1, id_size),
        x => panic!("unhandled gc record tag {x}"),
    }
}

fn parse_gc_root_unknown(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| RootUnknown { object_id }).parse(i)
}

fn parse_gc_root_thread_object(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), parse_u32, parse_u32),
        |(thread_object_id, thread_sequence_number, stack_sequence_number)| RootThreadObject {
            thread_object_id,
            thread_sequence_number,
            stack_sequence_number,
        },
    )
    .parse(i)
}

fn parse_gc_root_jni_global(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), id(id_size)),
        |(object_id, jni_global_ref_id)| RootJniGlobal {
            object_id,
            jni_global_ref_id,
        },
    )
    .parse(i)
}

fn parse_gc_root_jni_local(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), parse_u32, parse_u32),
        |(object_id, thread_serial_number, frame_number_in_stack_trace)| RootJniLocal {
            object_id,
            thread_serial_number,
            frame_number_in_stack_trace,
        },
    )
    .parse(i)
}

fn parse_gc_root_java_frame(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), parse_u32, parse_u32),
        |(object_id, thread_serial_number, frame_number_in_stack_trace)| RootJavaFrame {
            object_id,
            thread_serial_number,
            frame_number_in_stack_trace,
        },
    )
    .parse(i)
}

fn parse_gc_root_native_stack(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), parse_u32),
        |(object_id, thread_serial_number)| RootNativeStack {
            object_id,
            thread_serial_number,
        },
    )
    .parse(i)
}

fn parse_gc_root_sticky_class(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| RootStickyClass { object_id }).parse(i)
}

fn parse_gc_root_thread_block(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), parse_u32),
        |(object_id, thread_serial_number)| RootThreadBlock {
            object_id,
            thread_serial_number,
        },
    )
    .parse(i)
}

fn parse_gc_root_monitor_used(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| RootMonitorUsed { object_id }).parse(i)
}

// ---- Android HPROF 1.0.3 extension parsers ----
// All parsers below are ID-only roots (single object id payload), except
// where noted. The deprecated tags (RootFinalizing, RootReferenceCleanup,
// Unreachable) are still parsed for forward-compat with older Android
// builds; they're all single-id records.

fn parse_gc_root_interned_string(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| GcRecord::RootInternedString {
        object_id,
    })
    .parse(i)
}

fn parse_gc_root_finalizing(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| GcRecord::RootFinalizing {
        object_id,
    })
    .parse(i)
}

fn parse_gc_root_debugger(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| GcRecord::RootDebugger {
        object_id,
    })
    .parse(i)
}

fn parse_gc_root_reference_cleanup(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| GcRecord::RootReferenceCleanup {
        object_id,
    })
    .parse(i)
}

fn parse_gc_root_vm_internal(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| GcRecord::RootVmInternal {
        object_id,
    })
    .parse(i)
}

fn parse_gc_root_jni_monitor(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), parse_u32, parse_u32),
        |(object_id, thread_serial_number, stack_depth)| GcRecord::RootJniMonitor {
            object_id,
            thread_serial_number,
            stack_depth,
        },
    )
    .parse(i)
}

fn parse_gc_unreachable(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| GcRecord::Unreachable { object_id }).parse(i)
}

fn parse_gc_primitive_array_nodata_dump(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(
        (id(id_size), parse_u32, parse_u32, parse_field_type),
        |(object_id, stack_trace_serial_number, number_of_elements, element_type)| {
            GcRecord::PrimitiveArrayNoDataDump {
                object_id,
                stack_trace_serial_number,
                number_of_elements,
                element_type,
            }
        },
    )
    .parse(i)
}

fn parse_gc_heap_dump_info(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map((parse_u32, id(id_size)), |(heap_type, heap_name_id)| {
        GcRecord::HeapDumpInfo {
            heap_type,
            heap_name_id,
        }
    })
    .parse(i)
}

fn parse_field_value(ty: FieldType, id_size: u32) -> impl Fn(&[u8]) -> IResult<&[u8], FieldValue> {
    move |i| match ty {
        FieldType::Object => map(id(id_size), FieldValue::Object).parse(i),
        FieldType::Bool => map(parse_u8, |bu8| FieldValue::Bool(bu8 != 0)).parse(i),
        FieldType::Char => map(parse_u16, FieldValue::Char).parse(i),
        FieldType::Float => map(parse_f32, FieldValue::Float).parse(i),
        FieldType::Double => map(parse_f64, FieldValue::Double).parse(i),
        FieldType::Byte => map(parse_i8, FieldValue::Byte).parse(i),
        FieldType::Short => map(parse_i16, FieldValue::Short).parse(i),
        FieldType::Int => map(parse_i32, FieldValue::Int).parse(i),
        FieldType::Long => map(parse_i64, FieldValue::Long).parse(i),
    }
}

#[allow(dead_code)]
// could be used in the future to analyze content of largest arrays
fn parse_array_value(
    element_type: FieldType,
    number_of_elements: u32,
) -> impl Fn(&[u8]) -> IResult<&[u8], ArrayValue> {
    move |i| match element_type {
        FieldType::Object => panic!("object type in primitive array"),
        FieldType::Bool => map(count(parse_u8, number_of_elements as usize), |res| {
            ArrayValue::Bool(res.iter().map(|b| *b != 0).collect())
        })
        .parse(i),
        FieldType::Char => map(count(parse_u16, number_of_elements as usize), |res| {
            ArrayValue::Char(res)
        })
        .parse(i),
        FieldType::Float => map(count(parse_f32, number_of_elements as usize), |res| {
            ArrayValue::Float(res)
        })
        .parse(i),
        FieldType::Double => map(count(parse_f64, number_of_elements as usize), |res| {
            ArrayValue::Double(res)
        })
        .parse(i),
        FieldType::Byte => map(count(parse_i8, number_of_elements as usize), |res| {
            ArrayValue::Byte(res)
        })
        .parse(i),
        FieldType::Short => map(count(parse_i16, number_of_elements as usize), |res| {
            ArrayValue::Short(res)
        })
        .parse(i),
        FieldType::Int => map(count(parse_i32, number_of_elements as usize), |res| {
            ArrayValue::Int(res)
        })
        .parse(i),
        FieldType::Long => map(count(parse_i64, number_of_elements as usize), |res| {
            ArrayValue::Long(res)
        })
        .parse(i),
    }
}

fn skip_array_value(
    element_type: FieldType,
    number_of_elements: u32,
) -> impl Fn(&[u8]) -> IResult<&[u8], &[u8]> {
    let n = u64::from(number_of_elements);
    move |i| match element_type {
        FieldType::Object => panic!("object type in primitive array"),
        FieldType::Bool => bytes::streaming::take(n)(i),
        FieldType::Char => bytes::streaming::take(n * 2)(i),
        FieldType::Float => bytes::streaming::take(n * 4)(i),
        FieldType::Double => bytes::streaming::take(n * 8)(i),
        FieldType::Byte => bytes::streaming::take(n)(i),
        FieldType::Short => bytes::streaming::take(n * 2)(i),
        FieldType::Int => bytes::streaming::take(n * 4)(i),
        FieldType::Long => bytes::streaming::take(n * 8)(i),
    }
}

fn parse_field_type(i: &[u8]) -> IResult<&[u8], FieldType> {
    map(parse_i8, FieldType::from_value).parse(i)
}

fn parse_const_pool_item(i: &[u8], id_size: u32) -> IResult<&[u8], (ConstFieldInfo, FieldValue)> {
    flat_map(
        (parse_u16, parse_field_type),
        move |(const_pool_idx, const_type)| {
            map(parse_field_value(const_type, id_size), move |fv| {
                let const_field_info = ConstFieldInfo {
                    const_pool_idx,
                    const_type,
                };
                (const_field_info, fv)
            })
        },
    )
    .parse(i)
}

fn parse_static_field_item(i: &[u8], id_size: u32) -> IResult<&[u8], (FieldInfo, FieldValue)> {
    flat_map(
        (id(id_size), parse_field_type),
        move |(name_id, field_type)| {
            map(parse_field_value(field_type, id_size), move |fv| {
                let field_info = FieldInfo {
                    name_id,
                    field_type,
                };
                (field_info, fv)
            })
        },
    )
    .parse(i)
}

fn parse_instance_field_item(i: &[u8], id_size: u32) -> IResult<&[u8], FieldInfo> {
    map((id(id_size), parse_field_type), |(name_id, field_type)| {
        FieldInfo {
            name_id,
            field_type,
        }
    })
    .parse(i)
}

// TODO use nom combinators (instead of Result's)
fn parse_gc_class_dump(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    let (
        r1,
        (
            class_object_id,
            stack_trace_serial_number,
            super_class_object_id,
            _class_loader_object_id,
            _signers_object_id,
            _protection_domain_object_id,
            _reserved_1,
            _reserved_2,
            instance_size,
            constant_pool_size,
        ),
    ) = (
        id(id_size),
        parse_u32,
        id(id_size),
        id(id_size),
        id(id_size),
        id(id_size),
        id(id_size),
        id(id_size),
        parse_u32,
        parse_u16,
    )
        .parse(i)?;

    count(
        move |input| parse_const_pool_item(input, id_size),
        constant_pool_size as usize,
    )
    .parse(r1)
    .and_then(|(r2, const_fields)| {
        parse_u16(r2).and_then(|(r3, static_fields_number)| {
            count(
                move |input| parse_static_field_item(input, id_size),
                static_fields_number as usize,
            )
            .parse(r3)
            .and_then(|(r4, static_fields)| {
                parse_u16(r4).and_then(|(r5, instance_field_number)| {
                    count(
                        move |input| parse_instance_field_item(input, id_size),
                        instance_field_number as usize,
                    )
                    .parse(r5)
                    .map(|(r6, instance_fields)| {
                        let class_dump_fields = ClassDumpFields::new(
                            class_object_id,
                            stack_trace_serial_number,
                            super_class_object_id,
                            instance_size,
                            const_fields,
                            static_fields,
                            instance_fields,
                        );
                        let gcd = ClassDump(Box::new(class_dump_fields));
                        (r6, gcd)
                    })
                })
            })
        })
    })
}

/// Default parser: skips the instance body bytes (the original streaming
/// behavior). Used for summary mode where reference graph isn't needed.
fn parse_gc_instance_dump_lite(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, id(id_size), parse_u32),
        |(object_id, stack_trace_serial_number, class_object_id, data_size)| {
            map(bytes::streaming::take(data_size), move |_bytes_segment| {
                InstanceDump {
                    object_id,
                    stack_trace_serial_number,
                    class_object_id,
                    data_size,
                    body: None,
                }
            })
        },
    )
    .parse(i)
}

/// Retain-bodies parser: copies the instance body into a `Box<[u8]>` so
/// downstream recorders can walk fields against a known target id set.
/// Pass-2 of `--find-referrers` and `--paths-from-id` use this.
fn parse_gc_instance_dump_full(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, id(id_size), parse_u32),
        |(object_id, stack_trace_serial_number, class_object_id, data_size)| {
            map(
                bytes::streaming::take(data_size),
                move |bytes_segment: &[u8]| InstanceDump {
                    object_id,
                    stack_trace_serial_number,
                    class_object_id,
                    data_size,
                    body: Some(bytes_segment.to_vec().into_boxed_slice()),
                },
            )
        },
    )
    .parse(i)
}

fn parse_gc_object_array_dump_lite(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, parse_u32, id(id_size)),
        move |(object_id, stack_trace_serial_number, number_of_elements, array_class_id)| {
            map(
                bytes::streaming::take(u64::from(number_of_elements) * u64::from(id_size)),
                move |_byte_array_elements| ObjectArrayDump {
                    object_id,
                    stack_trace_serial_number,
                    number_of_elements,
                    array_class_id,
                    elements: None,
                },
            )
        },
    )
    .parse(i)
}

fn parse_gc_object_array_dump_full(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, parse_u32, id(id_size)),
        move |(object_id, stack_trace_serial_number, number_of_elements, array_class_id)| {
            map(
                count(id(id_size), number_of_elements as usize),
                move |elements_vec: Vec<u64>| ObjectArrayDump {
                    object_id,
                    stack_trace_serial_number,
                    number_of_elements,
                    array_class_id,
                    elements: Some(elements_vec.into_boxed_slice()),
                },
            )
        },
    )
    .parse(i)
}

fn parse_gc_primitive_array_dump(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, parse_u32, parse_field_type),
        |(object_id, stack_trace_serial_number, number_of_elements, element_type)| {
            // Do not parse the array of primitives as it is not needed for any analyses so far.
            // see `parse_array_value(element_type, number_of_elements)`
            map(
                skip_array_value(element_type, number_of_elements),
                move |_data_array_elements| PrimitiveArrayDump {
                    object_id,
                    stack_trace_serial_number,
                    number_of_elements,
                    element_type,
                    body: None,
                },
            )
        },
    )
    .parse(i)
}

/// Default parser: skips the primitive body bytes (the original
/// streaming behavior). Used everywhere `--preview-bytes` is not set.
fn parse_gc_primitive_array_dump_lite(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    parse_gc_primitive_array_dump(i, id_size)
}

/// Retain-bodies parser: copies up to `preview_bytes_limit` bytes of
/// the array body into the GcRecord. The parser still consumes the
/// full payload (we don't seek), but only stores the truncated prefix
/// to keep memory bounded. preview_bytes_limit = 0 means "retain full".
fn parse_gc_primitive_array_dump_full(
    i: &[u8],
    id_size: u32,
    preview_bytes_limit: u32,
) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, parse_u32, parse_field_type),
        move |(object_id, stack_trace_serial_number, number_of_elements, element_type)| {
            map(
                skip_array_value(element_type, number_of_elements),
                move |raw_bytes: &[u8]| {
                    let cap = if preview_bytes_limit == 0 {
                        raw_bytes.len()
                    } else {
                        std::cmp::min(raw_bytes.len(), preview_bytes_limit as usize)
                    };
                    let body = if cap == 0 {
                        None
                    } else {
                        Some(raw_bytes[..cap].to_vec().into_boxed_slice())
                    };
                    PrimitiveArrayDump {
                        object_id,
                        stack_trace_serial_number,
                        number_of_elements,
                        element_type,
                        body,
                    }
                },
            )
        },
    )
    .parse(i)
}

fn parse_header_record(i: &[u8]) -> IResult<&[u8], RecordHeader> {
    map((parse_u32, parse_u32), |(timestamp, length)| RecordHeader {
        timestamp,
        length,
    })
    .parse(i)
}

fn parse_utf8_string(i: &[u8], id_size: u32) -> IResult<&[u8], Record> {
    flat_map(parse_header_record, move |header_record| {
        map(
            (
                id(id_size),
                bytes::streaming::take(header_record.length.saturating_sub(id_size)),
            ),
            |(id, b)| {
                let str = String::from_utf8_lossy(b).into();
                Utf8String { id, str }
            },
        )
    })
    .parse(i)
}

fn parse_load_class(i: &[u8], id_size: u32) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            (parse_u32, id(id_size), parse_u32, id(id_size)),
            |(serial_number, class_object_id, stack_trace_serial_number, class_name_id)| {
                LoadClass(LoadClassData {
                    serial_number,
                    class_object_id,
                    stack_trace_serial_number,
                    class_name_id,
                })
            },
        ),
    )
    .parse(i)
}

fn parse_unload_class(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(parse_u32, |serial_number| UnloadClass { serial_number }),
    )
    .parse(i)
}

fn parse_stack_frame(i: &[u8], id_size: u32) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            (
                id(id_size),
                id(id_size),
                id(id_size),
                id(id_size),
                parse_u32,
                parse_i32,
            ),
            |(
                stack_frame_id,
                method_name_id,
                method_signature_id,
                source_file_name_id,
                class_serial_number,
                line_number,
            )| {
                StackFrame(StackFrameData {
                    stack_frame_id,
                    method_name_id,
                    method_signature_id,
                    source_file_name_id,
                    class_serial_number,
                    line_number,
                })
            },
        ),
    )
    .parse(i)
}

fn parse_stack_trace(i: &[u8], id_size: u32) -> IResult<&[u8], Record> {
    flat_map(parse_header_record, move |header_record| {
        let stack_frame_ids_len = header_record.length.saturating_sub(12) / id_size;
        map(
            (
                parse_u32,
                parse_u32,
                parse_u32,
                count(id(id_size), stack_frame_ids_len as usize),
            ),
            |(serial_number, thread_serial_number, number_of_frames, stack_frame_ids)| {
                StackTrace(StackTraceData {
                    serial_number,
                    thread_serial_number,
                    number_of_frames,
                    stack_frame_ids,
                })
            },
        )
    })
    .parse(i)
}

fn parse_start_thread(i: &[u8], id_size: u32) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            (
                parse_u32,
                id(id_size),
                parse_u32,
                id(id_size),
                id(id_size),
                id(id_size),
            ),
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
    )
    .parse(i)
}

fn parse_heap_summary(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(
            (parse_u32, parse_u32, parse_u64, parse_u64),
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
    )
    .parse(i)
}

fn parse_end_thread(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map(parse_u32, |thread_serial_number| EndThread {
            thread_serial_number,
        }),
    )
    .parse(i)
}

fn parse_allocation_site(i: &[u8]) -> IResult<&[u8], AllocationSite> {
    map(
        (
            parse_u8, parse_u32, parse_u32, parse_u32, parse_u32, parse_u32, parse_u32,
        ),
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
    )
    .parse(i)
}

fn parse_allocation_sites(i: &[u8]) -> IResult<&[u8], Record> {
    flat_map(
        preceded(
            parse_header_record,
            (
                parse_u16, parse_u32, parse_u32, parse_u32, parse_u64, parse_u64, parse_u32,
            ),
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
                    allocation_sites: Box::new(allocation_sites),
                },
            )
        },
    )
    .parse(i)
}

fn parse_heap_dump_end(i: &[u8]) -> IResult<&[u8], Record> {
    map(parse_header_record, |rb| HeapDumpEnd { length: rb.length }).parse(i)
}

fn parse_control_settings(i: &[u8]) -> IResult<&[u8], Record> {
    preceded(
        parse_header_record,
        map((parse_u32, parse_u16), |(flags, stack_trace_depth)| {
            ControlSettings {
                flags,
                stack_trace_depth,
            }
        }),
    )
    .parse(i)
}

fn parse_cpu_sample(i: &[u8]) -> IResult<&[u8], CpuSample> {
    map(
        (parse_u32, parse_u32),
        |(number_of_samples, stack_trace_serial_number)| CpuSample {
            number_of_samples,
            stack_trace_serial_number,
        },
    )
    .parse(i)
}

fn parse_cpu_samples(i: &[u8]) -> IResult<&[u8], Record> {
    flat_map(
        preceded(parse_header_record, (parse_u32, parse_u32)),
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
    )
    .parse(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::record::Record;

    #[test]
    fn parse_id_respects_32_bit_header_size() {
        let input = [0x12, 0x34, 0x56, 0x78, 0xaa];

        let (rest, id) = parse_id(&input, 4).unwrap();

        assert_eq!(id, 0x1234_5678);
        assert_eq!(rest, &[0xaa]);
    }

    #[test]
    fn parse_id_respects_64_bit_header_size() {
        let input = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xaa];

        let (rest, id) = parse_id(&input, 8).unwrap();

        assert_eq!(id, 0x0123_4567_89ab_cdef);
        assert_eq!(rest, &[0xaa]);
    }

    #[test]
    fn parse_utf8_string_uses_32_bit_id_size_for_payload_length() {
        let input = [
            0x00, 0x00, 0x00, 0x00, // timestamp
            0x00, 0x00, 0x00, 0x07, // length: 4-byte ID + "abc"
            0x00, 0x00, 0x00, 0x2a, // string ID
            b'a', b'b', b'c',
        ];

        let (rest, record) = parse_utf8_string(&input, 4).unwrap();

        assert!(rest.is_empty());
        match record {
            Record::Utf8String { id, str } => {
                assert_eq!(id, 42);
                assert_eq!(&*str, "abc");
            }
            other => panic!("expected UTF-8 string record, got {other:?}"),
        }
    }

    #[test]
    fn parse_stack_trace_counts_32_bit_frame_ids_from_record_length() {
        let input = [
            0x00, 0x00, 0x00, 0x00, // timestamp
            0x00, 0x00, 0x00, 0x14, // length: 3 u32 fields + 2 4-byte frame IDs
            0x00, 0x00, 0x00, 0x01, // stack trace serial number
            0x00, 0x00, 0x00, 0x02, // thread serial number
            0x00, 0x00, 0x00, 0x02, // number of frames
            0x00, 0x00, 0x00, 0x0a, // frame ID 10
            0x00, 0x00, 0x00, 0x0b, // frame ID 11
        ];

        let (rest, record) = parse_stack_trace(&input, 4).unwrap();

        assert!(rest.is_empty());
        match record {
            Record::StackTrace(data) => {
                assert_eq!(data.serial_number, 1);
                assert_eq!(data.thread_serial_number, 2);
                assert_eq!(data.number_of_frames, 2);
                assert_eq!(data.stack_frame_ids, vec![10, 11]);
            }
            other => panic!("expected stack trace record, got {other:?}"),
        }
    }

    fn synthetic_instance_dump_bytes(body_pattern: u8, body_len: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u64.to_be_bytes()); // object_id
        buf.extend_from_slice(&0u32.to_be_bytes()); // stack_trace_serial_number
        buf.extend_from_slice(&2u64.to_be_bytes()); // class_object_id
        buf.extend_from_slice(&body_len.to_be_bytes()); // data_size
        buf.extend(std::iter::repeat_n(body_pattern, body_len as usize));
        buf
    }

    #[test]
    fn instance_dump_lite_returns_none_body() {
        let buf = synthetic_instance_dump_bytes(0xAB, 16);
        let (_, gcd) = parse_gc_instance_dump_lite(&buf, 8).unwrap();
        match gcd {
            InstanceDump {
                object_id,
                class_object_id,
                data_size,
                body,
                ..
            } => {
                assert_eq!(object_id, 1);
                assert_eq!(class_object_id, 2);
                assert_eq!(data_size, 16);
                assert!(body.is_none(), "lite parser should not retain body");
            }
            other => panic!("expected InstanceDump, got {other:?}"),
        }
    }

    #[test]
    fn instance_dump_full_retains_body() {
        let buf = synthetic_instance_dump_bytes(0xAB, 16);
        let (_, gcd) = parse_gc_instance_dump_full(&buf, 8).unwrap();
        match gcd {
            InstanceDump {
                body: Some(b),
                data_size,
                ..
            } => {
                assert_eq!(data_size, 16);
                assert_eq!(b.len(), 16);
                assert!(b.iter().all(|&x| x == 0xAB));
            }
            other => panic!("expected InstanceDump with body, got {other:?}"),
        }
    }

    fn synthetic_object_array_dump_bytes(num_elements: u32, base_id: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&100u64.to_be_bytes()); // object_id
        buf.extend_from_slice(&0u32.to_be_bytes()); // stack_trace
        buf.extend_from_slice(&num_elements.to_be_bytes());
        buf.extend_from_slice(&200u64.to_be_bytes()); // array_class_id
        for n in 0..num_elements {
            buf.extend_from_slice(&(base_id + u64::from(n)).to_be_bytes());
        }
        buf
    }

    #[test]
    fn object_array_dump_lite_returns_none_elements() {
        let buf = synthetic_object_array_dump_bytes(3, 1000);
        let (_, gcd) = parse_gc_object_array_dump_lite(&buf, 8).unwrap();
        match gcd {
            ObjectArrayDump {
                number_of_elements,
                elements,
                ..
            } => {
                assert_eq!(number_of_elements, 3);
                assert!(elements.is_none(), "lite parser should not retain elements");
            }
            other => panic!("expected ObjectArrayDump, got {other:?}"),
        }
    }

    #[test]
    fn object_array_dump_full_retains_element_ids() {
        let buf = synthetic_object_array_dump_bytes(3, 1000);
        let (_, gcd) = parse_gc_object_array_dump_full(&buf, 8).unwrap();
        match gcd {
            ObjectArrayDump {
                number_of_elements,
                elements: Some(e),
                ..
            } => {
                assert_eq!(number_of_elements, 3);
                assert_eq!(e.as_ref(), &[1000u64, 1001, 1002]);
            }
            other => panic!("expected ObjectArrayDump with elements, got {other:?}"),
        }
    }

    // ---- Android HPROF 1.0.3 extension parsers ----
    // The 235 MiB dump that triggered "unhandled gc record tag 141"
    // exercised TAG_GC_ROOT_VM_INTERNAL specifically. These tests cover
    // the full extension set so the next exotic tag we see also works.

    fn dispatch_gc_record(tag: u8, payload: &[u8], id_size: u32) -> GcRecord {
        let mut buf = Vec::with_capacity(payload.len() + 1);
        buf.push(tag);
        buf.extend_from_slice(payload);
        let (rest, gcd) = parse_gc_record(&buf, id_size, false, false, 0).unwrap();
        assert!(rest.is_empty(), "parser left {} bytes unread", rest.len());
        gcd
    }

    #[test]
    fn android_root_vm_internal_parses() {
        // Tag 0x8d (decimal 141) — the exact tag that panicked.
        let payload = 42u64.to_be_bytes();
        match dispatch_gc_record(0x8D, &payload, 8) {
            GcRecord::RootVmInternal { object_id } => assert_eq!(object_id, 42),
            other => panic!("expected RootVmInternal, got {other:?}"),
        }
    }

    #[test]
    fn android_root_interned_string_parses() {
        let payload = 7u64.to_be_bytes();
        match dispatch_gc_record(0x89, &payload, 8) {
            GcRecord::RootInternedString { object_id } => assert_eq!(object_id, 7),
            other => panic!("expected RootInternedString, got {other:?}"),
        }
    }

    #[test]
    fn android_root_jni_monitor_parses_three_fields() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&100u64.to_be_bytes()); // object_id
        payload.extend_from_slice(&5u32.to_be_bytes()); // thread_serial
        payload.extend_from_slice(&3u32.to_be_bytes()); // stack_depth
        match dispatch_gc_record(0x8E, &payload, 8) {
            GcRecord::RootJniMonitor {
                object_id,
                thread_serial_number,
                stack_depth,
            } => {
                assert_eq!(object_id, 100);
                assert_eq!(thread_serial_number, 5);
                assert_eq!(stack_depth, 3);
            }
            other => panic!("expected RootJniMonitor, got {other:?}"),
        }
    }

    #[test]
    fn android_heap_dump_info_parses() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&2u32.to_be_bytes()); // heap_type=2 (APP)
        payload.extend_from_slice(&0xCAFEu64.to_be_bytes()); // heap_name_id
        match dispatch_gc_record(0xFE, &payload, 8) {
            GcRecord::HeapDumpInfo {
                heap_type,
                heap_name_id,
            } => {
                assert_eq!(heap_type, 2);
                assert_eq!(heap_name_id, 0xCAFE);
            }
            other => panic!("expected HeapDumpInfo, got {other:?}"),
        }
    }

    #[test]
    fn android_primitive_array_nodata_dump_parses() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&50u64.to_be_bytes()); // object_id
        payload.extend_from_slice(&1u32.to_be_bytes()); // stack_trace_serial
        payload.extend_from_slice(&64u32.to_be_bytes()); // number_of_elements
        payload.push(8); // element_type Byte
        match dispatch_gc_record(0xC3, &payload, 8) {
            GcRecord::PrimitiveArrayNoDataDump {
                object_id,
                number_of_elements,
                element_type,
                ..
            } => {
                assert_eq!(object_id, 50);
                assert_eq!(number_of_elements, 64);
                assert!(matches!(element_type, FieldType::Byte));
            }
            other => panic!("expected PrimitiveArrayNoDataDump, got {other:?}"),
        }
    }

    #[test]
    fn android_deprecated_roots_still_parse() {
        // 0x8a, 0x8c, 0x90 — older Android builds still emit these.
        for tag in [0x8A, 0x8C, 0x90] {
            let payload = 99u64.to_be_bytes();
            // Must not panic and must consume the payload exactly.
            dispatch_gc_record(tag, &payload, 8);
        }
    }

    #[test]
    fn android_root_debugger_and_reference_cleanup_parse() {
        let payload = 11u64.to_be_bytes();
        match dispatch_gc_record(0x8B, &payload, 8) {
            GcRecord::RootDebugger { object_id } => assert_eq!(object_id, 11),
            other => panic!("expected RootDebugger, got {other:?}"),
        }
        match dispatch_gc_record(0x8C, &payload, 8) {
            GcRecord::RootReferenceCleanup { object_id } => assert_eq!(object_id, 11),
            other => panic!("expected RootReferenceCleanup, got {other:?}"),
        }
    }

    // ---- v0.9.0 (feature B) primitive-array preview tests ----

    fn synthetic_primitive_array_dump_bytes(num_elements: u32, element_byte: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        // object_id (8 bytes)
        buf.extend_from_slice(&[0; 8]);
        // stack trace serial (4 bytes)
        buf.extend_from_slice(&[0; 4]);
        // num elements (4 bytes)
        buf.extend_from_slice(&num_elements.to_be_bytes());
        // element_type: FieldType::Byte = 8
        buf.push(8);
        // body: num_elements bytes of `element_byte`
        buf.extend(std::iter::repeat_n(element_byte, num_elements as usize));
        buf
    }

    #[test]
    fn parse_gc_primitive_array_dump_lite_returns_none_body() {
        let buf = synthetic_primitive_array_dump_bytes(64, 0xAB);
        let (_, gcd) = parse_gc_primitive_array_dump_lite(&buf, 8).unwrap();
        match gcd {
            GcRecord::PrimitiveArrayDump {
                body: None,
                number_of_elements,
                ..
            } => {
                assert_eq!(number_of_elements, 64);
            }
            other => panic!("expected None body, got {other:?}"),
        }
    }

    #[test]
    fn parse_gc_primitive_array_dump_full_truncates_to_limit() {
        let buf = synthetic_primitive_array_dump_bytes(1024, 0xCD);
        let (_, gcd) = parse_gc_primitive_array_dump_full(&buf, 8, 100).unwrap();
        match gcd {
            GcRecord::PrimitiveArrayDump { body: Some(b), .. } => {
                assert_eq!(b.len(), 100, "expected truncation to 100 bytes");
                assert!(b.iter().all(|&x| x == 0xCD));
            }
            other => panic!("expected Some(100) body, got {other:?}"),
        }
    }

    #[test]
    fn parse_gc_primitive_array_dump_full_keeps_smaller_than_limit_intact() {
        let buf = synthetic_primitive_array_dump_bytes(8, 0xEF);
        let (_, gcd) = parse_gc_primitive_array_dump_full(&buf, 8, 200).unwrap();
        match gcd {
            GcRecord::PrimitiveArrayDump { body: Some(b), .. } => {
                assert_eq!(b.len(), 8, "expected full body, no padding");
            }
            other => panic!("expected Some(8) body, got {other:?}"),
        }
    }
}
