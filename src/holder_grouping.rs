use ahash::AHashMap;
use serde::Serialize;

use crate::referrer::ReferrerEntry;

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GroupedHolder {
    pub owner_family: String,
    pub holder_class: String,
    pub field_label: String,
    pub ref_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_bytes: Option<u64>,
}

pub fn owner_family(class_name: &str) -> String {
    if !class_name.contains('.') {
        return class_name.to_string();
    }
    let parts = class_name.split('.').collect::<Vec<_>>();
    let take = if parts.first() == Some(&"java") || parts.first() == Some(&"kotlin") {
        parts.len().min(2)
    } else {
        parts.len().min(3)
    };
    parts[..take].join(".")
}

pub fn group_entries(
    entries: impl IntoIterator<Item = ReferrerEntry>,
    retained_by_name: Option<&AHashMap<String, u64>>,
) -> Vec<GroupedHolder> {
    let mut grouped: AHashMap<(String, String, String), GroupedHolder> = AHashMap::new();
    for entry in entries {
        let field_label = entry.field_name.clone().unwrap_or_else(|| "[]".to_string());
        let family = owner_family(&entry.holder_class);
        let key = (
            family.clone(),
            entry.holder_class.clone(),
            field_label.clone(),
        );
        let row = grouped.entry(key).or_insert_with(|| GroupedHolder {
            owner_family: family,
            holder_class: entry.holder_class.clone(),
            field_label,
            ref_count: 0,
            retained_bytes: retained_by_name.and_then(|m| m.get(&entry.holder_class).copied()),
        });
        row.ref_count += entry.ref_count;
    }
    let mut rows = grouped.into_values().collect::<Vec<_>>();
    rows.sort_by_key(|row| (std::cmp::Reverse(row.ref_count), row.holder_class.clone()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(class_name: &str, field_name: Option<&str>, count: u64) -> ReferrerEntry {
        ReferrerEntry {
            holder_class: class_name.to_string(),
            field_name: field_name.map(str::to_string),
            ref_count: count,
        }
    }

    #[test]
    fn owner_family_uses_first_three_segments_for_packages() {
        assert_eq!(
            owner_family("androidx.media3.exoplayer.ExoPlayerImplInternal"),
            "androidx.media3.exoplayer"
        );
        assert_eq!(owner_family("com.nexio.tv.core.Cache"), "com.nexio.tv");
        assert_eq!(owner_family("java.util.ArrayList"), "java.util");
        assert_eq!(owner_family("byte[]"), "byte[]");
    }

    #[test]
    fn groups_by_family_class_and_field() {
        let rows = vec![
            entry("com.nexio.tv.PlayerHolder", Some("player"), 2),
            entry("com.nexio.tv.PlayerHolder", Some("player"), 3),
            entry("java.lang.Object[]", None, 7),
        ];

        let grouped = group_entries(rows, None);

        assert_eq!(grouped[0].holder_class, "java.lang.Object[]");
        assert_eq!(grouped[0].field_label, "[]");
        assert_eq!(grouped[0].ref_count, 7);
        assert!(grouped.iter().any(|g| {
            g.owner_family == "com.nexio.tv"
                && g.holder_class == "com.nexio.tv.PlayerHolder"
                && g.field_label == "player"
                && g.ref_count == 5
        }));
    }
}
