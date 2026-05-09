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
    /// Deprecated in modern ART but still emitted by older Android builds.
    RootFinalizing {
        object_id: u64,
    },
    RootDebugger {
        object_id: u64,
    },
    /// Deprecated in modern ART.
    RootReferenceCleanup {
        object_id: u64,
    },
    RootVmInternal {
        object_id: u64,
    },
    RootJniMonitor {
        object_id: u64,
        thread_serial_number: u32,
        stack_depth: u32,
    },
    /// Deprecated in modern ART.
    Unreachable {
        object_id: u64,
    },
    /// Annotates the heap segment that follows. `heap_type` is 1=ZYGOTE,
    /// 2=APP, 3=SYSTEM, 4=IMAGE on Android. `heap_name_id` references a
    /// utf8 record. heaptrail does not currently surface this; we parse it
    /// to keep the record stream aligned.
    HeapDumpInfo {
        heap_type: u32,
        heap_name_id: u64,
    },
    /// `am dumpheap` emits primitive arrays without their data on Android
    /// when the system suppresses the body (e.g. zygote-shared arrays).
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
        /// Raw instance field bytes, retained only in `retain_bodies` parser
        /// mode (used by `--find-referrers` / `--paths-from-id`). `None` in
        /// the default summary path so existing throughput is preserved.
        body: Option<Box<[u8]>>,
    },
    ObjectArrayDump {
        object_id: u64,
        stack_trace_serial_number: u32,
        number_of_elements: u32,
        array_class_id: u64,
        /// Element object ids (`0` == null). Retained only in `retain_bodies`
        /// parser mode. `None` in the default summary path.
        elements: Option<Box<[u64]>>,
    },
    PrimitiveArrayDump {
        object_id: u64,
        stack_trace_serial_number: u32,
        number_of_elements: u32,
        element_type: FieldType,
        /// Truncated raw bytes (first `preview_bytes_limit` per array).
        /// Retained only when the parser is constructed with
        /// `retain_primitive_bodies = true` (v0.9.0 feature B). `None` in
        /// the default summary path so existing throughput is preserved.
        body: Option<Box<[u8]>>,
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
