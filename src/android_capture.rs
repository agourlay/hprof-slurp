use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde::Serialize;

use crate::errors::HprofSlurpError;

#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub serial: Option<String>,
    pub package: String,
    pub out_dir: PathBuf,
    pub allocation_sites: bool,
    pub foreground: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandRunner {
    fn run(&mut self, program: &str, args: &[String]) -> Result<CommandOutput, HprofSlurpError>;
}

#[derive(Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&mut self, program: &str, args: &[String]) -> Result<CommandOutput, HprofSlurpError> {
        let output = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct CaptureReport {
    pub package: String,
    pub serial: Option<String>,
    pub pid: String,
    pub foreground_requested: bool,
    pub allocation_sites_requested: bool,
    pub local_hprof: PathBuf,
    pub transcript: PathBuf,
    pub dump_size_bytes: u64,
    pub allocation_sites_present: bool,
}

#[derive(Debug, Clone)]
struct CommandRecord {
    command: String,
    status: i32,
    stdout: String,
    stderr: String,
}

pub fn run(options: CaptureOptions) -> Result<CaptureReport, HprofSlurpError> {
    let mut runner = SystemRunner;
    run_with_runner(options, &mut runner)
}

pub fn run_with_runner<R: CommandRunner>(
    options: CaptureOptions,
    runner: &mut R,
) -> Result<CaptureReport, HprofSlurpError> {
    let mut transcript = Vec::new();
    std::fs::create_dir_all(&options.out_dir)?;

    let pid_output = adb_checked(
        &options.serial,
        runner,
        &["shell", "pidof", &options.package],
        &mut transcript,
    )?;
    let pid = pid_output.stdout.trim().to_string();
    if pid.is_empty() {
        return Err(HprofSlurpError::AndroidCapture {
            message: format!("pidof returned no pid for package {}", options.package),
        });
    }

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let base_name = format!("{}-{}.hprof", sanitize_package(&options.package), timestamp);
    let local_hprof = options.out_dir.join(&base_name);
    let transcript_path = options.out_dir.join(format!(
        "{}-{}-transcript.txt",
        sanitize_package(&options.package),
        timestamp
    ));

    write_transcript(
        &transcript_path,
        &options,
        &pid,
        "",
        &local_hprof,
        0,
        false,
        &transcript,
    )?;

    Ok(CaptureReport {
        package: options.package,
        serial: options.serial,
        pid,
        foreground_requested: options.foreground,
        allocation_sites_requested: options.allocation_sites,
        local_hprof,
        transcript: transcript_path,
        dump_size_bytes: 0,
        allocation_sites_present: false,
    })
}

fn adb_args(serial: &Option<String>, args: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(serial) = serial {
        out.push("-s".to_string());
        out.push(serial.clone());
    }
    out.extend(args.iter().map(|arg| (*arg).to_string()));
    out
}

fn adb_checked<R: CommandRunner>(
    serial: &Option<String>,
    runner: &mut R,
    args: &[&str],
    transcript: &mut Vec<CommandRecord>,
) -> Result<CommandOutput, HprofSlurpError> {
    let full_args = adb_args(serial, args);
    let command = format!("adb {}", full_args.join(" "));
    let output = runner.run("adb", &full_args)?;
    transcript.push(CommandRecord {
        command: command.clone(),
        status: output.status,
        stdout: output.stdout.clone(),
        stderr: output.stderr.clone(),
    });
    if output.status != 0 {
        return Err(HprofSlurpError::AdbCommandFailed {
            command,
            status: output.status,
            stderr: output.stderr,
        });
    }
    Ok(output)
}

fn sanitize_package(package: &str) -> String {
    package
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn write_transcript(
    path: &Path,
    options: &CaptureOptions,
    pid: &str,
    foreground_evidence: &str,
    local_hprof: &Path,
    dump_size_bytes: u64,
    allocation_sites_present: bool,
    commands: &[CommandRecord],
) -> Result<(), HprofSlurpError> {
    let mut text = String::new();
    text.push_str("heaptrail android-capture transcript\n\n");
    text.push_str(&format!("package: {}\n", options.package));
    text.push_str(&format!(
        "serial: {}\n",
        options.serial.as_deref().unwrap_or("(adb default)")
    ));
    text.push_str(&format!("pid: {pid}\n"));
    text.push_str(&format!("foreground_requested: {}\n", options.foreground));
    text.push_str(&format!(
        "allocation_sites_requested: {}\n",
        options.allocation_sites
    ));
    text.push_str(&format!("local_hprof: {}\n", local_hprof.display()));
    text.push_str(&format!("dump_size_bytes: {dump_size_bytes}\n"));
    text.push_str(&format!(
        "allocation_sites_present: {allocation_sites_present}\n"
    ));
    text.push_str("\nforeground_evidence:\n");
    text.push_str(foreground_evidence);
    text.push_str("\n\ncommands:\n");
    for command in commands {
        text.push_str(&format!("$ {}\n", command.command));
        text.push_str(&format!("status: {}\n", command.status));
        if !command.stdout.trim().is_empty() {
            text.push_str("stdout:\n");
            text.push_str(command.stdout.trim_end());
            text.push('\n');
        }
        if !command.stderr.trim().is_empty() {
            text.push_str("stderr:\n");
            text.push_str(command.stderr.trim_end());
            text.push('\n');
        }
        text.push('\n');
    }
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeRunner {
        calls: Vec<Vec<String>>,
        outputs: VecDeque<CommandOutput>,
    }

    impl FakeRunner {
        fn push(&mut self, status: i32, stdout: &str, stderr: &str) {
            self.outputs.push_back(CommandOutput {
                status,
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            });
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &mut self,
            _program: &str,
            args: &[String],
        ) -> Result<CommandOutput, HprofSlurpError> {
            self.calls.push(args.to_vec());
            Ok(self.outputs.pop_front().expect("missing fake output"))
        }
    }

    #[test]
    fn pidof_failure_is_actionable() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-capture-test-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        let mut runner = FakeRunner::default();
        runner.push(1, "", "not found");

        let err = run_with_runner(
            CaptureOptions {
                serial: None,
                package: "com.example.app".to_string(),
                out_dir: dir,
                allocation_sites: false,
                foreground: false,
            },
            &mut runner,
        )
        .unwrap_err();

        match err {
            HprofSlurpError::AdbCommandFailed {
                command, stderr, ..
            } => {
                assert!(command.contains("pidof"));
                assert_eq!(stderr, "not found");
            }
            other => panic!("expected AdbCommandFailed, got {other:?}"),
        }
    }
}
