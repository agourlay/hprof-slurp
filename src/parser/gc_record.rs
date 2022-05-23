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
    pub fn from_value(v: i8) -> FieldType {
        match v {
            2 => FieldType::Object,
            4 => FieldType::Bool,
            5 => FieldType::Char,
            6 => FieldType::Float,
            7 => FieldType::Double,
            8 => FieldType::Byte,
            9 => FieldType::Short,
            10 => FieldType::Int,
            11 => FieldType::Long,
            x => panic!("{}", format!("FieldType {} not found", x)),
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
#[allow(clippy::box_collection)]
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
    ClassDump {
        class_object_id: u64,
        stack_trace_serial_number: u32,
        super_class_object_id: u64,
        instance_size: u32,
        const_fields: Box<Vec<(ConstFieldInfo, FieldValue)>>,
        static_fields: Box<Vec<(FieldInfo, FieldValue)>>,
        instance_fields: Box<Vec<FieldInfo>>,
    },
}
