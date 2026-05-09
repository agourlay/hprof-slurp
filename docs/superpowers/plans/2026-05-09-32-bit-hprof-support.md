# 32-bit HPROF Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add support for HPROF heap dumps whose file header declares 4-byte identifiers, while preserving existing 8-byte dump behavior.

**Architecture:** Keep record model IDs as `u64` so downstream maps and renderers do not bifurcate by pointer width. Thread the header's `id_size` into the streaming parser and use it only at binary parse boundaries and memory-size estimation boundaries. Remove the explicit 32-bit rejection once parser tests prove IDs, ID-sized payload lengths, and integration parsing work for both widths.

**Tech Stack:** Rust, `nom` streaming parsers, `crossbeam_channel`, existing `cargo test` unit/integration tests.

---

## File Structure

- Modify `src/parser/record_parser.rs`: replace the hard-coded `ID_SIZE = 8` parser assumption with dynamic ID parsing based on the HPROF header's `id_size`.
- Modify `src/parser/record_stream_parser.rs`: accept `id_size` in `HprofRecordStreamParser::new` and pass it into `HprofRecordParser::new`.
- Modify `src/slurp.rs`: pass `header.size_pointers` into the stream parser, stop rejecting `id_size == 4`, and update 32-bit tests from rejection to support.
- Modify `src/errors.rs`: remove `UnsupportedIdSize` if it becomes unused.
- Modify `src/result_recorder.rs`: make object and array header size calculations explicit for 4-byte and 8-byte identifiers.
- Modify `README.md`: update the limitations section to state that 32-bit and 64-bit JVM dumps are supported.

### Task 1: Add Parser Tests for Dynamic ID Size

**Files:**
- Modify: `src/parser/record_parser.rs`

- [ ] **Step 1: Write failing parser tests**

Append this test module to the end of `src/parser/record_parser.rs`:

```rust
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
}
```

- [ ] **Step 2: Run parser tests and verify failure**

Run:

```bash
cargo test parser::record_parser::tests -- --nocapture
```

Expected: compile failure because `parse_id`, `parse_utf8_string`, and `parse_stack_trace` currently do not accept an `id_size` argument.

- [ ] **Step 3: Commit failing tests**

```bash
git add src/parser/record_parser.rs
git commit -m "test: cover dynamic hprof identifier sizes"
```

### Task 2: Make Record Parsing Use Header ID Size

**Files:**
- Modify: `src/parser/record_parser.rs`

- [ ] **Step 1: Replace the hard-coded ID-size constant and parser**

In `src/parser/record_parser.rs`, remove the hard-coded ID-size constant:

```rust
const ID_SIZE: u32 = 8;
```

Replace the current `parse_id` function with:

```rust
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
```

- [ ] **Step 2: Add ID size to parser state**

Replace the `HprofRecordParser` struct and constructor with:

```rust
pub struct HprofRecordParser {
    debug_mode: bool,
    id_size: u32,
    heap_dump_remaining_len: u32,
}

impl HprofRecordParser {
    pub const fn new(debug_mode: bool, id_size: u32) -> Self {
        Self {
            debug_mode,
            id_size,
            heap_dump_remaining_len: 0,
        }
    }
```

- [ ] **Step 3: Thread ID size through top-level record dispatch**

Replace `parse_hprof_record` with:

```rust
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
                                self.heap_dump_remaining_len = hr.length;
                                HeapDumpStart { length: hr.length }
                            })
                            .parse(r1)
                        }
                        x => panic!("unhandled record tag {x}"),
                    }
                })
            } else {
                parse_gc_record(i, id_size).map(|(r1, gc_sub)| {
                    let gc_sub_len = i.len() - r1.len();
                    self.heap_dump_remaining_len = self
                        .heap_dump_remaining_len
                        .saturating_sub(gc_sub_len as u32);
                    (r1, GcSegment(gc_sub))
                })
            }
        }
    }
```

- [ ] **Step 4: Change GC record dispatch signatures**

Replace `parse_gc_record` with:

