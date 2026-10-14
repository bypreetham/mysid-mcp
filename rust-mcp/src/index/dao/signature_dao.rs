use rusqlite::{params, Connection};

use super::{like_escape, Result};
use crate::index::model::FileId;
use crate::parsers::{MethodSignature, TypeInfo};

/// The outline of one indexed file: exactly what its parser reported (every type, every method,
/// overloads included), so `overview` can answer without parsing.
pub struct Outline {
    pub path: String,
    pub language: String,
    pub lines: usize,
    pub types: Vec<TypeInfo>,
    pub methods: Vec<MethodSignature>,
}

pub struct SignatureDao<'a> {
    conn: &'a Connection,
}

impl<'a> SignatureDao<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Stores the parser output of a file, in source order (types first, then methods).
    pub fn insert_file(&self, file_id: FileId, types: &[TypeInfo], methods: &[MethodSignature]) -> Result<()> {
        let mut put = self.conn.prepare_cached(
            "INSERT INTO signatures (file_id, ord, row_kind, name, kind, signature, params, return_type, start_line, end_line, parent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        let mut ord = 0i64;
        for t in types {
            put.execute(params![file_id, ord, "type", t.name, t.kind, "", "", Option::<String>::None, t.line as i64, t.line as i64, Option::<String>::None])?;
            ord += 1;
        }
        for m in methods {
            put.execute(params![
                file_id,
                ord,
                "method",
                m.name,
                m.kind,
                m.signature,
                m.params,
                m.return_type,
                m.start_line as i64,
                m.end_line as i64,
                m.parent
            ])?;
            ord += 1;
        }
        Ok(())
    }

    /// Method name -> the one type every method of that name returns (`getDecoder` -> `IDecoder`); names whose methods
    /// disagree, or return nothing with a class type, are left out. Used to type the receiver of a chained call.
    pub fn return_types(&self) -> Result<std::collections::HashMap<String, String>> {
        use std::collections::HashMap;
        let mut stmt = self.conn.prepare("SELECT name, return_type FROM signatures WHERE row_kind = 'method' AND return_type IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map: HashMap<String, Option<String>> = HashMap::new();
        for row in rows {
            let (name, ty) = row?;
            let simple = crate::index::resolve::simplify_type(&ty);
            match map.get(&name) {
                None => {
                    map.insert(name, simple);
                }
                Some(prev) if *prev == simple => {}
                _ => {
                    map.insert(name, None);
                }
            }
        }
        Ok(map.into_iter().filter_map(|(k, v)| v.map(|t| (k, t))).collect())
    }

    /// Outlines of the file `target`, or of every indexed file under the directory `target`, by path.
    pub fn outlines(&self, target: &str) -> Result<Vec<Outline>> {
        let dir_pattern = format!("{}/%", like_escape(target));
        let mut files = self.conn.prepare_cached(
            "SELECT id, path, language, lines FROM files WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' ORDER BY path",
        )?;
        let found: Vec<(FileId, String, String, i64)> = files
            .query_map(params![target, dir_pattern], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_>>()?;

        let mut rows = self.conn.prepare_cached(
            "SELECT row_kind, name, kind, signature, params, return_type, start_line, end_line, parent
             FROM signatures WHERE file_id = ?1 ORDER BY ord",
        )?;
        let mut out = Vec::with_capacity(found.len());
        for (id, path, language, lines) in found {
            let mut types = Vec::new();
            let mut methods = Vec::new();
            let mut cursor = rows.query(params![id])?;
            while let Some(r) = cursor.next()? {
                let row_kind: String = r.get(0)?;
                if row_kind == "type" {
                    types.push(TypeInfo { name: r.get(1)?, kind: r.get(2)?, line: r.get::<_, i64>(6)? as usize });
                } else {
                    methods.push(MethodSignature {
                        name: r.get(1)?,
                        kind: r.get(2)?,
                        signature: r.get(3)?,
                        params: r.get(4)?,
                        return_type: r.get(5)?,
                        start_line: r.get::<_, i64>(6)? as usize,
                        end_line: r.get::<_, i64>(7)? as usize,
                        parent: r.get(8)?,
                    });
                }
            }
            out.push(Outline { path, language, lines: lines as usize, types, methods });
        }
        Ok(out)
    }
}
