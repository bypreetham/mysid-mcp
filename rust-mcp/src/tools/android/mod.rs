pub mod adb;
pub mod device;
pub mod input;
pub mod logs;
pub mod runner;

use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Deserialize, Debug, Clone)]
pub struct AndroidParams {
    pub action: String,
    #[serde(default)]
    pub workspace_root: Option<String>,
    #[serde(default)]
    pub package_name: Option<String>,
    #[serde(default)]
    pub ip_address: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub lines: Option<u32>,
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub keycode: Option<String>,
    #[serde(default)]
    pub activity_name: Option<String>,
}

pub fn execute_android(params: &Value, default_root: Option<&str>) -> String {
    let p: AndroidParams = match serde_json::from_value(params.clone()) {
        Ok(parsed) => parsed,
        Err(e) => {
            return json!({
                "success": false,
                "error": format!("Invalid android params: {}", e)
            })
            .to_string();
        }
    };

    // Resolve workspace: explicit param wins, then session default.
    let resolved_workspace: Option<String> = p.workspace_root.clone().or_else(|| default_root.map(|s| s.to_string()));
    let workspace_path = resolved_workspace.as_deref().map(Path::new);

    match p.action.as_str() {
        "" | "help" | "-h" | "--help" => {
            "mysid android: Android device & ADB utilities\n\n\
Usage:\n  \
  mysid android devices                       List connected ADB devices\n  \
  mysid android connect_wifi [--ip <ip>]      Pair/connect device over Wi-Fi\n  \
  mysid android install                       Deploy built debug APK to device\n  \
  mysid android launch_app                    Launch main application activity\n  \
  mysid android logcat [--lines <n>]          Dump recent logcat entries\n  \
  mysid android crashes                       Filter logcat for fatal exceptions & crashes\n  \
  mysid android clear_logcat                  Flush ADB logcat buffer\n  \
  mysid android tap --x <x> --y <y>           Simulate touch screen tap\n  \
  mysid android type --text <text>            Simulate text keyboard input\n  \
  mysid android keyevent --keycode <code|key> Send keycode (e.g. KEYCODE_BACK)\n".to_string()
        }
        // Device management
        "devices" => device::handle_devices(workspace_path),
        "disconnect" => device::handle_disconnect(workspace_path),
        "restart_server" => device::handle_restart_server(workspace_path),
        "connect_wifi" | "wifi" | "connect" => device::handle_connect_wifi(&p, workspace_path),

        // Execution and App Lifecycle
        "install" => runner::handle_install(&p, workspace_path),
        "run" => runner::handle_run_process(workspace_path),
        "launch_app" => runner::handle_launch_app(&p, workspace_path),

        // Logs & Diagnostics
        "logcat" | "crashes" => logs::handle_logcat_or_crashes(&p, workspace_path),
        "clear_logcat" => logs::handle_clear_logcat(workspace_path),

        // Input simulation
        "tap" => input::handle_tap(&p, workspace_path),
        "type" => input::handle_type(&p, workspace_path),
        "keyevent" => input::handle_keyevent(&p, workspace_path),

        _ => json!({
            "success": false,
            "error": format!(
                "Unknown android action '{}'. Run 'mysid android --help' for usage.",
                p.action
            )
        }).to_string(),
    }
}
