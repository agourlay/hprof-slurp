use crate::parser::gc_record::GcRecord;

#[derive(Debug, PartialEq, Eq)]
pub struct RecordHeader {
    pub timestamp: u32,
    pub length: u32,
}

#[derive(Debug)]
pub struct AllocationSite {
    pub is_array: u8,
    pub class_serial_number: u32,
    pub stack_trace_serial_number: u32,
    pub bytes_alive: u32,
    pub instances_alive: u32,
    pub bytes_allocated: u32,
    pub instances_allocated: u32,
}

#[derive(Debug)]
pub struct CpuSample {
    pub number_of_samples: u32,
    pub stack_trace_serial_number: u32,
}

#[derive(Debug, Default)]
pub struct StackFrameData {
    pub stack_frame_id: u64,
    pub method_name_id: u64,
    pub method_signature_id: u64,
    pub source_file_name_id: u64,
    pub class_serial_number: u32,
    pub line_number: i32,
}

#[derive(Debug, Default)]
pub struct StackTraceData {
    pub serial_number: u32,
    pub thread_serial_number: u32,
    pub number_of_frames: u32,
    pub stack_frame_ids: Vec<u64>,
}

#[derive(Debug, Default)]
pub struct LoadClassData {
    pub serial_number: u32,
    pub class_object_id: u64,
    pub stack_trace_serial_number: u32,
    pub class_name_id: u64,
}

#[derive(Debug)]
#[allow(clippy::box_collection)]
pub enum Record {
    Utf8String {
        id: u64,
        str: Box<str>,
    },
    LoadClass(LoadClassData),
    UnloadClass {
        serial_number: u32,
    },
    StackFrame(StackFrameData),
    StackTrace(StackTraceData),
    AllocationSites {
        flags: u16,
        cutoff_ratio: u32,
        total_live_bytes: u32,
        total_live_instances: u32,
        total_bytes_allocated: u64,
        total_instances_allocated: u64,
        number_of_sites: u32,
        allocation_sites: Box<Vec<AllocationSite>>,
    },
    StartThread {
        thread_serial_number: u32,
        thread_object_id: u64,
        stack_trace_serial_number: u32,
        thread_name_id: u64,
        thread_group_name_id: u64,
        thread_group_parent_name_id: u64,
    },
    EndThread {
        thread_serial_number: u32,
    },
    HeapSummary {
        total_live_bytes: u32,
        total_live_instances: u32,
        total_bytes_allocated: u64,
        total_instances_allocated: u64,
    },
    HeapDumpStart {
        length: u32,
    },
    HeapDumpEnd {
        length: u32,
    },
    ControlSettings {
        flags: u32,
        stack_trace_depth: u16,
    },
    CpuSamples {
        total_number_of_samples: u32,
        number_of_traces: u32,
        cpu_samples: Vec<CpuSample>,
    },
    GcSegment(GcRecord),
}
