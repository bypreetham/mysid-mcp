//! `mysid release`: raises the versionCode and builds the release bundle (Android), or deploys a web project.

use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

use super::web;
use super::worker::{self, Job, Kind};
use super::{build_response, java_home_env};
use crate::tools::workspace::resolve_existing_root;

/// `mysid release`: raises versionCode by one and builds the release AAB (`bundleRelease`) on the
/// detached worker, so it reports progress like `mysid build`. The bump happens inside the worker,
/// right before Gradle starts, and is put back if the build fails or is cancelled. A release that
/// is already running is attached to, not bumped again.
pub fn execute_release(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let root = Path::new(&workspace_root);
    let android = root.join("gradlew.bat").exists() || root.join("gradlew").exists();
    if android {
        if let Some(answer) = crate::tools::stale::guard(root, params) {
            return answer;
        }
    }
    if !android && web::is_web_project(root) {
        return web_release(&workspace_root, root, params);
    }
    if !android {
        return json!({
            "success": false,
            "error": format!("No gradlew or package.json found in {}: `mysid release` builds Android projects and deploys web projects", root.display())
        })
        .to_string();
    }

    #[cfg(target_os = "windows")]
    let gradlew = if root.join("gradlew.bat").exists() {
        root.join("gradlew.bat").to_string_lossy().to_string()
    } else {
        "gradlew.bat".to_string()
    };
    #[cfg(not(target_os = "windows"))]
    let gradlew = if root.join("gradlew").exists() {
        "./gradlew".to_string()
    } else {
        "gradlew".to_string()
    };

    let job = Job {
        stack: "Android release (Gradle)".to_string(),
        kind: Kind::GradleRelease,
        program: gradlew,
        args: vec!["bundleRelease".to_string(), "-x".to_string(), "test".to_string(), "--console=plain".to_string()],
        cwd: root.to_string_lossy().to_string(),
        env: java_home_env(root),
        bump: true,
    };

    let wait = Duration::from_secs(
        params.get("wait").and_then(|v| v.as_u64()).unwrap_or(worker::default_wait_secs()),
    );
    let attached = match worker::start(&workspace_root, &job) {
        Ok(a) => a,
        Err(e) => return json!({ "success": false, "error": e }).to_string(),
    };
    if !attached {
        eprintln!("RELEASE STARTED - {} - in {} - versionCode +1 - log: .mysid/build.log", job.stack, job.cwd);
    }
    match worker::wait(&workspace_root, wait, !attached) {
        Some(st) => build_response(&st, attached),
        None => json!({ "success": false, "error": "build status unavailable" }).to_string(),
    }
}

/// `mysid release` in a web project: build and `netlify deploy --prod` in the detached worker.
fn web_release(workspace_root: &str, root: &Path, params: &Value) -> String {
    let job = Job {
        stack: web::STACK.to_string(),
        kind: Kind::WebRelease,
        program: "netlify".to_string(),
        args: Vec::new(),
        cwd: root.to_string_lossy().to_string(),
        env: Vec::new(),
        bump: false,
    };
    let wait = Duration::from_secs(
        params.get("wait").and_then(|v| v.as_u64()).unwrap_or(worker::default_wait_secs()),
    );
    let attached = match worker::start(workspace_root, &job) {
        Ok(a) => a,
        Err(e) => return json!({ "success": false, "error": e }).to_string(),
    };
    if !attached {
        eprintln!("RELEASE STARTED - {} - in {} - log: .mysid/release.log", job.stack, job.cwd);
    }
    match worker::wait(workspace_root, wait, !attached) {
        Some(st) => build_response(&st, attached),
        None => json!({ "success": false, "error": "release status unavailable" }).to_string(),
    }
}
