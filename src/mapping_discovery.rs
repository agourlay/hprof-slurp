use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::errors::HprofSlurpError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceVersion {
    pub version_code: u64,
    pub version_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMapping {
    pub package: String,
    pub version_code: u64,
    pub version_name: String,
    pub variant_name: String,
    pub metadata_path: PathBuf,
    pub mapping_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct OutputMetadata {
    #[serde(rename = "applicationId")]
    application_id: String,
    #[serde(rename = "variantName")]
    variant_name: String,
    elements: Vec<OutputElement>,
}

#[derive(Debug, Deserialize)]
struct OutputElement {
    #[serde(rename = "versionCode")]
    version_code: u64,
    #[serde(rename = "versionName")]
    version_name: String,
}

pub fn parse_device_version(text: &str) -> Result<DeviceVersion, HprofSlurpError> {
    let version_code = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("versionCode="))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| discovery_err("could not parse versionCode from dumpsys package output"))?;
    let version_name = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("versionName="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| discovery_err("could not parse versionName from dumpsys package output"))?
        .to_string();
    Ok(DeviceVersion {
        version_code,
        version_name,
    })
}

pub fn find_local_mapping(
    project_root: &Path,
    package: &str,
    device: &DeviceVersion,
) -> Result<DiscoveredMapping, HprofSlurpError> {
    let apk_root = project_root.join("app/build/outputs/apk");
    let mut metadata_files = Vec::new();
    collect_metadata_files(&apk_root, &mut metadata_files)?;
    let mut matches = Vec::new();
    for metadata_path in metadata_files {
        let text = std::fs::read_to_string(&metadata_path)?;
        let metadata: OutputMetadata = serde_json::from_str(&text)?;
        let Some(element) = metadata.elements.first() else {
            continue;
        };
        if metadata.application_id == package
            && element.version_code == device.version_code
            && element.version_name == device.version_name
        {
            let mapping_path = project_root
                .join("app/build/outputs/mapping")
                .join(&metadata.variant_name)
                .join("mapping.txt");
            matches.push(DiscoveredMapping {
                package: package.to_string(),
                version_code: device.version_code,
                version_name: device.version_name.clone(),
                variant_name: metadata.variant_name,
                metadata_path,
                mapping_path,
            });
        }
    }
    match matches.len() {
        0 => Err(discovery_err(
            "no Gradle output metadata matched installed app version",
        )),
        1 => {
            let found = matches.remove(0);
            if !found.mapping_path.is_file() {
                return Err(discovery_err(&format!(
                    "matched {} but expected mapping file is missing: {}",
                    found.metadata_path.display(),
                    found.mapping_path.display()
                )));
            }
            Ok(found)
        }
        _ => Err(discovery_err(&format!(
            "multiple Gradle outputs matched installed app version; use --mapping explicitly: {}",
            matches
                .iter()
                .map(|m| m.mapping_path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub fn query_device_version(
    serial: Option<&str>,
    package: &str,
) -> Result<DeviceVersion, HprofSlurpError> {
    let mut args = Vec::new();
    if let Some(serial) = serial {
        args.push("-s".to_string());
        args.push(serial.to_string());
    }
    args.extend([
        "shell".to_string(),
        "dumpsys".to_string(),
        "package".to_string(),
        package.to_string(),
    ]);
    let output = Command::new("adb").args(&args).output()?;
    if !output.status.success() {
        return Err(discovery_err(&format!(
            "adb dumpsys package failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    parse_device_version(&String::from_utf8_lossy(&output.stdout))
}

fn collect_metadata_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), HprofSlurpError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_metadata_files(&path, out)?;
        } else if path.file_name().and_then(|n| n.to_str()) == Some("output-metadata.json") {
            out.push(path);
        }
    }
    Ok(())
}

fn discovery_err(message: &str) -> HprofSlurpError {
    HprofSlurpError::MappingDiscovery {
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dumpsys_package_version() {
        let text = r#"
Packages:
  Package [com.nexio.tv] (abc):
    versionCode=77 minSdk=26 targetSdk=36
    versionName=0.59
"#;

        let version = parse_device_version(text).unwrap();

        assert_eq!(version.version_code, 77);
        assert_eq!(version.version_name, "0.59");
    }

    #[test]
    fn matches_gradle_metadata_to_mapping_path() {
        let root = std::env::temp_dir().join(format!(
            "heaptrail-discovery-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        let apk_dir = root.join("app/build/outputs/apk/universal/release");
        let mapping_dir = root.join("app/build/outputs/mapping/universalRelease");
        std::fs::create_dir_all(&apk_dir).unwrap();
        std::fs::create_dir_all(&mapping_dir).unwrap();
        std::fs::write(
            mapping_dir.join("mapping.txt"),
            "com.example.Real -> a.b:\n",
        )
        .unwrap();
        std::fs::write(
            apk_dir.join("output-metadata.json"),
            r#"{
              "applicationId": "com.nexio.tv",
              "variantName": "universalRelease",
              "elements": [
                {"versionCode": 77, "versionName": "0.59", "outputFile": "nexio-release.apk"}
              ]
            }"#,
        )
        .unwrap();

        let result = find_local_mapping(
            &root,
            "com.nexio.tv",
            &DeviceVersion {
                version_code: 77,
                version_name: "0.59".to_string(),
            },
        )
        .unwrap();

        assert_eq!(result.variant_name, "universalRelease");
        assert!(
            result
                .mapping_path
                .ends_with("app/build/outputs/mapping/universalRelease/mapping.txt")
        );
    }
}
