#![allow(dead_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FieldType {
    Object = 2,
    Bool = 4,
    Char = 5,
    Float = 6,
    Double = 7,
    Byte = 8,
    Short = 9,
    Int = 10,
    Long = 11,
}

impl FieldType {
    pub fn from_value(v: i8) -> Self {
        match v {
            2 => Self::Object,
            4 => Self::Bool,
            5 => Self::Char,
            6 => Self::Float,
            7 => Self::Double,
            8 => Self::Byte,
            9 => Self::Short,
            10 => Self::Int,
            11 => Self::Long,
            x => panic!("FieldType {x} not found"),
        }
    }
}

#[derive(Debug)]
pub struct ConstFieldInfo {
    pub const_pool_idx: u16,
    pub const_type: FieldType,
}

#[derive(Debug)]
pub struct FieldInfo {
    pub name_id: u64,
    pub field_type: FieldType,
}

#[derive(Debug)]
pub enum FieldValue {
    Bool(bool),
    Byte(i8),
    Char(u16),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Object(u64),
}

#[derive(Debug)]
pub enum ArrayValue {
    Bool(Vec<bool>),
    Byte(Vec<i8>),
    Char(Vec<u16>),
    Short(Vec<i16>),
    Int(Vec<i32>),
    Long(Vec<i64>),
    Float(Vec<f32>),
    Double(Vec<f64>),
    //Object(Vec<u64>),
}

#[derive(Debug)]
pub enum GcRecord {
    RootUnknown {
        object_id: u64,
    },
    RootThreadObject {
        thread_object_id: u64,
        thread_sequence_number: u32,
        stack_sequence_number: u32,
    },
    RootJniGlobal {
        object_id: u64,
        jni_global_ref_id: u64,
    },
    RootJniLocal {
        object_id: u64,
        thread_serial_number: u32,
        frame_number_in_stack_trace: u32,
    },
    RootJavaFrame {
        object_id: u64,
        thread_serial_number: u32,
        frame_number_in_stack_trace: u32,
    },
    RootNativeStack {
        object_id: u64,
        thread_serial_number: u32,
    },
    RootStickyClass {
        object_id: u64,
    },
    RootThreadBlock {
        object_id: u64,
        thread_serial_number: u32,
    },
    RootMonitorUsed {
        object_id: u64,
    },
    // ---- Android HPROF 1.0.3 extension roots (art/runtime/hprof/hprof.cc) ----
    RootInternedString {
        object_id: u64,
    },
    /// Obsolete in ART (its writer `LOG(FATAL)`s on this tag), but older Dalvik
    /// dumps emit it as an id-only root. Parsed accordingly.
    RootFinalizing {
        object_id: u64,
    },
    RootDebugger {
        object_id: u64,
    },
    /// Obsolete in ART; see [`GcRecord::RootFinalizing`].
    RootReferenceCleanup {
        object_id: u64,
    },
    RootVmInternal {
        object_id: u64,
    },
    /// Layout mirrors `RootJniLocal`/`RootJavaFrame`: the trailing `u32` is the
    /// frame number in the stack trace (`-1` when empty), per ART's writer.
    RootJniMonitor {
        object_id: u64,
        thread_serial_number: u32,
        frame_number_in_stack_trace: u32,
    },
    /// Obsolete in ART; see [`GcRecord::RootFinalizing`].
    Unreachable {
        object_id: u64,
    },
    /// Annotates the heap segment that follows. On Android `heap_type` is an
    /// ASCII code: `'A'` (65) app, `'Z'` (90) zygote, `'I'` (73) image, `0`
    /// default. `heap_name_id` references a utf8 record. Parsed only to keep
    /// the record stream aligned.
    HeapDumpInfo {
        heap_type: u32,
        heap_name_id: u64,
    },
    /// Primitive array whose body was omitted by the dumper. Marked obsolete
    /// in ART (current builds emit a regular `PrimitiveArrayDump` instead);
    /// parsed defensively. Layout mirrors `PrimitiveArrayDump` minus the body.
    PrimitiveArrayNoDataDump {
        object_id: u64,
        stack_trace_serial_number: u32,
        number_of_elements: u32,
        element_type: FieldType,
    },
    InstanceDump {
        object_id: u64,
        stack_trace_serial_number: u32,
        class_object_id: u64,
        data_size: u32,
    },
    ObjectArrayDump {
        object_id: u64,
        stack_trace_serial_number: u32,
        number_of_elements: u32,
        array_class_id: u64,
    },
    PrimitiveArrayDump {
        object_id: u64,
        stack_trace_serial_number: u32,
        number_of_elements: u32,
        element_type: FieldType,
    },
    ClassDump(Box<ClassDumpFields>), // rare enough to be boxed to avoid large variant cost
}

#[derive(Debug)]
pub struct ClassDumpFields {
    pub class_object_id: u64,
    pub stack_trace_serial_number: u32,
    pub super_class_object_id: u64,
    pub instance_size: u32,
    pub const_fields: Vec<(ConstFieldInfo, FieldValue)>,
    pub static_fields: Vec<(FieldInfo, FieldValue)>,
    pub instance_fields: Vec<FieldInfo>,
}

impl ClassDumpFields {
    pub const fn new(
        class_object_id: u64,
        stack_trace_serial_number: u32,
        super_class_object_id: u64,
        instance_size: u32,
        const_fields: Vec<(ConstFieldInfo, FieldValue)>,
        static_fields: Vec<(FieldInfo, FieldValue)>,
        instance_fields: Vec<FieldInfo>,
    ) -> Self {
        Self {
            class_object_id,
            stack_trace_serial_number,
            super_class_object_id,
            instance_size,
            const_fields,
            static_fields,
            instance_fields,
        }
    }
}
