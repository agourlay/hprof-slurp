use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;

use crate::errors::HprofSlurpError;

const DEVICE_DUMP_STALL_TIMEOUT: Duration = Duration::from_secs(60);
const DEVICE_DUMP_READY_POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct CaptureOptions {
    pub serial: Option<String>,
    pub package: String,
    pub out_dir: PathBuf,
    pub allocation_sites: bool,
    pub foreground: bool,
    pub mapping: Option<crate::mapping::ResolvedMapping>,
    pub series_count: usize,
    pub series_delay_seconds: u64,
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
    pub local_hprofs: Vec<PathBuf>,
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
    let series_count = options.series_count.max(1);
    let first_base_name = capture_base_name(&options.package, &timestamp, 0, series_count);
    let first_local_hprof = options.out_dir.join(&first_base_name);
    let transcript_path = options.out_dir.join(format!(
        "{}-{}-transcript.txt",
        sanitize_package(&options.package),
        timestamp
    ));

    if options.foreground {
        adb_checked(
            &options.serial,
            runner,
            &["shell", "monkey", "-p", &options.package, "1"],
            &mut transcript,
        )?;
    }

    let focus_output = adb_checked(
        &options.serial,
        runner,
        &["shell", "dumpsys", "window"],
        &mut transcript,
    )?;
    let foreground_evidence = extract_focus_lines(&focus_output.stdout);

    let mut local_hprofs = Vec::new();
    let mut dump_size_bytes = 0;
    let mut any_allocation_sites_present = false;

    for index in 0..series_count {
        let base_name = capture_base_name(&options.package, &timestamp, index, series_count);
        let device_hprof = format!("/data/local/tmp/{base_name}");
        let local_hprof = options.out_dir.join(&base_name);
        match capture_one(
            &options,
            runner,
            &pid,
            &device_hprof,
            &local_hprof,
            &mut transcript,
        ) {
            Ok(size) => {
                dump_size_bytes += size;
                any_allocation_sites_present |= allocation_sites_present(&local_hprof)?;
                local_hprofs.push(local_hprof);
            }
            Err(err) => {
                write_transcript(
                    &transcript_path,
                    &options,
                    &pid,
                    &foreground_evidence,
                    &first_local_hprof,
                    dump_size_bytes,
                    any_allocation_sites_present,
                    &transcript,
                )?;
                return Err(err);
            }
        }

        if index + 1 < series_count && options.series_delay_seconds > 0 {
            std::thread::sleep(Duration::from_secs(options.series_delay_seconds));
        }
    }

    write_transcript(
        &transcript_path,
        &options,
        &pid,
        &foreground_evidence,
        &first_local_hprof,
        dump_size_bytes,
        any_allocation_sites_present,
        &transcript,
    )?;

    let local_hprof = local_hprofs
        .first()
        .cloned()
        .unwrap_or_else(|| first_local_hprof.clone());

    Ok(CaptureReport {
        package: options.package,
        serial: options.serial,
        pid,
        foreground_requested: options.foreground,
        allocation_sites_requested: options.allocation_sites,
        local_hprof,
        local_hprofs,
        transcript: transcript_path,
        dump_size_bytes,
        allocation_sites_present: any_allocation_sites_present,
    })
}

fn capture_base_name(package: &str, timestamp: &str, index: usize, series_count: usize) -> String {
    let package = sanitize_package(package);
    if series_count <= 1 {
        format!("{package}-{timestamp}.hprof")
    } else {
        format!("{package}-{timestamp}-{:02}.hprof", index + 1)
    }
}

fn capture_one<R: CommandRunner>(
    options: &CaptureOptions,
    runner: &mut R,
    pid: &str,
    device_hprof: &str,
    local_hprof: &Path,
    transcript: &mut Vec<CommandRecord>,
) -> Result<u64, HprofSlurpError> {
    let mut profile_started = false;
    if options.allocation_sites {
        adb_checked(
            &options.serial,
            runner,
            &[
                "shell",
                "am",
                "profile",
                "start",
                pid,
                "/data/local/tmp/heaptrail-alloc.trace",
            ],
            transcript,
        )?;
        profile_started = true;
    }

    let dump_result = adb_checked(
        &options.serial,
        runner,
        &["shell", "am", "dumpheap", pid, device_hprof],
        transcript,
    );
    if profile_started {
        let _ = adb_recorded(
            &options.serial,
            runner,
            &["shell", "am", "profile", "stop", pid],
            transcript,
        );
    }
    dump_result?;

    if let Err(err) = wait_for_device_dump_ready(
        &options.serial,
        runner,
        device_hprof,
        transcript,
        DEVICE_DUMP_STALL_TIMEOUT,
    ) {
        return Err(capture_failure(err, options.allocation_sites));
    }

    adb_checked(
        &options.serial,
        runner,
        &["pull", device_hprof, local_hprof.to_string_lossy().as_ref()],
        transcript,
    )?;

    let dump_size_bytes = std::fs::metadata(local_hprof)?.len();
    if dump_size_bytes == 0 {
        return Err(capture_failure(
            HprofSlurpError::AndroidCapture {
                message: format!("captured hprof is 0 bytes: {}", local_hprof.display()),
            },
            options.allocation_sites,
        ));
    }
    Ok(dump_size_bytes)
}

