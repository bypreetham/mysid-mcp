//! Text index for `search`: which files could possibly contain a pattern. Filled lazily.
//!
//! Each searchable text file gets a small bloom filter of its (lower-cased) 3-byte sequences, stored in
//! `.mysid/text.db`. Nothing is indexed up front: a search plans its work from what is already known.
//!
//! - a file with a fresh filter (same mtime and size) is read only if the filter says the pattern may be in it;
//! - a file with no filter yet, or a stale one, is read and searched, and its filter is saved from that same read.
//!
//! So the first search of an area costs a normal scan, later ones read only likely files, and the index grows
//! with use. A filter can say "maybe" wrongly, never "no" wrongly, and every candidate is verified by the real
//! regex, so results always equal a full scan. Each row records the mtime/size seen *before* the file was read:
//! if the file changes meanwhile, the row simply stops matching and the file is read again next time.
//!
//! Like the code index it is a cache; any problem means "no index" and `search` scans everything.

use ignore::{WalkBuilder, WalkState};
use regex_syntax::hir::literal::{ExtractKind, Extractor};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use crate::adapters::detect_adapter;
use crate::tools::build::log;

const DB_FILE: &str = "text.db";
const SCHEMA_VERSION: i32 = 1;
/// Files with these extensions are never searched (kept in step with `search`).
pub const BINARY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "ico", "webp", "svg", "mp3", "wav", "ogg", "flac", "mp4", "avi",
    "mov", "mkv", "zip", "tar", "gz", "7z", "rar", "jar", "aar", "apk", "aab", "dex", "so", "dylib",
    "dll", "exe", "bin", "class", "pyc", "pdf", "db", "sqlite", "sqlite3",
];
/// Larger files (bundles, generated sources) are not searched without being named.
pub const MAX_FILE_BYTES: u64 = 1_500_000;
/// Bits of filter per distinct trigram: ~8% false "maybe" per trigram, so a 5-letter word passes ~0.05% of files.
const BITS_PER_GRAM: usize = 6;
const MIN_BITS: usize = 256;

/// A file `search` should read.
pub struct Item {
    pub rel: String,
    pub mtime: i64,
    pub size: i64,
    /// No usable filter is stored yet: save one from this read.
    pub learn: bool,
}

/// What a read taught us about one file.
pub struct Learned {
    pub rel: String,
    pub mtime: i64,
    pub size: i64,
    /// Empty for a binary file (remembered so it is not read again).
    pub bloom: Vec<u8>,
}

pub struct Plan {
    pub items: Vec<Item>,
    root: PathBuf,
    conn: Mutex<Connection>,
    gone: Vec<String>,
}

/// The files to read for `pattern`, or None when indexing is off or broken (then scan everything).
/// A pattern without a 3+ character literal cannot use the filters, so every file is listed.
pub fn plan(root: &Path, pattern: &str) -> Option<Plan> {
    if std::env::var_os("MYSID_NOINDEX").is_some() {
        return None;
    }
    let conn = open(root).ok()?;
    let on_disk = walk(root);
    let wanted: Option<Vec<Vec<u32>>> = required_literals(pattern).map(|ls| ls.iter().map(|l| grams(l)).collect());

    // path -> (mtime, size, filter allows the pattern)
    let mut stored: HashMap<String, (i64, i64, bool)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT path, mtime, size, bloom FROM docs").ok()?;
        let mut rows = stmt.query([]).ok()?;
        while let Ok(Some(row)) = rows.next() {
            let (Ok(path), Ok(mtime), Ok(size)) = (row.get::<_, String>(0), row.get::<_, i64>(1), row.get::<_, i64>(2)) else { continue };
            let Ok(rusqlite::types::ValueRef::Blob(bloom)) = row.get_ref(3) else { continue };
            let allows = !bloom.is_empty()
                && wanted.as_ref().map_or(true, |w| w.iter().any(|gs| gs.iter().all(|g| contains(bloom, *g))));
            stored.insert(path, (mtime, size, allows));
        }
    }

    let mut items = Vec::new();
    let mut live = std::collections::HashSet::new();
    for f in on_disk {
        live.insert(f.rel.clone());
        match stored.get(&f.rel) {
            Some(&(m, s, allows)) if m == f.mtime && s == f.size => {
                if allows {
                    items.push(Item { rel: f.rel, mtime: f.mtime, size: f.size, learn: false });
                }
            }
            _ => items.push(Item { rel: f.rel, mtime: f.mtime, size: f.size, learn: true }),
        }
    }
    let gone = stored.into_keys().filter(|p| !live.contains(p)).collect();
    Some(Plan { items, root: root.to_path_buf(), conn: Mutex::new(conn), gone })
}

