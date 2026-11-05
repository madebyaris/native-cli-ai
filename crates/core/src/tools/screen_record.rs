//! Records one macOS app window to a .mov file, including while the window is in the
//! background. `start` runs an open-ended `screencapture -v -l <window id>` and a watchdog
//! thread that interrupts it at the time limit. `stop` sends SIGINT and waits for the file
//! to be written. Timed `-V` recordings are not used, because they do not finish their file
//! when interrupted. Neither action activates an app. Files go to the recordings folder from
//! `[recording]` in the config, by default the project's folder under the nca data root.
//! Plan: `docs/plans/macos-window-recording.md`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use nca_common::tool::{ToolCall, ToolDefinition, ToolResult};
use serde_json::Value;

use super::ToolExecutor;
use super::app_input::{WindowRef, focus_note, frontmost, main_window_of, required_str};

const DEFAULT_SECONDS: u64 = 60;
const MAX_SECONDS: u64 = 600;
const DEFAULT_SCREENSHOT_DELAY: u64 = 3;
const MAX_SCREENSHOT_DELAY: u64 = 30;
const WINDOW_WAIT_ATTEMPTS: u32 = 20;
const WINDOW_WAIT_STEP: Duration = Duration::from_millis(150);
/// Shortest recording before an interrupt is sent. An interrupt that arrives while screencapture
/// is still starting can be lost, or can kill it before it writes a file, so `stop` waits this
/// long first.
const MIN_RECORDING: Duration = Duration::from_secs(2);
/// How long to wait for screencapture to exit after each interrupt before interrupting again.
const INTERRUPT_WAIT: Duration = Duration::from_secs(3);
/// Interrupts to send before killing screencapture.
const INTERRUPT_ATTEMPTS: u32 = 4;

#[allow(non_snake_case)]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// Tool that records one app window in the background. Registered only on macOS.
pub struct ScreenRecordTool {
    /// Resolved recordings folder, or why it could not be resolved. Reported on `start`.
    recordings_dir: Result<PathBuf, String>,
    state: Arc<Mutex<RecordingState>>,
}

#[derive(Default)]
struct RecordingState {
    next_id: u64,
    active: HashMap<String, Recording>,
}

struct Recording {
    child: Child,
    path: PathBuf,
    started: Instant,
    /// Dropping this cancels the time limit, so a stopped recording is not interrupted again.
    cancel: Sender<()>,
}

