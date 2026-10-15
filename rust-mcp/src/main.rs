#![recursion_limit = "512"]
mod adapters;
mod cli;
mod dispatch;
mod help;
mod index;
mod mcp;
pub mod parsers;
mod textindex;
mod tools;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if cli::run(&args) {
        return;
    }
    mcp::run();
}

#[cfg(test)]
mod cli_tests {
    use serde_json::{json, Value};
    use std::io::{self, BufRead, IsTerminal, Write};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use crate::cli::args::*;
    use crate::cli::filter::*;
    use crate::cli::output::*;
    use crate::cli::suggest::*;
    use crate::dispatch::*;
    use crate::help::*;
    use crate::mcp::tool_list::*;
    use crate::tools;
    use crate::tools::workspace::{parse_file_uri, WorkspaceManager};


    fn exec_args(a: &[&str]) -> Value {
        parse_exec_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn exec_flags_before_the_command_after_one_quoted_command_and_after_the_separator() {
        // Before the command.
        let p = exec_args(&["--tail", "25", "--errors", "cargo", "build"]);
        assert_eq!((p["tail"].clone(), p["errors"].clone(), p["command"].clone()), (json!("25"), json!(true), json!("cargo build")));
        // After ONE quoted command string, mysid's flags still work.
        let p = exec_args(&["npm run build", "--timeout", "90", "--tail", "30", "--errors"]);
        assert_eq!((p["command"].clone(), p["timeout_secs"].clone(), p["tail"].clone(), p["errors"].clone()), (json!("npm run build"), json!(90), json!("30"), json!(true)));
        // `--` ends mysid's flags: what follows is the command, even `--tail`.
        let p = exec_args(&["--timeout", "5", "--", "docker", "logs", "--tail", "50", "web"]);
        assert_eq!((p["timeout_secs"].clone(), p["command"].clone(), p.get("tail").cloned()), (json!(5), json!("docker logs --tail 50 web"), None));
        // Several unquoted words: a `--tail` among them belongs to the command (docker, tail, git ...).
        let p = exec_args(&["docker", "logs", "--tail", "50", "web"]);
        assert_eq!((p["command"].clone(), p.get("tail").cloned()), (json!("docker logs --tail 50 web"), None));
        // The toolchain environment.
        let (verb, p) = parse_cli_args(&["msvc".to_string(), "cl".to_string(), "/nologo".to_string(), "a.cpp".to_string()]);
        assert_eq!((verb.as_str(), p["env"].clone(), p["command"].clone()), ("exec", json!("msvc"), json!("cl /nologo a.cpp")));
        let (_, p) = parse_cli_args(&["wasm".to_string(), "emcc a.cpp".to_string()]);
        assert_eq!(p["env"], json!("emsdk"));
    }

    #[test]
    fn errors_keeps_only_the_lines_that_say_something_went_wrong() {
        let mut params = json!({ "errors": true });
        let f = OutputFilter::take(&mut params).unwrap();
        let out = f.apply("compiling a.c\nb.c(3): error C2065: 'x': undeclared identifier\nLINK : fatal error LNK1181: cannot open input file\nall fine\n[exit 2]");
        assert!(out.contains("error C2065") && out.contains("LNK1181") && out.contains("[exit 2]") && !out.contains("all fine") && !out.contains("compiling"), "{}", out);
        assert!(out.contains("--errors"), "says what it filtered: {}", out);
    }

    #[test]
    fn typos_get_suggestions_and_nonsense_gets_none() {
        assert_eq!(suggest_commands("bild"), vec!["build"]);
        assert_eq!(suggest_commands("serach"), vec!["search"]);
        assert!(suggest_commands("rel").contains(&"release"), "prefix of a command");
        assert!(suggest_commands("--grpah").contains(&"graph"), "leading dashes are ignored");
        assert!(suggest_commands("zzzzzz").is_empty());
        assert!(suggest_commands("").is_empty());
    }

    #[test]
    fn every_known_command_is_actually_dispatched() {
        let mgr = Arc::new(Mutex::new(WorkspaceManager::new()));
        for cmd in KNOWN_COMMANDS {
            let out = dispatch_method(cmd, &json!({ "help": true }), None, &mgr);
            assert!(!out.starts_with("ERROR: No such command"), "'{}' is listed but not handled", cmd);
        }
        assert!(dispatch_method("definitely-not-a-command", &json!({}), None, &mgr).starts_with("ERROR: No such command"));
    }

    #[test]
    fn exit_codes_reflect_success_failure_and_unknown_commands() {
        assert_eq!(cli_exit_code("ERROR: File not found: x", false), 1);
        assert_eq!(cli_exit_code("{\"success\": false, \"error\": \"boom\"}", false), 1);
        assert_eq!(cli_exit_code("{\"success\": true, \"output\": \"fine\"}", false), 0);
        assert_eq!(cli_exit_code("No matches found.", false), 0);
        assert_eq!(cli_exit_code("ERROR: No such command: 'x'.", true), 2);
    }
}