impl Plan {
    /// Stores what reads taught us. Safe to call from several threads and repeatedly; a file that changed or
    /// vanished since it was read is skipped so a stale filter is never saved.
    pub fn save(&self, learned: &mut Vec<Learned>) {
        if learned.is_empty() {
            return;
        }
        let Ok(conn) = self.conn.lock() else { return };
        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            learned.clear();
            return;
        }
        let ok = (|| -> rusqlite::Result<()> {
            let mut put = conn.prepare_cached("INSERT OR REPLACE INTO docs (path, mtime, size, bloom) VALUES (?1, ?2, ?3, ?4)")?;
            for l in learned.iter() {
                if stat_of(&self.root.join(&l.rel)) == Some((l.mtime, l.size)) {
                    put.execute(params![l.rel, l.mtime, l.size, l.bloom])?;
                }
            }
            Ok(())
        })();
        let _ = conn.execute_batch(if ok.is_ok() { "COMMIT" } else { "ROLLBACK" });
        learned.clear();
    }

    /// Drops the rows of files that no longer exist.
    pub fn finish(&self) {
        if self.gone.is_empty() {
            return;
        }
        let Ok(conn) = self.conn.lock() else { return };
        if conn.execute_batch("BEGIN IMMEDIATE").is_ok() {
            for p in &self.gone {
                let _ = conn.execute("DELETE FROM docs WHERE path = ?1", params![p]);
            }
            let _ = conn.execute_batch("COMMIT");
        }
    }
}

fn stat_of(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    Some((mtime, meta.len() as i64))
}

/// Literals every match of `pattern` must begin with (lower-cased), at least 3 ASCII bytes each; None if the
/// regex has no such guarantee.
fn required_literals(pattern: &str) -> Option<Vec<Vec<u8>>> {
    let hir = regex_syntax::ParserBuilder::new().multi_line(true).build().parse(pattern).ok()?;
    let seq = Extractor::new().kind(ExtractKind::Prefix).extract(&hir);
    let lits = seq.literals()?;
    if lits.is_empty() {
        return None;
    }
    let mut out: Vec<Vec<u8>> = Vec::new();
    for l in lits {
        let bytes = l.as_bytes();
        if bytes.len() < 3 || !bytes.is_ascii() {
            return None;
        }
        let lower = bytes.to_ascii_lowercase();
        if grams(&lower).is_empty() {
            return None;
        }
        if !out.contains(&lower) {
            out.push(lower);
        }
    }
    Some(out)
}

/// Trigram keys of `bytes` that can be stored (line breaks and NULs never are).
fn grams(bytes: &[u8]) -> Vec<u32> {
    bytes
        .windows(3)
        .filter(|w| !w.iter().any(|b| matches!(b, b'\n' | b'\r' | 0)))
        .map(|w| (w[0] as u32) << 16 | (w[1] as u32) << 8 | w[2] as u32)
        .collect()
}

fn positions(gram: u32, bits: usize) -> [usize; 2] {
    let g = gram as u64;
    let h1 = g.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 20;
    let h2 = (g ^ (g >> 7)).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) >> 24 | 1;
    let mask = (bits - 1) as u64;
    [(h1 & mask) as usize, (h1.wrapping_add(h2) & mask) as usize]
}

fn contains(bloom: &[u8], gram: u32) -> bool {
    let bits = bloom.len() * 8;
    if bits == 0 || !bits.is_power_of_two() {
        return true; // unreadable filter: let the scan decide
    }
    positions(gram, bits).iter().all(|&p| bloom[p / 8] & (1 << (p % 8)) != 0)
}

/// The filter for a file's text, or None for a binary file.
pub fn build_bloom(buf: &[u8]) -> Option<Vec<u8>> {
    if buf[..buf.len().min(8192)].contains(&0) {
        return None;
    }
    let lower = buf.to_ascii_lowercase();
    let mut keys: Vec<u32> = grams(&lower);
    keys.sort_unstable();
    keys.dedup();
    let bits = (keys.len() * BITS_PER_GRAM).next_power_of_two().max(MIN_BITS);
    let mut bloom = vec![0u8; bits / 8];
    for k in keys {
        for p in positions(k, bits) {
            bloom[p / 8] |= 1 << (p % 8);
        }
    }
    Some(bloom)
}

fn open(root: &Path) -> rusqlite::Result<Connection> {
    let dir = log::ensure_dir(root).map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?;
    let conn = Connection::open(dir.join(DB_FILE))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS docs;
             CREATE TABLE docs (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, mtime INTEGER NOT NULL, size INTEGER NOT NULL, bloom BLOB NOT NULL);",
        )?;
        conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION))?;
    }
    Ok(conn)
}

struct Seen {
    rel: String,
    mtime: i64,
    size: i64,
}