```rust
fn parse_gc_record(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
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
        TAG_GC_INSTANCE_DUMP => parse_gc_instance_dump(r1, id_size),
        TAG_GC_OBJ_ARRAY_DUMP => parse_gc_object_array_dump(r1, id_size),
        TAG_GC_PRIM_ARRAY_DUMP => parse_gc_primitive_array_dump(r1, id_size),
        x => panic!("unhandled gc record tag {x}"),
    }
}
```

Update every GC parser that reads an ID so its signature accepts `id_size: u32` and every ID parser position uses `id(id_size)`. For example, `parse_gc_root_unknown` becomes:

```rust
fn parse_gc_root_unknown(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    map(id(id_size), |object_id| RootUnknown { object_id }).parse(i)
}
```

Use the same pattern for these functions:

```rust
fn parse_gc_root_thread_object(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
fn parse_gc_root_jni_global(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
fn parse_gc_root_jni_local(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
fn parse_gc_root_java_frame(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
fn parse_gc_root_native_stack(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
fn parse_gc_root_sticky_class(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
fn parse_gc_root_thread_block(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
fn parse_gc_root_monitor_used(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord>
```

- [ ] **Step 5: Thread ID size through field-value parsers**

Replace `parse_field_value`, `parse_const_pool_item`, `parse_static_field_item`, and `parse_instance_field_item` with:

```rust
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

fn parse_const_pool_item(
    i: &[u8],
    id_size: u32,
) -> IResult<&[u8], (ConstFieldInfo, FieldValue)> {
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
    flat_map((id(id_size), parse_field_type), move |(name_id, field_type)| {
        map(parse_field_value(field_type, id_size), move |fv| {
            let field_info = FieldInfo {
                name_id,
                field_type,
            };
            (field_info, fv)
        })
    })
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
```

- [ ] **Step 6: Thread ID size through heap dump parsers**

Replace `parse_gc_class_dump`, `parse_gc_instance_dump`, `parse_gc_object_array_dump`, and `parse_gc_primitive_array_dump` with:

```rust
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

fn parse_gc_instance_dump(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
    flat_map(
        (id(id_size), parse_u32, id(id_size), parse_u32),
        |(object_id, stack_trace_serial_number, class_object_id, data_size)| {
            map(bytes::streaming::take(data_size), move |_bytes_segment| {
                InstanceDump {
                    object_id,
                    stack_trace_serial_number,
                    class_object_id,
                    data_size,
                }
            })
        },
    )
    .parse(i)
}

fn parse_gc_object_array_dump(i: &[u8], id_size: u32) -> IResult<&[u8], GcRecord> {
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
            map(
                skip_array_value(element_type, number_of_elements),
                move |_data_array_elements| PrimitiveArrayDump {
                    object_id,
                    stack_trace_serial_number,
                    number_of_elements,
                    element_type,
                },
            )
        },
    )
    .parse(i)
}
```

- [ ] **Step 7: Thread ID size through top-level record body parsers**

Replace the ID-sensitive top-level parser functions with:

```rust
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
```

- [ ] **Step 8: Run parser tests and full tests**

Run:

```bash
cargo test parser::record_parser::tests -- --nocapture
```

Expected: all parser tests pass.

Run:

```bash
cargo test
```

Expected: existing 64-bit tests pass; existing 32-bit tests still fail or still reject until Task 3 changes the slurp boundary.

- [ ] **Step 9: Commit dynamic parser implementation**

```bash
git add src/parser/record_parser.rs
git commit -m "feat: parse hprof records with dynamic identifier size"
```

### Task 3: Allow 32-bit Dumps Through the Streaming Pipeline

**Files:**
- Modify: `src/parser/record_stream_parser.rs`
- Modify: `src/slurp.rs`
- Modify: `src/errors.rs`

- [ ] **Step 1: Update stream parser constructor**

In `src/parser/record_stream_parser.rs`, replace `HprofRecordStreamParser::new` with:

