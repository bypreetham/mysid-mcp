use std::path::Path;
use std::process::Command;

pub const MAX_OUTPUT_BYTES: usize = 32_768;

pub fn resolve_adb_binary() -> String {
    if let Ok(android_home) = std::env::var("ANDROID_HOME") {
        let p = Path::new(&android_home)
            .join("platform-tools")
            .join(if cfg!(windows) { "adb.exe" } else { "adb" });
        if p.exists() {
            return p.to_string_lossy().to_string();
        }
    }
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let p = Path::new(&local_app_data)
            .join("Android")
            .join("Sdk")
            .join("platform-tools")
            .join("adb.exe");
        if p.exists() {
            return p.to_string_lossy().to_string();
        }
    }
    "adb".to_string()
}

pub fn run_adb_cmd(args: &[&str], cwd: Option<&Path>) -> Result<(i32, String, String), String> {
    let adb_bin = resolve_adb_binary();
    let mut cmd = Command::new(&adb_bin);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    match cmd.output() {
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1);
            let mut stdout_str = String::from_utf8_lossy(&out.stdout).to_string();
            let mut stderr_str = String::from_utf8_lossy(&out.stderr).to_string();

            if stdout_str.len() > MAX_OUTPUT_BYTES {
                let start = stdout_str.len() - MAX_OUTPUT_BYTES;
                stdout_str = format!("[... truncated ...]\n{}", &stdout_str[start..]);
            }
            if stderr_str.len() > MAX_OUTPUT_BYTES {
                let start = stderr_str.len() - MAX_OUTPUT_BYTES;
                stderr_str = format!("[... truncated ...]\n{}", &stderr_str[start..]);
            }

            Ok((code, stdout_str, stderr_str))
        }
        Err(e) => Err(format!("Failed to execute '{} {}': {}", adb_bin, args.join(" "), e)),
    }
}