/// Files `search` could look at: same ignore and skip rules as the search walk. Metadata only, no reads.
fn walk(root: &Path) -> Vec<Seen> {
    let adapter = detect_adapter(root);
    let base = root.to_path_buf();
    let found: Mutex<Vec<Seen>> = Mutex::new(Vec::new());
    WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(move |e| {
            if e.depth() == 0 {
                return true;
            }
            let rel = e.path().strip_prefix(&base).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            e.file_name().to_str().map_or(true, |n| !adapter.is_skipped_entry(&rel, n))
        })
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                let Ok(e) = entry else { return WalkState::Continue };
                if !e.file_type().map_or(false, |t| t.is_file()) {
                    return WalkState::Continue;
                }
                let ext = e.path().extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                if BINARY_EXTS.contains(&ext.as_str()) {
                    return WalkState::Continue;
                }
                let Ok(meta) = e.metadata() else { return WalkState::Continue };
                if meta.len() > MAX_FILE_BYTES {
                    return WalkState::Continue;
                }
                let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
                if let Ok(rel) = e.path().strip_prefix(root) {
                    found.lock().unwrap().push(Seen { rel: rel.to_string_lossy().replace('\\', "/"), mtime, size: meta.len() as i64 });
                }
                WalkState::Continue
            })
        });
    found.into_inner().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_extracted_from_literals_and_alternations() {
        let l = |p: &str| required_literals(p).map(|v| v.into_iter().map(|b| String::from_utf8(b).unwrap()).collect::<Vec<_>>());
        assert_eq!(l(r"\bSong\b"), Some(vec!["song".to_string()]));
        let alt = l("buffer_length|buffer.resize|ctx->buffer|mIsIso =|isIso").unwrap();
        for want in ["buffer", "ctx->buffer", "misiso =", "isiso"] {
            assert!(alt.iter().any(|a| a == want), "{:?} lacks {}", alt, want);
        }
        assert_eq!(l("^class"), Some(vec!["class".to_string()]));
        assert_eq!(l(r"\w+Service"), None, "no required prefix: full scan");
        assert_eq!(l("ab"), None, "too short for a trigram");
        assert_eq!(l("caf\u{e9}"), None, "non-ASCII is left to the scan");
    }

    #[test]
    fn filter_never_misses_a_file_that_contains_the_text() {
        let text = b"fun playTrack() { audioBuffer.fill(UsbBulkEngine) }\n";
        let bloom = build_bloom(text).unwrap();
        for lit in ["playtrack", "audiobuffer", "usbbulkengine", "fill(usb"] {
            assert!(grams(lit.as_bytes()).iter().all(|g| contains(&bloom, *g)), "{}", lit);
        }
        assert!(build_bloom(b"abc\0def").is_none(), "binary files get no filter");
    }

    /// Reads what the plan lists, like `search` does, and saves what it learns.
    fn run(dir: &Path, pattern: &str) -> Vec<String> {
        let plan = plan(dir, pattern).unwrap();
        let mut learned = Vec::new();
        let mut read = Vec::new();
        for it in &plan.items {
            read.push(it.rel.clone());
            if it.learn {
                let buf = std::fs::read(dir.join(&it.rel)).unwrap();
                learned.push(Learned { rel: it.rel.clone(), mtime: it.mtime, size: it.size, bloom: build_bloom(&buf).unwrap_or_default() });
            }
        }
        plan.save(&mut learned);
        plan.finish();
        read.sort();
        read
    }

    #[test]
    fn the_index_fills_as_searches_run_and_follows_edits_and_deletes() {
        let dir = std::env::temp_dir().join(format!("mysid_textidx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.kt"), "val needleAlpha = 1\n").unwrap();
        std::fs::write(dir.join("b.kt"), "val other = 2\n").unwrap();

        assert_eq!(run(&dir, "needleAlpha"), vec!["a.kt", "b.kt"], "first search reads everything it has not seen");
        assert_eq!(run(&dir, "needleAlpha"), vec!["a.kt"], "then only files whose filter allows the pattern");

        std::fs::write(dir.join("b.kt"), "val needleAlpha = 22222\n").unwrap(); // edited: stale row
        std::fs::write(dir.join("c.kt"), "needlealpha again\n").unwrap(); // new: unknown
        std::fs::remove_file(dir.join("a.kt")).unwrap();
        assert_eq!(run(&dir, "needleAlpha"), vec!["b.kt", "c.kt"]);
        assert_eq!(run(&dir, "needleAlpha"), vec!["b.kt", "c.kt"]);
        assert_eq!(run(&dir, "zzzUnique"), Vec::<String>::new(), "filters now rule everything out");
        let rows: i64 = open(&dir).unwrap().query_row("SELECT count(*) FROM docs", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2, "the deleted file's row is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
