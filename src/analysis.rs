use crate::gc_record::FieldType;
use crate::utils::pretty_bytes_size;
use std::collections::HashMap;

#[derive(Debug, Copy, Clone)]
pub struct ArrayCounter {
    number_of_arrays: u64,
    total_number_of_elements: u64,
}

impl ArrayCounter {
    pub fn add_elements_from_array(&mut self, elements: u32) {
        self.number_of_arrays += 1;
        self.total_number_of_elements += elements as u64;
    }

    pub fn empty() -> ArrayCounter {
        ArrayCounter {
            number_of_arrays: 0,
            total_number_of_elements: 0,
        }
    }
}

fn get_class_name_string(
    class_id: &u64,
    classes_loaded_by_id: &HashMap<u64, u64>,
    utf8_strings_by_id: &HashMap<u64, String>,
) -> String {
    classes_loaded_by_id
        .get(class_id)
        .and_then(|class_id| utf8_strings_by_id.get(class_id))
        .expect("class_id must have an Utf8 string representation available")
        .to_owned()
}

pub fn analysis(
    top: usize,
    id_size: u64,
    utf8_strings_by_id: &HashMap<u64, String>,
    classes_loaded_by_id: &HashMap<u64, u64>,
    classes_all_instance_total_size_by_id: &HashMap<u64, u64>,
    primitive_array_counters: &HashMap<FieldType, ArrayCounter>,
    object_array_counters: &HashMap<u64, ArrayCounter>,
) {
    let mut classes_dump_vec: Vec<_> = classes_all_instance_total_size_by_id
        .iter()
        .map(|(class_id, v)| {
            let class_name =
                get_class_name_string(class_id, classes_loaded_by_id, utf8_strings_by_id);
            (class_name, *v)
        })
        .collect();

    // https://www.baeldung.com/java-memory-layout
    // the array's `elements` size are already computed via `GcInstanceDump`
    // here we are interested in the total size of the array headers and outgoing elements references
    let ref_size = id_size;
    let array_header_size = ref_size + 4 + 4; // 4 bytes of klass + 4 bytes for the array length.

    let mut array_primitives_dump_vec: Vec<_> = primitive_array_counters
        .iter()
        .map(|(ft, &ac)| {
            let primitive_array_label = format!("{:?}[]", ft);
            let cost_of_all_array_headers = array_header_size * ac.number_of_arrays;
            let cost_of_all_values = primitive_byte_size(ft) * ac.total_number_of_elements;
            (
                primitive_array_label,
                cost_of_all_array_headers + cost_of_all_values,
            )
        })
        .collect();

    let mut array_objects_dump_vec: Vec<_> = object_array_counters
        .iter()
        .map(|(class_id, &ac)| {
            let raw_class_name =
                get_class_name_string(class_id, classes_loaded_by_id, utf8_strings_by_id);
            // remove '[L' prefix and ';' suffix
            let cleaned_class_name: String = raw_class_name
                .chars()
                .skip(2)
                .take(raw_class_name.chars().count() - 3)
                .collect();
            let class_name = format!("{}[]", cleaned_class_name);
            let cost_of_all_refs = ref_size * ac.total_number_of_elements;
            let cost_of_all_array_headers = array_header_size * ac.number_of_arrays;
            (class_name, cost_of_all_array_headers + cost_of_all_refs)
        })
        .collect();

    // Merge results
    classes_dump_vec.append(&mut array_primitives_dump_vec);
    classes_dump_vec.append(&mut array_objects_dump_vec);
    // reverse sort by size
    classes_dump_vec.sort_by(|a, b| b.1.cmp(&a.1));

    let total_size = classes_dump_vec.iter().map(|(_, s)| *s as u64).sum();
    let display_total_size = pretty_bytes_size(total_size);

    println!();
    println!(
        "Top {} allocations for the {} heap total size:",
        top, display_total_size
    );
    println!();

    let all_formatted: Vec<_> = classes_dump_vec
        .iter()
        .take(top)
        .map(|(class_name, allocation_size)| {
            let display_allocation = pretty_bytes_size(*allocation_size as u64);
            let allocation_str_len = display_allocation.chars().count();
            (display_allocation, allocation_str_len, class_name)
        })
        .collect();

    let max_length_size_label = all_formatted
        .iter()
        .max_by(|(_, l1, _), (_, l2, _)| l1.cmp(l2))
        .expect("Results can't be empty")
        .1;

    all_formatted
        .iter()
        .for_each(|(allocation_size, allocation_str_len, class_name)| {
            let padding_size = max_length_size_label - allocation_str_len;
            let padding = " ".repeat(padding_size);
            println!("{}{} - {}", padding, allocation_size, class_name,);
        });
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