impl ScreenRecordTool {
    pub fn new(recordings_dir: Result<PathBuf, String>) -> Self {
        Self {
            recordings_dir,
            state: Arc::new(Mutex::new(RecordingState::default())),
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for ScreenRecordTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "screen_record".into(),
            description: "Record one macOS app window to a .mov file, including while the \
                window is in the background. action=start takes app and an optional seconds \
                (default 60, max 600) and returns an id. action=stop takes that id and finishes \
                the file. Files are saved in the project's nca data folder unless the user set \
                [recording] dir. action=screenshot takes app and an optional delay (0 to 30 s, \
                default 3) and saves one still image of the app's window. To let the user see \
                any image, call show_image with its path. Needs Screen Recording permission for \
                the app that runs nca."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "stop", "screenshot"] },
                    "app": {
                        "type": "string",
                        "description": "start: name of a running app with a window, e.g. Dia"
                    },
                    "seconds": {
                        "type": "integer",
                        "description": "start: longest recording in seconds, 1 to 600 (default 60)"
                    },
                    "delay": {
                        "type": "integer",
                        "description": "screenshot: seconds to wait before capturing, 0 to 30 (default 3)"
                    },
                    "id": {
                        "type": "string",
                        "description": "stop: the id that start returned"
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn execute(&self, call: &ToolCall) -> ToolResult {
        let input = call.input.clone();
        let state = Arc::clone(&self.state);
        let recordings_dir = self.recordings_dir.clone();
        // Window lookup and process waits block, so keep them off the async runtime.
        let outcome = tokio::task::spawn_blocking(move || run(&state, &recordings_dir, &input))
            .await
            .unwrap_or_else(|error| Err(format!("screen_record stopped early: {error}")));
        match outcome {
            Ok(output) => ToolResult {
                call_id: call.id.clone(),
                success: true,
                output,
                error: None,
            },
            Err(error) => ToolResult {
                call_id: call.id.clone(),
                success: false,
                output: String::new(),
                error: Some(error),
            },
        }
    }
}

impl Drop for ScreenRecordTool {
    /// Ends any recording still running, so none keeps capturing the screen after nca exits.
    fn drop(&mut self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for (_, recording) in state.active.drain() {
            if let Err(error) = finish(recording) {
                tracing::warn!(%error, "screen recording did not finish cleanly");
            }
        }
    }
}

fn run(
    state: &Mutex<RecordingState>,
    recordings_dir: &Result<PathBuf, String>,
    input: &Value,
) -> Result<String, String> {
    match input["action"].as_str() {
        Some("start") => {
            let app = required_str(input, "app")?;
            let dir = recordings_dir.as_ref().map_err(Clone::clone)?;
            start(state, dir, app, seconds_arg(input)?)
        }
        Some("screenshot") => {
            let app = required_str(input, "app")?;
            let dir = recordings_dir.as_ref().map_err(Clone::clone)?;
            screenshot(state, dir, app, delay_arg(input)?)
        }
        Some("stop") => stop(state, required_str(input, "id")?),
        Some(other) => Err(format!(
            "Unknown action \"{other}\". Use start, stop, or screenshot."
        )),
        None => Err("`action` is required. Use start, stop, or screenshot.".into()),
    }
}

/// The `delay` argument of a screenshot: absent means the default; otherwise 0 to 30 seconds.
fn delay_arg(input: &Value) -> Result<u64, String> {
    match input.get("delay") {
        None | Some(Value::Null) => Ok(DEFAULT_SCREENSHOT_DELAY),
        Some(value) => value
            .as_u64()
            .filter(|delay| *delay <= MAX_SCREENSHOT_DELAY)
            .ok_or_else(|| {
                format!("`delay` must be a whole number from 0 to {MAX_SCREENSHOT_DELAY}.")
            }),
    }
}

/// The `seconds` argument: absent means the default; otherwise a whole number in range.
fn seconds_arg(input: &Value) -> Result<u64, String> {
    match input.get("seconds") {
        None | Some(Value::Null) => Ok(DEFAULT_SECONDS),
        Some(value) => value
            .as_u64()
            .filter(|seconds| (1..=MAX_SECONDS).contains(seconds))
            .ok_or_else(|| format!("`seconds` must be a whole number from 1 to {MAX_SECONDS}.")),
    }
}

fn start(
    state: &Mutex<RecordingState>,
    dir: &Path,
    app: &str,
    seconds: u64,
) -> Result<String, String> {
    require_screen_recording()?;
    let window = wait_for_window(app)?;
    let before = frontmost();

    fs::create_dir_all(dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let id = {
        let mut guard = lock(state)?;
        guard.next_id += 1;
        format!("rec-{}-{}", unix_millis(), guard.next_id)
    };
    let path = dir.join(format!("{id}-{}.mov", file_slug(app)));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    // screencapture reports failures on either stream, so both go to the log that `finish` reads.
    let log = File::create(path.with_extension("log"))
        .map_err(|error| format!("could not create the recording log: {error}"))?;
    let log_for_stderr = log
        .try_clone()
        .map_err(|error| format!("could not create the recording log: {error}"))?;
    let child = Command::new("screencapture")
        .args(["-x", "-v", "-l", &window.id.to_string()])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_for_stderr))
        .spawn()
        .map_err(|error| format!("could not start screencapture: {error}"))?;
    let cancel = spawn_time_limit(child.id(), seconds);
    let after = frontmost();

    lock(state)?.active.insert(
        id.clone(),
        Recording {
            child,
            path: path.clone(),
            started: Instant::now(),
            cancel,
        },
    );
    Ok(format!(
        "Recording {id} of {app} (window {}, {:.0}x{:.0} points) started. It stops by itself \
         after {seconds} s; finish it early with action=stop and id={id}. Saving to {}.\n{}",
        window.id,
        window.width,
        window.height,
        path.display(),
        focus_note(before.as_ref(), after.as_ref())
    ))
}

