use std::path::{Path, PathBuf};

use ahash::AHashMap;

use crate::args::MappingOptions;
use crate::errors::HprofSlurpError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingInfo {
    pub path: PathBuf,
    pub pg_map_id: Option<String>,
    pub pg_map_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Symbolicator {
    pub info: MappingInfo,
    class_by_obfuscated: AHashMap<String, String>,
    fields_by_obfuscated_class: AHashMap<String, AHashMap<String, String>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedMapping {
    pub symbolicator: Symbolicator,
    pub source: MappingSource,
}

#[derive(Debug, Clone)]
pub enum MappingSource {
    Manual,
    Auto {
        package: String,
        version_code: u64,
        version_name: String,
        variant_name: String,
    },
}

impl ResolvedMapping {
    pub fn notice(&self) -> String {
        match &self.source {
            MappingSource::Manual => {
                format!("Using mapping: {}", self.symbolicator.info.path.display())
            }
            MappingSource::Auto {
                package,
                version_code,
                version_name,
                variant_name,
            } => format!(
                "Using mapping: {}\nMatched package {package} versionCode={version_code} versionName={version_name} variant={variant_name}",
                self.symbolicator.info.path.display()
            ),
        }
    }
}

impl Symbolicator {
    pub fn from_file(path: &Path) -> Result<Self, HprofSlurpError> {
        if !path.is_file() {
            return Err(HprofSlurpError::MappingFileNotFound {
                path: path.display().to_string(),
            });
        }
        let text = std::fs::read_to_string(path)?;
        Self::parse_text(path, &text)
    }

    pub fn parse_text(path: &Path, text: &str) -> Result<Self, HprofSlurpError> {
        let mut out = Self {
            info: MappingInfo {
                path: path.to_path_buf(),
                pg_map_id: None,
                pg_map_hash: None,
            },
            class_by_obfuscated: AHashMap::new(),
            fields_by_obfuscated_class: AHashMap::new(),
        };
        let mut current_obfuscated_class: Option<String> = None;

        for (idx, raw_line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let line = raw_line.trim_end();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("# pg_map_id:") {
                out.info.pg_map_id = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("# pg_map_hash:") {
                out.info.pg_map_hash = Some(value.trim().to_string());
                continue;
            }
            if trimmed.starts_with('#') {
                continue;
            }

            if !raw_line.starts_with(' ') && !raw_line.starts_with('\t') {
                let Some((original, obfuscated_with_colon)) = trimmed.split_once(" -> ") else {
                    return Err(invalid(path, line_no, "expected class mapping with ` -> `"));
                };
                let Some(obfuscated) = obfuscated_with_colon.strip_suffix(':') else {
                    return Err(invalid(path, line_no, "class mapping must end with `:`"));
                };
                out.class_by_obfuscated
                    .insert(obfuscated.to_string(), original.to_string());
                current_obfuscated_class = Some(obfuscated.to_string());
                continue;
            }

            let Some(class_name) = current_obfuscated_class.as_ref() else {
                continue;
            };
            if trimmed.contains('(') {
                continue;
            }
            let Some((left, obfuscated)) = trimmed.split_once(" -> ") else {
                continue;
            };
            let Some(original_field) = left.split_whitespace().last() else {
                continue;
            };
            out.fields_by_obfuscated_class
                .entry(class_name.clone())
                .or_default()
                .insert(obfuscated.to_string(), original_field.to_string());
        }

        Ok(out)
    }

    pub fn class_name(&self, raw: &str) -> String {
        let suffix = raw
            .strip_suffix("[][]")
            .map(|base| (base, "[][]"))
            .or_else(|| raw.strip_suffix("[]").map(|base| (base, "[]")));
        if let Some((base, suffix)) = suffix {
            return self
                .class_by_obfuscated
                .get(base)
                .map(|name| format!("{name}{suffix}"))
                .unwrap_or_else(|| raw.to_string());
        }
        self.class_by_obfuscated
            .get(raw)
            .cloned()
            .unwrap_or_else(|| raw.to_string())
    }

    pub fn field_name(&self, raw_holder_class: &str, raw_field: &str) -> String {
        self.fields_by_obfuscated_class
            .get(raw_holder_class)
            .and_then(|fields| fields.get(raw_field))
            .cloned()
            .unwrap_or_else(|| raw_field.to_string())
    }

}

fn invalid(path: &Path, line: usize, message: &str) -> HprofSlurpError {
    HprofSlurpError::InvalidMapping {
        path: path.display().to_string(),
        line,
        message: message.to_string(),
    }
}

pub fn resolve_mapping(
    options: &MappingOptions,
) -> Result<Option<ResolvedMapping>, HprofSlurpError> {
    if let Some(path) = &options.mapping {
        let symbolicator = Symbolicator::from_file(Path::new(path))?;
        return Ok(Some(ResolvedMapping {
            symbolicator,
            source: MappingSource::Manual,
        }));
    }
    if let Some(mode) = options.auto_mapping {
        let project_root =
            options
                .project_root
                .as_deref()
                .ok_or_else(|| HprofSlurpError::MappingDiscovery {
                    message: "--auto-mapping requires --project-root".to_string(),
                })?;
        let package =
            options
                .package
                .as_deref()
                .ok_or_else(|| HprofSlurpError::MappingDiscovery {
                    message: "--auto-mapping requires --package".to_string(),
                })?;
        let discovered = match crate::mapping_discovery::query_device_version(
            options.serial.as_deref(),
            package,
        )
        .and_then(|device| {
            crate::mapping_discovery::find_local_mapping(Path::new(project_root), package, &device)
        }) {
            Ok(discovered) => discovered,
            Err(err) if mode == crate::args::AutoMappingMode::Optional => {
                eprintln!("warning: {err}");
                return Ok(None);
            }
            Err(err) => return Err(err),
        };
        let symbolicator = Symbolicator::from_file(&discovered.mapping_path)?;
        return Ok(Some(ResolvedMapping {
            symbolicator,
            source: MappingSource::Auto {
                package: discovered.package,
                version_code: discovered.version_code,
                version_name: discovered.version_name,
                variant_name: discovered.variant_name,
            },
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_class_field_and_r8_metadata() {
        let text = r#"
# compiler: R8
# pg_map_id: abc123
# pg_map_hash: SHA-256 deadbeef
com.nexio.tv.domain.model.MetaPreview -> d1.q2:
    java.lang.String title -> a
    int count -> b
    1:4:void render():12:15 -> c
"#;

        let parsed = Symbolicator::parse_text(Path::new("mapping.txt"), text).unwrap();

        assert_eq!(parsed.info.pg_map_id.as_deref(), Some("abc123"));
        assert_eq!(
            parsed.info.pg_map_hash.as_deref(),
            Some("SHA-256 deadbeef")
        );
        assert_eq!(
            parsed.class_name("d1.q2"),
            "com.nexio.tv.domain.model.MetaPreview"
        );
        assert_eq!(
            parsed.class_name("d1.q2[]"),
            "com.nexio.tv.domain.model.MetaPreview[]"
        );
        assert_eq!(
            parsed.class_name("d1.q2[][]"),
            "com.nexio.tv.domain.model.MetaPreview[][]"
        );
        assert_eq!(parsed.field_name("d1.q2", "a"), "title");
        assert_eq!(parsed.field_name("d1.q2", "missing"), "missing");
        assert_eq!(parsed.field_name("unknown.Class", "a"), "a");
    }

    #[test]
    fn leaves_primitives_and_unknown_classes_unchanged() {
        let parsed = Symbolicator::parse_text(
            Path::new("mapping.txt"),
            "com.example.Real -> a.b:\n    java.lang.String name -> c\n",
        )
        .unwrap();

        assert_eq!(parsed.class_name("byte[]"), "byte[]");
        assert_eq!(parsed.class_name("char[]"), "char[]");
        assert_eq!(parsed.class_name("unknown.Name[]"), "unknown.Name[]");
    }

    #[test]
    fn malformed_class_line_reports_line_number() {
        let err = Symbolicator::parse_text(Path::new("mapping.txt"), "bad -> line\n").unwrap_err();
        assert!(
            err.to_string().contains("line 1"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolves_manual_mapping_from_file() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-mapping-test-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mapping.txt");
        std::fs::write(&path, "com.example.Real -> a.b:\n").unwrap();

        let resolved = resolve_mapping(&crate::args::MappingOptions {
            mapping: Some(path.display().to_string()),
            auto_mapping: None,
            project_root: None,
            package: None,
            serial: None,
        })
        .unwrap()
        .unwrap();

        assert_eq!(resolved.symbolicator.class_name("a.b"), "com.example.Real");
        assert!(resolved.notice().contains("Using mapping:"));
    }
}
