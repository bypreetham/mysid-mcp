pub mod c_family;
pub mod csharp;
pub mod go;
pub mod java;
pub mod kotlin;
pub mod markdown;
pub mod python;
pub mod rust_lang;
pub mod swift;
pub mod web;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub kind: String, // "class", "interface", "struct", "enum", "trait", "object", "impl", "h1", "h2", etc.
    pub line: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MethodSignature {
    pub name: String,
    pub kind: String, // "method", "constructor", "function", "heading", etc.
    pub signature: String,
    pub params: String,
    pub return_type: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub parent: Option<String>,
}

pub trait LanguageParser: Send + Sync {
    fn language_name(&self) -> &'static str;
    fn extensions(&self) -> &[&'static str];
    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>);
}

/// Helper function to gather signatures spanning multiple lines across language parsers
pub fn gather_multiline_signature(
    lines: &[&str],
    start_idx: usize,
    stop_char: char,
) -> (String, String, usize) {
    let mut accumulated = String::new();
    let mut end_idx = start_idx;
    let mut paren_depth = 0;
    let mut found_open_paren = false;

    let max_idx = (start_idx + 25).min(lines.len());

    for idx in start_idx..max_idx {
        let line = lines[idx].trim();
        if accumulated.is_empty() {
            accumulated.push_str(line);
        } else {
            accumulated.push(' ');
            accumulated.push_str(line);
        }
        end_idx = idx;

        for ch in line.chars() {
            if ch == '(' {
                paren_depth += 1;
                found_open_paren = true;
            } else if ch == ')' {
                paren_depth -= 1;
            }
        }

        if found_open_paren && paren_depth <= 0 {
            // Check if stop char is reached on this line or trailing type is completed
            if line.contains(stop_char)
                || line.contains('{')
                || line.contains(';')
                || paren_depth == 0
            {
                break;
            }
        }
    }

    // Extract parameters inside first outermost ()
    let params = if let Some(open_pos) = accumulated.find('(') {
        if let Some(close_pos) = accumulated.rfind(')') {
            if close_pos > open_pos {
                accumulated[open_pos + 1..close_pos]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Clean up signature up to opening brace { or semicolon
    let clean_sig = if let Some(brace_pos) = accumulated.find('{') {
        accumulated[..brace_pos].trim().to_string()
    } else if let Some(semi_pos) = accumulated.find(';') {
        accumulated[..semi_pos].trim().to_string()
    } else {
        accumulated.trim().to_string()
    };

    (clean_sig, params, end_idx + 1)
}

static PARSERS: &[&dyn LanguageParser] = &[
    &kotlin::KotlinParser,
    &java::JavaParser,
    &c_family::CppParser,
    &rust_lang::RustParser,
    &python::PythonParser,
    &web::WebParser,
    &go::GoParser,
    &swift::SwiftParser,
    &csharp::CSharpParser,
    &markdown::MarkdownParser,
];

pub fn find_parser(ext: &str) -> Option<&'static dyn LanguageParser> {
    let lower = ext.to_ascii_lowercase();
    PARSERS
        .iter()
        .copied()
        .find(|p| p.extensions().contains(&lower.as_str()))
}

pub fn is_supported_extension(ext: &str) -> bool {
    find_parser(ext).is_some()
}