```rust
    pub const fn new(
        debug_mode: bool,
        id_size: u32,
        file_len: usize,
        processed_len: usize,
        initial_loop_buffer: Vec<u8>,
    ) -> Self {
        let parser = HprofRecordParser::new(debug_mode, id_size);
        Self {
            parser,
            debug_mode,
            file_len,
            processed_len,
            loop_buffer: initial_loop_buffer,
            pooled_vec: Vec::new(),
            needed: 0,
        }
    }
```

- [ ] **Step 2: Pass ID size from the file header**

In `src/slurp.rs`, replace the stream parser construction with:

```rust
    let stream_parser = HprofRecordStreamParser::new(
        debug_mode,
        id_size,
        file_len,
        FILE_HEADER_LENGTH,
        initial_loop_buffer,
    );
```

- [ ] **Step 3: Stop rejecting valid 4-byte ID headers**

In `src/slurp.rs`, remove `UnsupportedIdSize` from the import list:

```rust
use crate::errors::HprofSlurpError::{
    InvalidHeaderSize, InvalidHprofFile, InvalidIdSize, StdThreadError,
};
```

Replace the `slurp_header` ID-size invariant with:

```rust
    let id_size = header.size_pointers;
    if id_size != 4 && id_size != 8 {
        return Err(InvalidIdSize);
    }
    if !rest.is_empty() {
        return Err(InvalidHeaderSize);
    }
```

In `src/errors.rs`, remove this enum variant:

```rust
    #[error("unsupported pointer size - {message:?}")]
    UnsupportedIdSize { message: String },
```

- [ ] **Step 4: Update slurp tests for 32-bit support**

In `src/slurp.rs`, replace the existing `unsupported_32_bits` test with:

```rust
    #[test]
    fn supported_32_bits() {
        let result = slurp_file(FILE_PATH_32, false, false);
        assert!(result.is_ok());

        let rendered_result = result.unwrap();
        assert!(rendered_result.summary.contains("UTF-8 Strings:"));
        assert!(!rendered_result.memory_usage.is_empty());
    }
```

Replace `file_header_32_bits` with:

```rust
    #[test]
    fn file_header_32_bits() {
        let file_path = FILE_PATH_32.to_string();
        let file = File::open(file_path).unwrap();
        let mut reader = BufReader::new(file);
        let file_header = slurp_header(&mut reader).unwrap();
        assert_eq!(file_header.size_pointers, 4);
        assert!(matches!(
            file_header.format.as_str(),
            "JAVA PROFILE 1.0.1" | "JAVA PROFILE 1.0.2"
        ));
    }
```

- [ ] **Step 5: Run tests and verify the 32-bit fixture parses**

Run:

```bash
cargo test slurp::tests::supported_32_bits -- --nocapture
```

Expected: PASS.

Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 6: Commit streaming pipeline support**

```bash
git add src/parser/record_stream_parser.rs src/slurp.rs src/errors.rs
git commit -m "feat: accept 32-bit hprof dumps"
```

### Task 4: Make Memory Layout Estimates Explicit for 32-bit Dumps

**Files:**
- Modify: `src/result_recorder.rs`

- [ ] **Step 1: Write memory layout helper tests**

Append this test module to `src/result_recorder.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_header_size_matches_identifier_width() {
        assert_eq!(object_header_size(4), 8);
        assert_eq!(object_header_size(8), 16);
    }

    #[test]
    fn array_header_size_matches_identifier_width() {
        assert_eq!(array_header_size(4), 12);
        assert_eq!(array_header_size(8), 16);
    }
}
```

- [ ] **Step 2: Run helper tests and verify failure**

Run:

```bash
cargo test result_recorder::tests -- --nocapture
```

Expected: compile failure because `object_header_size` and `array_header_size` are not defined.

- [ ] **Step 3: Add helper functions**

Add these functions near `primitive_byte_size` in `src/result_recorder.rs`:

```rust
fn object_header_size(id_size: u32) -> u32 {
    match id_size {
        4 => 8,
        8 => 16,
        x => panic!("unsupported id size {x}"),
    }
}

fn array_header_size(id_size: u32) -> u64 {
    match id_size {
        4 => 12,
        8 => 16,
        x => panic!("unsupported id size {x}"),
    }
}
```

