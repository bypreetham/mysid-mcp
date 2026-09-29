use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

/// Ensures the `agent-data` folder exists in the project root and returns its path.
pub fn get_agent_data_dir(workspace_root: &Path) -> PathBuf {
    let dir = workspace_root.join("agent-data");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

/// Reads a persistent HashMap from `<workspace>/agent-data/<filename>`.
/// If the file or folder doesn't exist, returns an empty HashMap.
pub fn read_agent_data_map<T: DeserializeOwned>(
    workspace_root: &Path,
    filename: &str,
) -> HashMap<String, T> {
    let path = workspace_root.join("agent-data").join(filename);
    if !path.exists() {
        return HashMap::new();
    }
    match File::open(&path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            serde_json::from_reader(reader).unwrap_or_default()
        }
        Err(_) => HashMap::new(),
    }
}

/// Writes a HashMap to `<workspace>/agent-data/<filename>` using formatted JSON.
/// Automatically creates `agent-data` directory if missing.
pub fn write_agent_data_map<T: Serialize>(
    workspace_root: &Path,
    filename: &str,
    map: &HashMap<String, T>,
) -> io::Result<()> {
    let dir = get_agent_data_dir(workspace_root);
    let path = dir.join(filename);
    let file = File::create(&path)?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, map)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    Ok(())
}