/// Saves one still image of the app's window after `delay` seconds. Like a recording, it
/// captures the window while the app stays in the background.
fn screenshot(
    state: &Mutex<RecordingState>,
    dir: &Path,
    app: &str,
    delay: u64,
) -> Result<String, String> {
    require_screen_recording()?;
    let window = wait_for_window(app)?;
    let before = frontmost();
    std::thread::sleep(Duration::from_secs(delay));

    fs::create_dir_all(dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let id = {
        let mut guard = lock(state)?;
        guard.next_id += 1;
        format!("shot-{}-{}", unix_millis(), guard.next_id)
    };
    let path = dir.join(format!("{id}-{}.png", file_slug(app)));
    let output = Command::new("screencapture")
        .args(["-x", "-o", "-l", &window.id.to_string()])
        .arg(&path)
        .output()
        .map_err(|error| format!("could not start screencapture: {error}"))?;
    let after = frontmost();
    if !output.status.success() || !path.exists() {
        return Err(format!(
            "screencapture could not capture {app} (window {}): {}",
            window.id,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(format!(
        "Saved a screenshot of {app} (window {}, {:.0}x{:.0} points) to {}. {}",
        window.id,
        window.width,
        window.height,
        path.display(),
        focus_note(before.as_ref(), after.as_ref())
    ))
}

fn stop(state: &Mutex<RecordingState>, id: &str) -> Result<String, String> {
    let mut recording = lock(state)?
        .active
        .remove(id)
        .ok_or_else(|| format!("No active recording \"{id}\"."))?;
    let (path, started) = (recording.path.clone(), recording.started);
    // The time limit may have ended it already; then the elapsed time is not
    // the recording's length, so do not report it as one.
    let already_ended = matches!(recording.child.try_wait(), Ok(Some(_)));
    finish(recording)?;
    let bytes = fs::metadata(&path)
        .map(|meta| meta.len())
        .map_err(|error| format!("recording {id} produced no file: {error}"))?;
    if already_ended {
        return Ok(format!(
            "{id} had already ended at its time limit. Saved {} ({} KB).",
            path.display(),
            bytes / 1024
        ));
    }
    Ok(format!(
        "Stopped {id} after {:.1} s. Saved {} ({} KB).",
        started.elapsed().as_secs_f64(),
        path.display(),
        bytes / 1024
    ))
}

/// Ends a recording: cancels its time limit, interrupts screencapture, and waits until the
/// file is written.
fn finish(recording: Recording) -> Result<(), String> {
    let Recording {
        mut child,
        path,
        started,
        cancel,
    } = recording;
    // Dropping the sender first means the time limit can no longer fire.
    drop(cancel);
    if let Some(wait) = MIN_RECORDING.checked_sub(started.elapsed()) {
        std::thread::sleep(wait);
    }
    let status = stop_child(&mut child)?;
    if status.success() {
        // A log is kept only when screencapture wrote something to it.
        let log = path.with_extension("log");
        if fs::metadata(&log).is_ok_and(|meta| meta.len() == 0) {
            let _ = fs::remove_file(&log);
        }
        return Ok(());
    }
    let log = fs::read_to_string(path.with_extension("log")).unwrap_or_default();
    Err(format!(
        "screencapture ended with {status} for {}: {}",
        path.display(),
        log.trim()
    ))
}

/// Interrupts screencapture until it exits. Repeats the interrupt when it has not exited in
/// time, and kills it after the last attempt, so a lost interrupt cannot hang `stop`.
fn stop_child(child: &mut Child) -> Result<ExitStatus, String> {
    for _ in 0..INTERRUPT_ATTEMPTS {
        interrupt(child.id());
        let deadline = Instant::now() + INTERRUPT_WAIT;
        while Instant::now() < deadline {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("could not check screencapture: {error}"))?
            {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = child.kill();
    child
        .wait()
        .map_err(|error| format!("could not wait for screencapture: {error}"))?;
    Err("screencapture did not stop after repeated interrupts, so it was killed".into())
}

/// Sends SIGINT. An open-ended screencapture finishes its file when it gets SIGINT.
fn interrupt(pid: u32) {
    let _ = Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status();
}

/// Interrupts `pid` after `seconds`, unless the returned sender is dropped first.
fn spawn_time_limit(pid: u32, seconds: u64) -> Sender<()> {
    let (cancel, cancelled) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        if let Err(RecvTimeoutError::Timeout) =
            cancelled.recv_timeout(Duration::from_secs(seconds).max(MIN_RECORDING))
        {
            interrupt(pid);
        }
    });
    cancel
}

fn lock(state: &Mutex<RecordingState>) -> Result<MutexGuard<'_, RecordingState>, String> {
    state
        .lock()
        .map_err(|_| "recording state is unavailable".to_string())
}

/// Milliseconds since the Unix epoch. Makes recording ids unique across sessions.
fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

/// The on-screen window of `app`, retried briefly because the window list can miss a
/// window for a moment.
fn wait_for_window(app: &str) -> Result<WindowRef, String> {
    for attempt in 0..WINDOW_WAIT_ATTEMPTS {
        if let Some(window) = main_window_of(app) {
            return Ok(window);
        }
        if attempt + 1 < WINDOW_WAIT_ATTEMPTS {
            std::thread::sleep(WINDOW_WAIT_STEP);
        }
    }
    Err(format!(
        "No on-screen window found for \"{app}\". Check that the app is running and that its \
         window is not minimized or on another desktop."
    ))
}

/// Screen Recording permission for this process. Checks without prompting.
fn require_screen_recording() -> Result<(), String> {
    // SAFETY: takes no arguments and only reads the permission state.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        Ok(())
    } else {
        Err("Screen Recording permission is missing for the app that runs nca. Grant it in System Settings > Privacy & Security > Screen & System Audio Recording, then retry.".into())
    }
}

/// A filename-safe form of an app name: ASCII letters and digits, with other runs turned
/// into single dashes. Path separators never survive, so the file stays in its folder.
fn file_slug(app: &str) -> String {
    let mut slug = String::new();
    for ch in app.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "window".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::app_input::{find_app, main_window_of};
    use nca_common::config::resolve_recordings_dir;
    use serde_json::json;
    use std::process::Command;

    #[test]
    fn seconds_default_and_stay_in_range() {
        assert_eq!(
            seconds_arg(&json!({"action": "start"})),
            Ok(DEFAULT_SECONDS)
        );
        assert_eq!(seconds_arg(&json!({"seconds": 5})), Ok(5));
        assert!(seconds_arg(&json!({"seconds": 0})).is_err());
        assert!(seconds_arg(&json!({"seconds": 601})).is_err());
        assert!(seconds_arg(&json!({"seconds": 2.5})).is_err());
    }

    #[test]
    fn delay_defaults_and_stays_in_range() {
        assert_eq!(
            delay_arg(&json!({"action": "screenshot"})),
            Ok(DEFAULT_SCREENSHOT_DELAY)
        );
        assert_eq!(delay_arg(&json!({"delay": 0})), Ok(0));
        assert_eq!(delay_arg(&json!({"delay": 30})), Ok(30));
        assert!(delay_arg(&json!({"delay": 31})).is_err());
    }

    #[test]
    fn screenshot_requires_an_app_before_capturing() {
        let state = Mutex::new(RecordingState::default());
        let dir = Ok(PathBuf::from("/nonexistent"));
        let error = run(&state, &dir, &json!({"action": "screenshot"})).unwrap_err();
        assert!(error.contains("`app`"));
    }

    #[test]
    fn file_slug_keeps_names_inside_the_recordings_folder() {
        assert_eq!(file_slug("Dia"), "dia");
        assert_eq!(file_slug("T3 Code (Beta)"), "t3-code-beta");
        assert_eq!(file_slug("../evil"), "evil");
        assert_eq!(file_slug("///"), "window");
    }

    #[test]
    fn run_rejects_bad_input_before_touching_the_system() {
        let state = Mutex::new(RecordingState::default());
        let dir = Ok(PathBuf::from("/nonexistent"));
        assert!(
            run(&state, &dir, &json!({}))
                .unwrap_err()
                .contains("`action`")
        );
        assert!(
            run(&state, &dir, &json!({"action": "pause"}))
                .unwrap_err()
                .contains("Unknown action")
        );
        assert!(
            run(&state, &dir, &json!({"action": "start"}))
                .unwrap_err()
                .contains("`app`")
        );
        assert!(
            run(&state, &dir, &json!({"action": "stop"}))
                .unwrap_err()
                .contains("`id`")
        );
        assert!(
            run(&state, &dir, &json!({"action": "stop", "id": "rec-9"}))
                .unwrap_err()
                .contains("No active recording")
        );
    }

    #[test]
    fn start_reports_an_unresolvable_folder_instead_of_recording() {
        let state = Mutex::new(RecordingState::default());
        let dir = Err("no data folder".to_string());
        let error = run(
            &state,
            &dir,
            &json!({"action": "start", "app": "Calculator"}),
        )
        .unwrap_err();
        assert_eq!(error, "no data folder");
    }

    #[tokio::test]
    async fn execute_turns_bad_input_into_a_failed_result() {
        let tool = ScreenRecordTool::new(Ok(PathBuf::from("/nonexistent")));
        let call = nca_common::tool::ToolCall {
            id: "call-1".into(),
            name: "screen_record".into(),
            input: json!({"action": "stop", "id": "rec-9"}),
        };
        let result = tool.execute(&call).await;
        assert_eq!(result.call_id, "call-1");
        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap_or_default()
                .contains("No active recording")
        );
    }

    /// Three recordings of Calculator's window while another app stays frontmost: one that
    /// the time limit ends with no stop call, one that `stop` ends early, and one that is
    /// still running when the tool is dropped. Opens Calculator with `open -g -n`, refuses to
    /// run if Calculator is already open, and quits only the instance it launched. Files go
    /// to the default recordings folder for this checkout, which must be outside the
    /// checkout. Set `NCA_HOME` to keep them out of your real data folder while testing.
    /// Needs Screen Recording and Accessibility permission for the terminal that runs it.
    #[test]
    #[ignore = "launches Calculator and records its window"]
    fn records_a_window_that_is_not_frontmost() {
        require_screen_recording()
            .expect("grant Screen Recording permission to the app that runs cargo test");
        assert!(
            find_app("Calculator").is_none(),
            "Calculator is already open; quit it before running this test"
        );
        let launched = Command::new("open")
            .args(["-g", "-n", "-a", "Calculator"])
            .status()
            .expect("run open");
        assert!(launched.success(), "open -g -n -a Calculator failed");

        let outcome = record_calculator();
        if let Some(calculator) = find_app("Calculator") {
            let _ = Command::new("kill")
                .args(["-TERM", &calculator.pid.to_string()])
                .status();
        }
        if let Err(error) = outcome {
            panic!("{error}");
        }
    }

    fn record_calculator() -> Result<(), String> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .ok_or("no project root")?;
        let dir = resolve_recordings_dir("", project).map_err(|error| error.to_string())?;
        if dir.starts_with(project) {
            return Err(format!(
                "recordings default into the project: {}",
                dir.display()
            ));
        }
        eprintln!("recordings folder: {}", dir.display());

        let front = frontmost().ok_or("frontmost app could not be read")?;
        for _ in 0..50 {
            if main_window_of("Calculator").is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let calculator = find_app("Calculator").ok_or("Calculator did not open")?;
        if front.pid == calculator.pid {
            return Err(
                "Calculator was frontmost before recording; the test would prove nothing".into(),
            );
        }

        let tool = ScreenRecordTool::new(Ok(dir.clone()));
        let state = Arc::clone(&tool.state);

        // 0. A stop right after start, as a model may issue it, still yields a playable file.
        eprintln!("{}", start(&state, &dir, "Calculator", 30)?);
        let immediate = only_path(&state)?;
        eprintln!("{}", stop(&state, &only_id(&state)?)?);
        require_movie(&immediate, 0.1, 3.0)?;

        // 1. The time limit finishes a recording that nobody stops.
        eprintln!("{}", start(&state, &dir, "Calculator", 2)?);
        std::thread::sleep(Duration::from_secs(4));
        require_movie(&only_path(&state)?, 1.0, 3.0)?;
        eprintln!("{}", stop(&state, &only_id(&state)?)?);

        // 2. stop finishes a recording early.
        eprintln!("{}", start(&state, &dir, "Calculator", 30)?);
        std::thread::sleep(Duration::from_secs(2));
        eprintln!("{}", stop(&state, &only_id(&state)?)?);

        // 3. Dropping the tool finishes a recording that is still running.
        eprintln!("{}", start(&state, &dir, "Calculator", 30)?);
        std::thread::sleep(Duration::from_secs(2));
        let running = only_path(&state)?;
        drop(tool);
        require_movie(&running, 1.0, 3.0)?;

        let after = frontmost().ok_or("frontmost app could not be read after recording")?;
        if after.pid != front.pid {
            return Err(format!(
                "frontmost app changed from {} to {}",
                front.name, after.name
            ));
        }
        Ok(())
    }

    /// The id of the only active recording.
    fn only_id(state: &Mutex<RecordingState>) -> Result<String, String> {
        lock(state)?
            .active
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| "no active recording".to_string())
    }

    /// The file path of the only active recording.
    fn only_path(state: &Mutex<RecordingState>) -> Result<PathBuf, String> {
        lock(state)?
            .active
            .values()
            .next()
            .map(|recording| recording.path.clone())
            .ok_or_else(|| "no active recording".to_string())
    }

    /// Checks that `path` is a finished movie whose length is within `min..=max` seconds.
    /// An unfinished movie has no readable duration, so ffprobe fails on it.
    fn require_movie(path: &Path, min: f64, max: f64) -> Result<(), String> {
        let bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        if bytes < 5_000 {
            return Err(format!("{} is only {bytes} bytes", path.display()));
        }
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=nw=1:nk=1",
            ])
            .arg(path)
            .output()
            .map_err(|error| format!("ffprobe is needed for this test: {error}"))?;
        let duration: f64 = String::from_utf8_lossy(&probe.stdout)
            .trim()
            .parse()
            .map_err(|_| format!("{} has no readable duration; not finished?", path.display()))?;
        if !(min..=max).contains(&duration) {
            return Err(format!(
                "{} is {duration:.2} s, expected {min} to {max} s",
                path.display()
            ));
        }
        eprintln!("{}: {duration:.2} s, {bytes} bytes", path.display());
        Ok(())
    }
}