- [ ] **Step 4: Use helpers in aggregation**

In `aggregate_memory_usage`, replace:

```rust
        // https://www.baeldung.com/java-memory-layout
        // total_size = object_header + data
        // on a 64-bit arch.
        // object_header = mark(ref_size) + klass(4) + padding_gap(4) = 16 bytes
        // data = instance_size + padding_next(??)
        let object_header = self.id_size + 4 + 4;
```

with:

```rust
        // total_size = object_header + data
        // 32-bit object_header = mark(4) + klass(4) = 8 bytes
        // 64-bit object_header = mark(8) + klass(4) + padding_gap(4) = 16 bytes
        // data = instance_size + padding_next
        let object_header = object_header_size(self.id_size);
```

Replace:

```rust
        // array_header = mark(ref_size) + klass(4) + array_length(4) = 16 bytes
        // data_primitive = primitive_size * length + padding(??)
        // data_object = ref_size * length (no padding because the ref size is already aligned!)
        let ref_size = u64::from(self.id_size);
        let array_header_size = ref_size + 4 + 4;
```

with:

```rust
        // array_header = mark(ref_size) + klass(4) + array_length(4)
        // data_primitive = primitive_size * length + padding
        // data_object = ref_size * length
        let ref_size = u64::from(self.id_size);
        let array_header_size = array_header_size(self.id_size);
```

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test result_recorder::tests -- --nocapture
```

Expected: PASS.

Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 6: Commit memory estimate clarification**

```bash
git add src/result_recorder.rs
git commit -m "fix: estimate 32-bit object headers"
```

### Task 5: Update Documentation and Verify Real 32-bit Dump

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README limitation text**

In `README.md`, replace:

```markdown
- Does not support dumps generated by 32 bits JVM.
```

with:

```markdown
- Supports heap dumps with 4-byte and 8-byte HPROF identifiers.
```

- [ ] **Step 2: Run unit tests**

Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 3: Run the bundled 32-bit fixture through the CLI**

Run:

```bash
cargo run -- --inputFile test-heap-dumps/hprof-32.bin --top 5
```

Expected: exit code 0, output includes `File successfully processed`, and output does not include `unsupported pointer size`.

- [ ] **Step 4: Run the provided Nexio 32-bit dump through the CLI**

Run:

```bash
cargo run -- --inputFile /Users/jneerdael/Scripts/nexio/nexio-heap-jvm.hprof --top 20
```

Expected: exit code 0, output includes `Processing 540.54MiB binary hprof file in 'JAVA PROFILE 1.0.2' format.`, output includes `File content summary:`, and output does not include `unsupported pointer size`.

- [ ] **Step 5: Re-run the 64-bit fixture to guard against regression**

Run:

```bash
cargo run -- --inputFile test-heap-dumps/hprof-64.bin --top 5
```

Expected: exit code 0, output includes `File successfully processed`, and output still reports allocated classes.

- [ ] **Step 6: Commit docs and verification-ready state**

```bash
git add README.md
git commit -m "docs: document 32-bit hprof support"
```

## Final Verification

- [ ] Run:

```bash
cargo test
```

Expected: all tests pass.

- [ ] Run:

```bash
cargo run -- --inputFile test-heap-dumps/hprof-32.bin --top 5
```

Expected: exit code 0.

- [ ] Run:

```bash
cargo run -- --inputFile test-heap-dumps/hprof-64.bin --top 5
```

Expected: exit code 0.

- [ ] Run:

```bash
cargo run -- --inputFile /Users/jneerdael/Scripts/nexio/nexio-heap-jvm.hprof --top 20
```

Expected: exit code 0.

## Self-Review Notes

- Spec coverage: the plan removes the explicit 32-bit rejection, parses IDs dynamically, updates all ID-sized payload calculations, keeps 64-bit behavior covered by existing tests, and verifies the provided 32-bit dump.
- Placeholder scan: no task relies on unspecified behavior; each code-changing task includes concrete code snippets and commands.
- Type consistency: IDs remain `u64` in record structs and maps; `id_size` remains `u32` from the header through parser and recorder boundaries.
