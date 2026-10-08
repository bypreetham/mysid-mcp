use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

pub const MYSID_DIR: &str = ".mysid";
pub const BUILD_LOG: &str = "build.log";

/// Creates `<workspace>/.mysid/` (self-ignored via its own `.gitignore`) and returns it.
/// Best effort: callers treat a failure as "no log", never as a tool error.
pub fn ensure_dir(workspace_root: &Path) -> io::Result<PathBuf> {
    let dir = workspace_root.join(MYSID_DIR);
    fs::create_dir_all(&dir)?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        fs::write(&ignore, "*\n")?;
    }
    Ok(dir)
}

pub struct LoggedRun {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `cmd`, appending every output line to `log_path` as it arrives (so the file can be
/// tailed live), while also capturing stdout/stderr for the caller's summary.
/// The log is truncated at the start of each run.
pub fn run_logged(mut cmd: Command, log_path: &Path, title: &str) -> io::Result<LoggedRun> {
    let mut file = File::create(log_path)?;
    writeln!(file, "# {}", title)?;
    file.flush()?;
    let log = Arc::new(Mutex::new(file));

    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let out = child.stdout.take().expect("piped stdout");
    let err = child.stderr.take().expect("piped stderr");

    let t_out = pump(out, Arc::clone(&log));
    let t_err = pump(err, Arc::clone(&log));

    let status = child.wait()?;
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();

    if let Ok(mut f) = log.lock() {
        let _ = writeln!(f, "# exit code: {}", status.code().unwrap_or(-1));
        let _ = f.flush();
    }
    Ok(LoggedRun { status, stdout, stderr })
}

fn pump<R: io::Read + Send + 'static>(reader: R, log: Arc<Mutex<File>>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut captured = String::new();
        let mut reader = BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    if let Ok(mut f) = log.lock() {
                        let _ = f.write_all(line.as_bytes());
                        let _ = f.flush();
                    }
                    captured.push_str(&line);
                }
            }
        }
        captured
    })
}