fn capture_failure(err: HprofSlurpError, allocation_sites: bool) -> HprofSlurpError {
    if !allocation_sites {
        return err;
    }
    match err {
        HprofSlurpError::AndroidCapture { message } => HprofSlurpError::AndroidCapture {
            message: format!(
                "allocation-tracked dump unsupported/failed on this device: {message}"
            ),
        },
        other => other,
    }
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

fn adb_recorded<R: CommandRunner>(
    serial: &Option<String>,
    runner: &mut R,
    args: &[&str],
    transcript: &mut Vec<CommandRecord>,
) -> Result<CommandOutput, HprofSlurpError> {
    let full_args = adb_args(serial, args);
    let command = format!("adb {}", full_args.join(" "));
    let output = runner.run("adb", &full_args)?;
    transcript.push(CommandRecord {
        command,
        status: output.status,
        stdout: output.stdout.clone(),
        stderr: output.stderr.clone(),
    });
    Ok(output)
}

fn wait_for_device_dump_ready<R: CommandRunner>(
    serial: &Option<String>,
    runner: &mut R,
    device_hprof: &str,
    transcript: &mut Vec<CommandRecord>,
    stall_timeout: Duration,
) -> Result<u64, HprofSlurpError> {
    let mut last_size = None;
    let mut last_change = Instant::now();

    loop {
        let now = Instant::now();
        let output = adb_recorded(
            serial,
            runner,
            &["shell", "ls", "-l", device_hprof],
            transcript,
        )?;
        if output.status == 0
            && let Some(size) = parse_ls_size(&output.stdout)
        {
            if last_size == Some(size) && size > 0 {
                return Ok(size);
            }
            if last_size != Some(size) {
                last_size = Some(size);
                last_change = now;
            }
        }

        if now.duration_since(last_change) >= stall_timeout {
            let last_size = last_size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return Err(HprofSlurpError::AndroidCapture {
                message: format!(
                    "device hprof size did not change for {}s while waiting for dump to finish: {device_hprof} (last observed size: {last_size} bytes)",
                    stall_timeout.as_secs()
                ),
            });
        }

        std::thread::sleep(DEVICE_DUMP_READY_POLL);
    }
}

fn parse_ls_size(stdout: &str) -> Option<u64> {
    stdout.lines().find_map(|line| {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 5 {
            return None;
        }
        parts.get(4)?.parse().ok()
    })
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

fn extract_focus_lines(dumpsys_window: &str) -> String {
    dumpsys_window
        .lines()
        .filter(|line| {
            line.contains("mCurrentFocus")
                || line.contains("mFocusedApp")
                || line.contains("topResumedActivity")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn allocation_sites_present(path: &Path) -> Result<bool, HprofSlurpError> {
    let rendered = crate::slurp::slurp_file_with_modes(
        path.to_string_lossy().as_ref(),
        false,
        false,
        0,
        1024,
        false,
        false,
    )?;
    Ok(!rendered.allocation_sites.is_empty() || rendered.allocation_sites_record_count > 0)
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
    if let Some(mapping) = &options.mapping {
        text.push_str(&format!(
            "mapping_path: {}\n",
            mapping.symbolicator.info.path.display()
        ));
        let source = match &mapping.source {
            crate::mapping::MappingSource::Manual => "manual",
            crate::mapping::MappingSource::Auto { .. } => "auto",
        };
        text.push_str(&format!("mapping_source: {source}\n"));
        if let Some(id) = &mapping.symbolicator.info.pg_map_id {
            text.push_str(&format!("pg_map_id: {id}\n"));
        }
        if let Some(hash) = &mapping.symbolicator.info.pg_map_hash {
            text.push_str(&format!("pg_map_hash: {hash}\n"));
        }
    } else {
        text.push_str("mapping_path: (none)\n");
    }
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
        on_pull_write: Option<Vec<u8>>,
        on_pull_writes: VecDeque<Vec<u8>>,
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
            if args.iter().any(|arg| arg == "pull")
                && let Some(bytes) = self
                    .on_pull_write
                    .take()
                    .or_else(|| self.on_pull_writes.pop_front())
            {
                let dest = args.last().expect("pull destination");
                std::fs::write(dest, bytes)?;
            }
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
                mapping: None,
                series_count: 1,
                series_delay_seconds: 0,
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

    #[test]
    fn successful_capture_pulls_nonzero_dump_and_writes_transcript() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-capture-success-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut runner = FakeRunner::default();
        runner.push(0, "1234\n", "");
        runner.push(
            0,
            "mCurrentFocus=Window{ com.example.app/.MainActivity }\n",
            "",
        );
        runner.push(0, "", "");
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(0, "", "");

        let fixture = std::fs::read("test-heap-dumps/hprof-64.bin").unwrap();
        runner.on_pull_write = Some(fixture);

        let report = run_with_runner(
            CaptureOptions {
                serial: Some("device-1".to_string()),
                package: "com.example.app".to_string(),
                out_dir: dir.clone(),
                allocation_sites: false,
                foreground: false,
                mapping: None,
                series_count: 1,
                series_delay_seconds: 0,
            },
            &mut runner,
        )
        .unwrap();

        assert_eq!(report.pid, "1234");
        assert!(report.dump_size_bytes > 0);
        assert!(report.local_hprof.is_file());
        assert!(report.transcript.is_file());
        let transcript = std::fs::read_to_string(report.transcript).unwrap();
        assert!(transcript.contains("mCurrentFocus"));
        assert!(transcript.contains("allocation_sites_present: false"));
    }

    #[test]
    fn zero_byte_capture_fails_with_transcript() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-capture-zero-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut runner = FakeRunner::default();
        runner.push(0, "1234\n", "");
        runner.push(
            0,
            "mFocusedApp=ActivityRecord{ com.example.app/.MainActivity }\n",
            "",
        );
        runner.push(0, "", "");
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(0, "", "");
        runner.on_pull_write = Some(Vec::new());

        let err = run_with_runner(
            CaptureOptions {
                serial: None,
                package: "com.example.app".to_string(),
                out_dir: dir,
                allocation_sites: false,
                foreground: false,
                mapping: None,
                series_count: 1,
                series_delay_seconds: 0,
            },
            &mut runner,
        )
        .unwrap_err();

        match err {
            HprofSlurpError::AndroidCapture { message } => {
                assert!(message.contains("0 bytes"), "got: {message}");
            }
            other => panic!("expected AndroidCapture, got {other:?}"),
        }
    }

    #[test]
    fn foreground_and_allocation_tracking_commands_are_recorded() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-capture-alloc-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut runner = FakeRunner::default();
        runner.push(0, "1234\n", "");
        runner.push(0, "", "");
        runner.push(0, "topResumedActivity=com.example.app/.MainActivity\n", "");
        runner.push(0, "", "");
        runner.push(0, "", "");
        runner.push(0, "", "");
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(0, "", "");
        runner.on_pull_write = Some(std::fs::read("test-heap-dumps/hprof-64.bin").unwrap());

        let _ = run_with_runner(
            CaptureOptions {
                serial: None,
                package: "com.example.app".to_string(),
                out_dir: dir,
                allocation_sites: true,
                foreground: true,
                mapping: None,
                series_count: 1,
                series_delay_seconds: 0,
            },
            &mut runner,
        )
        .unwrap();

        let calls = runner
            .calls
            .iter()
            .map(|args| args.join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            calls.contains("shell monkey -p com.example.app 1"),
            "{calls}"
        );
        assert!(
            calls.contains("shell am profile start 1234 /data/local/tmp/heaptrail-alloc.trace"),
            "{calls}"
        );
        assert!(calls.contains("shell am profile stop 1234"), "{calls}");
        assert!(
            calls.contains("shell am dumpheap 1234 /data/local/tmp/"),
            "{calls}"
        );
    }

    #[test]
    fn allocation_tracking_stops_even_when_dumpheap_fails() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-capture-alloc-fail-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut runner = FakeRunner::default();
        runner.push(0, "1234\n", "");
        runner.push(0, "mCurrentFocus=Window{ com.example/.MainActivity }\n", "");
        runner.push(0, "", "");
        runner.push(1, "", "dump failed");
        runner.push(0, "", "");

        let err = run_with_runner(
            CaptureOptions {
                serial: None,
                package: "com.example.app".to_string(),
                out_dir: dir,
                allocation_sites: true,
                foreground: false,
                mapping: None,
                series_count: 1,
                series_delay_seconds: 0,
            },
            &mut runner,
        )
        .unwrap_err();

        match err {
            HprofSlurpError::AdbCommandFailed { stderr, .. } => {
                assert_eq!(stderr, "dump failed");
            }
            other => panic!("expected AdbCommandFailed, got {other:?}"),
        }
        let calls = runner
            .calls
            .iter()
            .map(|args| args.join(" "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(calls.contains("shell am profile stop 1234"), "{calls}");
    }

    #[test]
    fn series_capture_collects_multiple_hprofs() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-capture-series-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut runner = FakeRunner::default();
        runner.push(0, "1234\n", "");
        runner.push(0, "mCurrentFocus=Window{ com.example/.MainActivity }\n", "");
        let fixture = std::fs::read("test-heap-dumps/hprof-64.bin").unwrap();
        for _ in 0..3 {
            runner.push(0, "", "");
            runner.push(
                0,
                "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
                "",
            );
            runner.push(
                0,
                "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
                "",
            );
            runner.push(0, "", "");
            runner.on_pull_writes.push_back(fixture.clone());
        }

        let report = run_with_runner(
            CaptureOptions {
                serial: None,
                package: "com.example.app".to_string(),
                out_dir: dir,
                allocation_sites: false,
                foreground: false,
                mapping: None,
                series_count: 3,
                series_delay_seconds: 0,
            },
            &mut runner,
        )
        .unwrap();

        assert_eq!(report.local_hprofs.len(), 3);
        assert!(
            report.local_hprofs[0]
                .display()
                .to_string()
                .contains("-01.hprof")
        );
        assert!(
            report.local_hprofs[2]
                .display()
                .to_string()
                .contains("-03.hprof")
        );
        assert!(report.local_hprofs.iter().all(|path| path.is_file()));
    }

    #[test]
    fn transcript_includes_mapping_metadata_when_present() {
        let dir = std::env::temp_dir().join(format!(
            "heaptrail-capture-mapping-{}",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mapping_path = dir.join("mapping.txt");
        std::fs::write(
            &mapping_path,
            "# pg_map_id: abc123\n# pg_map_hash: SHA-256 deadbeef\ncom.example.Real -> a.b:\n",
        )
        .unwrap();
        let mut runner = FakeRunner::default();
        runner.push(0, "1234\n", "");
        runner.push(0, "mCurrentFocus=Window{ com.example/.MainActivity }\n", "");
        runner.push(0, "", "");
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(
            0,
            "-rwxrwx--- 1 root root 1024 2026-05-24 20:56 /data/local/tmp/heap.hprof\n",
            "",
        );
        runner.push(0, "", "");
        runner.on_pull_write = Some(std::fs::read("test-heap-dumps/hprof-64.bin").unwrap());

        let report = run_with_runner(
            CaptureOptions {
                serial: None,
                package: "com.example.app".to_string(),
                out_dir: dir,
                allocation_sites: false,
                foreground: false,
                mapping: Some(crate::mapping::ResolvedMapping {
                    symbolicator: crate::mapping::Symbolicator::from_file(&mapping_path).unwrap(),
                    source: crate::mapping::MappingSource::Manual,
                }),
                series_count: 1,
                series_delay_seconds: 0,
            },
            &mut runner,
        )
        .unwrap();

        let transcript = std::fs::read_to_string(report.transcript).unwrap();
        assert!(transcript.contains("mapping_path:"));
        assert!(transcript.contains("pg_map_id: abc123"));
        assert!(transcript.contains("pg_map_hash: SHA-256 deadbeef"));
    }

    #[test]
    fn extract_focus_lines_keeps_only_relevant_window_lines() {
        let input = "\
irrelevant
mCurrentFocus=Window{ com.example/.MainActivity }
mFocusedApp=ActivityRecord{ com.example/.MainActivity }
topResumedActivity=ActivityRecord{ com.example/.MainActivity }
other";

        let actual = extract_focus_lines(input);

        assert!(actual.contains("mCurrentFocus"));
        assert!(actual.contains("mFocusedApp"));
        assert!(actual.contains("topResumedActivity"));
        assert!(!actual.contains("irrelevant"));
        assert!(!actual.contains("other"));
    }

    #[test]
    fn parse_ls_size_reads_android_toybox_output() {
        let stdout = "-rwxrwx--- 1 root root 82133672 2026-05-24 20:56 /data/local/tmp/app.hprof\n";

        assert_eq!(parse_ls_size(stdout), Some(82133672));
    }

    #[test]
    fn device_dump_wait_fails_when_size_stalls() {
        let mut runner = FakeRunner::default();
        runner.push(
            0,
            "-rwxrwx--- 1 root root 0 2026-05-24 20:56 /data/local/tmp/app.hprof\n",
            "",
        );
        let mut transcript = Vec::new();

        let err = wait_for_device_dump_ready(
            &None,
            &mut runner,
            "/data/local/tmp/app.hprof",
            &mut transcript,
            Duration::ZERO,
        )
        .unwrap_err();

        match err {
            HprofSlurpError::AndroidCapture { message } => {
                assert!(message.contains("size did not change"), "got: {message}");
                assert!(message.contains("last observed size: 0"), "got: {message}");
            }
            other => panic!("expected AndroidCapture, got {other:?}"),
        }
        assert_eq!(transcript.len(), 1);
        assert!(
            transcript[0]
                .command
                .contains("ls -l /data/local/tmp/app.hprof")
        );
    }
}
