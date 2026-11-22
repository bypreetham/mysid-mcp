//! D-pad navigation for screens without touch (Android TV): which element has focus, and `goto`, which presses the
//! D-pad until a named element has focus. Reads the accessibility tree (`uiautomator dump`), so a key press is answered
//! with one line of text instead of a screenshot.

use super::touch::*;
use regex::Regex;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Longest a key press waits for the screen to stop changing.
pub(super) const SETTLE_CAP_MS: u64 = 2000;

#[derive(Clone, Debug)]
pub(super) struct TNode {
    text: String,
    desc: String,
    id: String,
    class: String,
    b: (i64, i64, i64, i64),
    focused: bool,
    focusable: bool,
    clickable: bool,
    visible: bool,
    parent: Option<usize>,
    /// One past the last descendant (nodes are in document order).
    end: usize,
}

impl TNode {
    fn cx(&self) -> i64 {
        (self.b.0 + self.b.2) / 2
    }
    fn cy(&self) -> i64 {
        (self.b.1 + self.b.3) / 2
    }
    fn area(&self) -> i64 {
        (self.b.2 - self.b.0).max(0) * (self.b.3 - self.b.1).max(0)
    }
    fn short_id(&self) -> &str {
        self.id.rsplit('/').next().unwrap_or("")
    }
    fn short_class(&self) -> &str {
        self.class.rsplit('.').next().unwrap_or("")
    }
    fn own(&self) -> String {
        if !self.text.is_empty() { self.text.clone() } else { self.desc.clone() }
    }
}

/// The whole accessibility tree with parents, in document order.
pub(super) fn parse_tree(xml: &str) -> Vec<TNode> {
    let tag_re = Regex::new(r#"<node\s((?:[^>"]|"[^"]*")*?)(/?)>|</node>"#).unwrap();
    let attr_re = Regex::new(r#"([\w:-]+)="([^"]*)""#).unwrap();
    let bounds_re = Regex::new(r"\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]").unwrap();
    let mut nodes: Vec<TNode> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for c in tag_re.captures_iter(xml) {
        match c.get(1) {
            None => {
                if let Some(i) = stack.pop() {
                    nodes[i].end = nodes.len();
                }
            }
            Some(attrs) => {
                let map: HashMap<String, String> = attr_re.captures_iter(attrs.as_str()).map(|a| (a[1].to_string(), unescape(&a[2]))).collect();
                let get = |k: &str| map.get(k).cloned().unwrap_or_default();
                let is = |k: &str| map.get(k).map_or(false, |v| v == "true");
                let b = bounds_re
                    .captures(&get("bounds"))
                    .map(|b| (b[1].parse().unwrap_or(0), b[2].parse().unwrap_or(0), b[3].parse().unwrap_or(0), b[4].parse().unwrap_or(0)))
                    .unwrap_or((0, 0, 0, 0));
                let idx = nodes.len();
                nodes.push(TNode {
                    text: get("text"),
                    desc: get("content-desc"),
                    id: get("resource-id"),
                    class: get("class"),
                    b,
                    focused: is("focused"),
                    focusable: is("focusable"),
                    clickable: is("clickable"),
                    visible: !(b.2 <= b.0 || b.3 <= b.1) && map.get("visible-to-user").map_or(true, |v| v != "false"),
                    parent: stack.last().copied(),
                    end: idx + 1,
                });
                if &c[2] != "/" {
                    stack.push(idx);
                }
            }
        }
    }
    for i in stack {
        nodes[i].end = nodes.len();
    }
    nodes
}

/// The last accessibility read. A read costs 2-3 s on a TV, so the line printed right after a settle check reuses the read the
/// check just made; any input (`invalidate`) throws it away.
static TREE_CACHE: Mutex<Option<(Instant, String, Vec<TNode>)>> = Mutex::new(None);
const CACHE_MS: u128 = 700;

pub(super) fn invalidate() {
    if let Ok(mut c) = TREE_CACHE.lock() {
        *c = None;
    }
}

/// A read of the screen right now (never the cached one); it becomes the cached one.
fn fresh_tree(ctx: &Ctx) -> Result<Vec<TNode>, String> {
    let nodes = dump_xml(ctx).map(|x| parse_tree(&x))?;
    if let Ok(mut c) = TREE_CACHE.lock() {
        *c = Some((Instant::now(), ctx.serial.clone(), nodes.clone()));
    }
    Ok(nodes)
}

fn tree(ctx: &Ctx) -> Result<Vec<TNode>, String> {
    if let Ok(c) = TREE_CACHE.lock() {
        if let Some((at, serial, nodes)) = c.as_ref() {
            if *serial == ctx.serial && at.elapsed().as_millis() < CACHE_MS {
                return Ok(nodes.clone());
            }
        }
    }
    fresh_tree(ctx)
}

/// Texts of the descendants (a focusable row is usually labelled by its children).
fn subtree_texts(nodes: &[TNode], i: usize) -> Vec<String> {
    (i + 1..nodes[i].end).filter(|&j| nodes[j].visible).filter_map(|j| Some(nodes[j].own()).filter(|s| !s.is_empty())).take(16).collect()
}

/// 2: the node's text, description or all its descendants' text equals `q`; 1: contains it; 0: no.
fn match_level(nodes: &[TNode], i: usize, q: &str) -> u8 {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return 0;
    }
    let n = &nodes[i];
    let own = [n.text.to_lowercase(), n.desc.to_lowercase()];
    let joined = subtree_texts(nodes, i).join(" ").to_lowercase();
    if own.iter().any(|s| *s == q) || (!joined.is_empty() && joined == q) {
        2
    } else if own.iter().any(|s| s.contains(&q)) || joined.contains(&q) {
        1
    } else {
        0
    }
}

/// How well the focused element (or anything inside it) matches `q`: a row is named by its children.
fn focus_level(nodes: &[TNode], i: usize, q: &str) -> u8 {
    (i..nodes[i].end).filter(|&j| nodes[j].visible).map(|j| match_level(nodes, j, q)).max().unwrap_or(0)
}

fn clip(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > n { format!("{}...", s.chars().take(n).collect::<String>()) } else { s }
}

/// What an element is called: its own text, else ALL the text inside it (`2 | ab_short | 3:12` for a playlist row), so the
/// index, the title and the duration all name the row.
fn label(nodes: &[TNode], i: usize) -> String {
    let own = nodes[i].own();
    if !own.is_empty() {
        return clip(&own, 40);
    }
    let kids = subtree_texts(nodes, i);
    if !kids.is_empty() {
        let mut parts: Vec<String> = Vec::new();
        for k in kids {
            if !parts.contains(&k) {
                parts.push(k);
            }
        }
        return clip(&parts.iter().take(4).map(|s| clip(s, 28)).collect::<Vec<_>>().join(" | "), 80);
    }
    if !nodes[i].short_id().is_empty() {
        return nodes[i].short_id().to_string();
    }
    nodes[i].short_class().to_string()
}

/// The element that has input focus: the deepest visible one flagged `focused`.
fn focused_index(nodes: &[TNode]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (i, n) in nodes.iter().enumerate() {
        if n.focused && n.visible {
            let mut depth = 0;
            let mut cur = n.parent;
            while let Some(p) = cur {
                depth += 1;
                cur = nodes[p].parent;
            }
            if best.map_or(true, |(_, d)| depth >= d) {
                best = Some((i, depth));
            }
        }
    }
    best.map(|(i, _)| i)
}

/// Labelled ancestors, outermost first: `Home > Playlists > Row 2`.
fn path(nodes: &[TNode], i: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = nodes[i].parent;
    while let Some(p) = cur {
        let n = &nodes[p];
        let part = if !n.own().is_empty() { clip(&n.own(), 24) } else { n.short_id().to_string() };
        if !part.is_empty() {
            parts.push(part);
        }
        cur = n.parent;
    }
    parts.reverse();
    let tail: Vec<String> = parts.iter().rev().take(5).rev().cloned().collect();
    if tail.is_empty() { "-".to_string() } else { tail.join(" > ") }
}

fn describe(nodes: &[TNode], i: usize) -> String {
    let n = &nodes[i];
    format!(
        "\"{}\" [{}] center ({},{}) bounds [{},{}][{},{}]{} path: {}",
        label(nodes, i),
        n.short_class(),
        n.cx(),
        n.cy(),
        n.b.0,
        n.b.1,
        n.b.2,
        n.b.3,
        if n.short_id().is_empty() { String::new() } else { format!(" id={}", n.short_id()) },
        path(nodes, i)
    )
}

fn focus_line_of(nodes: &[TNode]) -> String {
    match focused_index(nodes) {
        Some(i) => format!("focus: {}", describe(nodes, i)),
        None => "focus: none - no element has input focus (the first D-pad press usually gives it to one)".to_string(),
    }
}

/// The focus line for the screen right now.
pub(super) fn focus_line(ctx: &Ctx) -> String {
    match tree(ctx) {
        Ok(nodes) => focus_line_of(&nodes),
        Err(e) => format!("focus: unreadable - {}", e),
    }
}

/// Whether the focused element (or something inside it) matches `q`, and what has focus now (for `expect focus`).
pub(super) fn focus_is(ctx: &Ctx, q: &str) -> Result<(bool, String), String> {
    let nodes = tree(ctx)?;
    Ok(match focused_index(&nodes) {
        Some(i) => (focus_level(&nodes, i, q) > 0, format!("\"{}\"", label(&nodes, i))),
        None => (false, "nothing".to_string()),
    })
}

/// `mysid android focus [--list]`
pub(super) fn report(ctx: &Ctx, p: &Value) -> Result<String, String> {
    let nodes = tree(ctx)?;
    if !(flag(p, "list") || flag(p, "all")) {
        return Ok(focus_line_of(&nodes));
    }
    let focused = focused_index(&nodes);
    let mut out = vec![focus_line_of(&nodes), String::new(), "focusable elements on screen (* = has focus): [index] label | class | center(x,y)".to_string()];
    for (i, n) in nodes.iter().enumerate().filter(|(_, n)| n.focusable && n.visible).take(80) {
        out.push(format!("{} [{}] {} | {} | ({},{})", if Some(i) == focused { "*" } else { " " }, i, label(&nodes, i), n.short_class(), n.cx(), n.cy()));
    }
    if out.len() == 3 {
        out.push("(none: this screen exposes no focusable elements)".to_string());
    }
    Ok(out.join("\n"))
}

/// (fingerprint, has focus): which element has focus and where it is. Animations, progress bars and clocks do not change it,
/// so it settles even when the pixels never stop moving.
fn ui_signature(ctx: &Ctx) -> Option<(u64, bool)> {
    let nodes = fresh_tree(ctx).ok()?;
    if nodes.is_empty() {
        return None;
    }
    let focus = focused_index(&nodes).map(|i| (label(&nodes, i), nodes[i].b));
    let has = focus.is_some();
    let mut h = DefaultHasher::new();
    focus.hash(&mut h);
    Some((h.finish(), has))
}

/// Waits until the focus has settled, or `cap_ms` passes. Returns (ms waited, settled).
/// A D-pad move lands at once, so after a short pause ONE read that shows an element with focus is enough (a read costs
/// 2-3 s on a TV). `strict` (OK, back, home: the page changes) wants two reads in a row to agree. Falls back to comparing
/// screen captures when the accessibility tree cannot be read.
pub(super) fn wait_settled(ctx: &Ctx, cap_ms: u64, strict: bool) -> (u64, bool) {
    let t0 = Instant::now();
    thread::sleep(Duration::from_millis(300));
    let Some((mut last, has)) = ui_signature(ctx) else { return wait_settled_pixels(ctx, cap_ms) };
    if !strict && has {
        return (t0.elapsed().as_millis() as u64, true);
    }
    loop {
        if t0.elapsed() >= Duration::from_millis(cap_ms) {
            return (t0.elapsed().as_millis() as u64, false);
        }
        thread::sleep(Duration::from_millis(100));
        if let Some((s, h)) = ui_signature(ctx) {
            if (!strict && h) || s == last {
                return (t0.elapsed().as_millis() as u64, true);
            }
            last = s;
        }
    }
}

/// Waits until two screen captures in a row are equal (or `cap_ms` passes). Returns (ms waited, settled).
pub(super) fn wait_settled_pixels(ctx: &Ctx, cap_ms: u64) -> (u64, bool) {
    let t0 = Instant::now();
    thread::sleep(Duration::from_millis(150));
    let mut last = screen_hash(ctx);
    let mut equal = 0;
    loop {
        let h = screen_hash(ctx);
        if h.is_some() && h == last {
            equal += 1;
            if equal >= 2 {
                return (t0.elapsed().as_millis() as u64, true);
            }
        } else {
            equal = 0;
            last = h;
        }
        if t0.elapsed() >= Duration::from_millis(cap_ms) {
            return (t0.elapsed().as_millis() as u64, false);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// The focusable element a query names: the exact match with the smallest area, else a partial match.
/// A label on a non-focusable child resolves to its nearest focusable ancestor (or descendant).
fn candidates(nodes: &[TNode], q: &str, focus: Option<usize>) -> Vec<(usize, u8)> {
    let mut cands: Vec<(usize, u8)> = Vec::new();
    for i in 0..nodes.len() {
        if !nodes[i].visible {
            continue;
        }
        let level = match_level(nodes, i, q);
        if level == 0 {
            continue;
        }
        let mut t = i;
        if !(nodes[i].focusable || nodes[i].clickable) {
            let mut cur = nodes[i].parent;
            let mut up = None;
            while let Some(p) = cur {
                if nodes[p].focusable || nodes[p].clickable {
                    up = Some(p);
                    break;
                }
                cur = nodes[p].parent;
            }
            t = up
                .or_else(|| (i + 1..nodes[i].end).find(|&j| nodes[j].visible && (nodes[j].focusable || nodes[j].clickable)))
                .unwrap_or(i);
        }
        if !cands.iter().any(|(c, _)| *c == t) {
            cands.push((t, level));
        } else if let Some(c) = cands.iter_mut().find(|(c, _)| *c == t) {
            c.1 = c.1.max(level);
        }
    }
    let Some(best) = cands.iter().map(|c| c.1).max() else { return Vec::new() };
    cands.retain(|c| c.1 == best);
    cands.sort_by_key(|(i, _)| {
        let n = &nodes[*i];
        let near = focus.map_or(0, |f| (nodes[f].cx() - n.cx()).abs() + (nodes[f].cy() - n.cy()).abs());
        (n.area(), near)
    });
    cands
}

fn pick_target(nodes: &[TNode], q: &str, focus: Option<usize>) -> Option<(usize, u8)> {
    candidates(nodes, q, focus).first().copied()
}

/// The target, and how many elements match. `nth` (1-based, screen order: top to bottom, left to right) picks one of several.
fn choose(nodes: &[TNode], q: &str, focus: Option<usize>, nth: Option<usize>) -> (Option<(usize, u8)>, usize) {
    let mut c = candidates(nodes, q, focus);
    let count = c.len();
    if let Some(n) = nth {
        c.sort_by_key(|(i, _)| (nodes[*i].cy(), nodes[*i].cx()));
        return (c.get(n.saturating_sub(1)).copied(), count);
    }
    (c.first().copied(), count)
}

/// `a` is `b`, or one contains the other (the focused cell and the focusable row around it).
fn related(nodes: &[TNode], a: usize, b: usize) -> bool {
    a == b || (a < b && b < nodes[a].end) || (b < a && a < nodes[b].end)
}

/// How much of the shorter of two spans lies inside the other (0 = apart, 1 = one contains the other).
fn overlap_ratio(a0: i64, a1: i64, b0: i64, b1: i64) -> f64 {
    let shared = (a1.min(b1) - a0.max(b0)).max(0) as f64;
    shared / ((a1 - a0).min(b1 - b0).max(1) as f64)
}

/// Which D-pad key moves focus from `f` towards `t`. `alt` flips to the other axis (used after a press that did nothing).
fn dir_to(f: (i64, i64, i64, i64), t: (i64, i64, i64, i64), alt: bool) -> &'static str {
    dir_to_with(f, t, alt, None)
}

/// `dir_to`, where `diagonal` says which way to go when the target shares neither a row nor a column with `f`:
/// `Some(true)` sideways first (focus sits in a column of items), `Some(false)` up/down first (focus sits in a row of items),
/// `None` the longer distance. A span counts as shared only when at least 30% of it overlaps: a row that touches the next
/// one by 3 pixels is not the same row.
fn dir_to_with(f: (i64, i64, i64, i64), t: (i64, i64, i64, i64), alt: bool, diagonal: Option<bool>) -> &'static str {
    let (fx, fy) = ((f.0 + f.2) / 2, (f.1 + f.3) / 2);
    let (tx, ty) = ((t.0 + t.2) / 2, (t.1 + t.3) / 2);
    let (dx, dy) = (tx - fx, ty - fy);
    let y_overlap = overlap_ratio(f.1, f.3, t.1, t.3) >= 0.3;
    let x_overlap = overlap_ratio(f.0, f.2, t.0, t.2) >= 0.3;
    let horizontal = if y_overlap && !x_overlap {
        true
    } else if x_overlap && !y_overlap {
        false
    } else if !y_overlap && !x_overlap {
        diagonal.unwrap_or(dx.abs() > dy.abs())
    } else {
        dx.abs() > dy.abs()
    };
    match horizontal ^ alt {
        true if dx >= 0 => "RIGHT",
        true => "LEFT",
        false if dy >= 0 => "DOWN",
        false => "UP",
    }
}

/// `n` presses of one D-pad key in ONE adb call (a call costs about 1.7 s on a TV).
fn press_n(ctx: &Ctx, key: &str, n: usize) -> Result<(), String> {
    let code = format!("KEYCODE_DPAD_{}", key);
    let mut args: Vec<&str> = vec!["keyevent"];
    args.extend(std::iter::repeat(code.as_str()).take(n.max(1)));
    shell_input(ctx, &args)
}

fn static_key(k: &str) -> &'static str {
    match k {
        "UP" => "UP",
        "DOWN" => "DOWN",
        "LEFT" => "LEFT",
        _ => "RIGHT",
    }
}

fn opposite(k: &str) -> &'static str {
    match k {
        "UP" => "DOWN",
        "DOWN" => "UP",
        "LEFT" => "RIGHT",
        _ => "LEFT",
    }
}

type Bounds = (i64, i64, i64, i64);

fn center(b: Bounds) -> (i64, i64) {
    ((b.0 + b.2) / 2, (b.1 + b.3) / 2)
}

/// The nearest ancestor of `f` that groups several focusable elements (a tab row, a list, a grid): focus moves along it
/// with the keys that run along it, and leaves it with the key that points out of it.
fn container_of(nodes: &[TNode], f: usize, screen: (i64, i64)) -> Option<usize> {
    let mut cur = nodes[f].parent;
    while let Some(p) = cur {
        let n = &nodes[p];
        let count = (p + 1..n.end).filter(|&j| nodes[j].visible && nodes[j].focusable).count();
        if count >= 2 && n.area() < screen.0 * screen.1 * 9 / 10 {
            return Some(p);
        }
        cur = n.parent;
    }
    None
}

/// The keys to try from focus `f` to target `t`, best first. When the target lies OUTSIDE the container that holds focus
/// (a page element below the tab row), the first key is the one that leaves the container (DOWN), not the sideways key
/// towards the target's x position: moving along the row first only walks away from the way out.
/// The rest follow: sideways towards the target, sideways away, and last the key back.
fn key_order(f: Bounds, t: Bounds, container: Option<Bounds>, mates: (usize, usize)) -> Vec<&'static str> {
    let (tx, ty) = center(t);
    let (fx, fy) = center(f);
    let outside = container.filter(|c| !(tx >= c.0 && tx < c.2 && ty >= c.1 && ty < c.3));
    // No container to leave (a flat Compose tree): go the way that has items to move through. Items further along the row
    // towards the target and none in the column: sideways; the other way round: up/down; otherwise the longer distance.
    let diagonal = match mates {
        (row_ahead, 0) if row_ahead > 0 => Some(true),
        (0, col_ahead) if col_ahead > 0 => Some(false),
        _ => None,
    };
    let primary = match outside {
        // Leaving a row (wider than tall) is up or down, leaving a column is left or right, whatever the target's x or y is:
        // a tab row is left by DOWN even when the target lies below and further left than the row reaches.
        Some(c) => {
            let (w, h) = (c.2 - c.0, c.3 - c.1);
            let (cx, cy) = center(c);
            if w * 2 > h * 3 {
                if ty < cy { "UP" } else { "DOWN" }
            } else if h * 2 > w * 3 {
                if tx < cx { "LEFT" } else { "RIGHT" }
            } else {
                dir_to(c, t, false)
            }
        }
        None => dir_to_with(f, t, false, diagonal),
    };
    let side = if matches!(primary, "UP" | "DOWN") {
        if tx < fx { "LEFT" } else { "RIGHT" }
    } else if ty < fy {
        "UP"
    } else {
        "DOWN"
    };
    vec![primary, side, opposite(side), opposite(primary)]
}

/// (items further along f's row towards `t`, items further along f's column towards `t`): the other focusable elements
/// that share at least half of f's vertical (horizontal) span and lie on the target's side of it. Whether a sideways or an
/// up/down press has anything to move through, when the tree has no group around the items.
fn mates(nodes: &[TNode], f: usize, t: usize) -> (usize, usize) {
    let (fb, (fx, fy), (tx, ty)) = (nodes[f].b, center(nodes[f].b), center(nodes[t].b));
    let (mut row_ahead, mut col_ahead) = (0, 0);
    for (j, n) in nodes.iter().enumerate() {
        if j == f || !n.visible || !n.focusable || related(nodes, f, j) {
            continue;
        }
        let (cx, cy) = center(n.b);
        if overlap_ratio(fb.1, fb.3, n.b.1, n.b.3) >= 0.5 && (cx - fx).signum() == (tx - fx).signum() {
            row_ahead += 1;
        }
        if overlap_ratio(fb.0, fb.2, n.b.0, n.b.2) >= 0.5 && (cy - fy).signum() == (ty - fy).signum() {
            col_ahead += 1;
        }
    }
    (row_ahead, col_ahead)
}

/// Presses needed when focus `f` and target `t` are siblings in one row (or column) and the key runs along it: one per
/// focusable sibling between them. They go in one adb call; the next read checks where focus landed.
fn steps_along(nodes: &[TNode], f: usize, t: usize, key: &str) -> usize {
    let Some(parent) = nodes[f].parent else { return 1 };
    if nodes[t].parent != Some(parent) {
        return 1;
    }
    let horizontal = matches!(key, "LEFT" | "RIGHT");
    let (fc, tc) = (center(nodes[f].b), center(nodes[t].b));
    let aligned = |b: Bounds| if horizontal { nodes[f].b.1 < b.3 && b.1 < nodes[f].b.3 } else { nodes[f].b.0 < b.2 && b.0 < nodes[f].b.2 };
    if !aligned(nodes[t].b) {
        return 1;
    }
    let along = |c: (i64, i64)| match key {
        "RIGHT" => c.0,
        "LEFT" => -c.0,
        "DOWN" => c.1,
        _ => -c.1,
    };
    let (from, to) = (along(fc), along(tc));
    if to <= from {
        return 1;
    }
    let n = (parent + 1..nodes[parent].end)
        .filter(|&j| nodes[j].parent == Some(parent) && nodes[j].visible && nodes[j].focusable && aligned(nodes[j].b))
        .filter(|&j| {
            let a = along(center(nodes[j].b));
            a > from && a <= to
        })
        .count();
    n.clamp(1, 30)
}

fn visible_focusables(nodes: &[TNode]) -> String {
    let labels: Vec<String> = (0..nodes.len()).filter(|&i| nodes[i].visible && nodes[i].focusable).take(10).map(|i| format!("\"{}\"", label(nodes, i))).collect();
    if labels.is_empty() { "no focusable element is visible".to_string() } else { format!("focusable on screen: {}", labels.join(", ")) }
}

/// One target: press the D-pad until it has focus.
/// Returns the summary line and, when no key was pressed after the last read, the focus line of that read.
///
/// Direction: the key that leaves the container holding focus comes first when the target is outside it, else the key
/// towards the target. A press that moves nothing is repeated once (a lazy list may not have composed more rows yet),
/// then that key is marked blocked at that spot and the next key in `key_order` is tried; only when all four keys do
/// nothing does it stop, and the message lists them (that is a real focus trap, or the app is not receiving keys).
fn goto_one(ctx: &Ctx, p: &Value, q: &str, max: usize, select: bool) -> Result<(String, Option<String>), String> {
    let scroll_key = text(p, &["scroll", "dir"]).map(|s| s.to_uppercase()).filter(|s| matches!(s.as_str(), "UP" | "DOWN" | "LEFT" | "RIGHT")).unwrap_or_else(|| "DOWN".to_string());
    let scroll_key = static_key(&scroll_key);
    let mut pressed: Vec<String> = Vec::new();
    let mut last: Option<(String, Option<(i64, i64)>)> = None;
    let nth = num(p, &["nth", "row"]).map(|n| n.max(1.0) as usize);
    let mut seen: HashMap<(i64, i64), u32> = HashMap::new();
    let mut blocked: HashMap<(i64, i64), Vec<&'static str>> = HashMap::new();
    let mut retried: HashSet<((i64, i64), &'static str)> = HashSet::new();
    let trail = |pressed: &Vec<String>| if pressed.is_empty() { "no presses".to_string() } else { format!("{} press(es): {}", pressed.len(), pressed.join(" ")) };
    loop {
        let nodes = tree(ctx).map_err(|e| format!("goto \"{}\": cannot read the screen after {}: {}", q, trail(&pressed), e))?;
        let fi = focused_index(&nodes);
        let fpos = fi.map(|i| (nodes[i].cx(), nodes[i].cy()));
        let now = fi.map_or("nothing".to_string(), |i| format!("\"{}\"", label(&nodes, i)));
        let mut retry_key: Option<&'static str> = None;
        let mut stalled = false;

        if let Some((key, before)) = last.take() {
            match (fpos, before) {
                (None, _) => return Err(format!("goto \"{}\": focus disappeared after pressing {} ({}). The screen may have changed.", q, key, trail(&pressed))),
                (Some(pos), Some(b)) if pos == b => {
                    stalled = true;
                    let k = static_key(&key);
                    if retried.insert((pos, k)) {
                        retry_key = Some(k);
                    } else {
                        blocked.entry(pos).or_default().push(k);
                    }
                }
                (Some(pos), Some(b)) => {
                    let (dx, dy) = (pos.0 - b.0, pos.1 - b.1);
                    let wrong = match key.as_str() {
                        "RIGHT" => dx < -20,
                        "LEFT" => dx > 20,
                        "DOWN" => dy < -20,
                        "UP" => dy > 20,
                        _ => false,
                    };
                    if wrong {
                        return Err(format!(
                            "goto \"{}\": focus jumped the wrong way: pressed {}, focus went from ({},{}) to ({},{}) {} ({}). {}",
                            q, key, b.0, b.1, pos.0, pos.1, now, trail(&pressed), visible_focusables(&nodes)
                        ));
                    }
                }
                (Some(_), None) => {}
            }
        }
        // Visiting the same spot again and again (after moves) means focus is going in circles; a press that moved nothing is not a visit.
        if !stalled {
            if let Some(pos) = fpos {
                let n = seen.entry(pos).or_insert(0);
                *n += 1;
                if *n >= 4 {
                    return Err(format!("goto \"{}\": focus is going in circles around {} ({}). {}", q, now, trail(&pressed), visible_focusables(&nodes)));
                }
            }
        }

        let (target, count) = choose(&nodes, q, fi, nth);
        if nth.is_some() && target.is_none() {
            return Err(format!("goto \"{}\": --nth {} but {} element(s) match on this screen. {}", q, nth.unwrap_or(0), count, visible_focusables(&nodes)));
        }
        if let Some(i) = fi {
            let have = focus_level(&nodes, i, q);
            let arrived = match (nth, target) {
                // With --nth only the chosen element counts, not another row that also matches.
                (Some(_), Some((t, _))) => related(&nodes, t, i),
                (_, t) => t.map_or(true, |(t, level)| t == i || have >= level),
            };
            if have > 0 && arrived {
                let mut line = format!("goto \"{}\": focused after {} on {}{}", q, trail(&pressed), now, if count > 1 { format!(" ({}{} of {} matches{})", if nth.is_some() { "#" } else { "" }, nth.unwrap_or(1), count, if nth.is_none() { "; --nth n picks another" } else { "" }) } else { String::new() });
                if !select {
                    return Ok((line, Some(focus_line_of(&nodes))));
                }
                {
                    shell_input(ctx, &["keyevent", &keycode(&text(p, &["ok_key", "ok-key"]).unwrap_or_else(|| "DPAD_CENTER".to_string()))])?;
                    let (ms, settled) = wait_settled(ctx, SETTLE_CAP_MS, true);
                    line.push_str(&format!(", OK pressed (screen {} in {}ms)", if settled { "settled" } else { "still changing" }, ms));
                }
                return Ok((line, None));
            }
        }
        if pressed.len() >= max {
            return Err(format!(
                "goto \"{}\": not focused after {} presses ({}). Focus is on {}. {}",
                q,
                max,
                pressed.join(" "),
                now,
                if target.is_some() { format!("The target is on screen at ({},{}); something blocks the way.", nodes[target.unwrap().0].cx(), nodes[target.unwrap().0].cy()) } else { format!("\"{}\" never appeared; {}", q, visible_focusables(&nodes)) }
            ));
        }

        let order: Vec<&'static str> = match (fi, target) {
            (_, None) => vec![scroll_key],
            (Some(f), Some((t, _))) => {
                let container = container_of(&nodes, f, (ctx.dev_w, ctx.dev_h)).map(|c| nodes[c].b);
                key_order(nodes[f].b, nodes[t].b, container, mates(&nodes, f, t))
            }
            (None, Some((t, _))) => vec![dir_to((0, 0, ctx.dev_w, ctx.dev_h), nodes[t].b, false)],
        };
        let none: Vec<&'static str> = Vec::new();
        let blocked_here = fpos.and_then(|pos| blocked.get(&pos)).unwrap_or(&none);
        let Some(key) = retry_key.or_else(|| order.iter().copied().find(|k| !blocked_here.contains(k))) else {
            return Err(match target {
                None => format!("goto \"{}\": it is not on this screen, and focus cannot move further {} ({}). It stays on {}. {}", q, scroll_key, trail(&pressed), now, visible_focusables(&nodes)),
                Some((t, _)) => format!(
                    "goto \"{}\": focus will not leave {} - {} each did nothing ({}). The target is at ({},{}). Either this page traps focus (a real focus bug) or the app is not receiving keys. {}",
                    q,
                    now,
                    blocked_here.join(", "),
                    trail(&pressed),
                    nodes[t].cx(),
                    nodes[t].cy(),
                    visible_focusables(&nodes)
                ),
            });
        };
        // Siblings in one row: all the presses in one call; the next read shows where focus landed.
        let n = match (retry_key, fi, target) {
            (None, Some(f), Some((t, _))) => steps_along(&nodes, f, t, key).min(max.saturating_sub(pressed.len()).max(1)),
            _ => 1,
        };
        if retry_key.is_some() {
            thread::sleep(Duration::from_millis(700));
        }
        press_n(ctx, key, n)?;
        pressed.extend(std::iter::repeat(key.to_string()).take(n));
        last = Some((key.to_string(), fpos));
        // The next accessibility read is the check; a screen capture per press would only slow the walk.
        thread::sleep(Duration::from_millis(if n > 1 { 150 + 60 * n.min(10) as u64 } else { 250 }));
    }
}

/// `mysid android goto "USB Drive" "Durandhar"`: each target gets focus (D-pad), then OK; text only.
pub(super) fn goto(ctx: &Ctx, p: &Value) -> Result<String, String> {
    let mut targets: Vec<String> = match p.get("targets") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };
    if targets.is_empty() {
        targets = args_of(p);
    }
    if targets.is_empty() {
        targets.extend(text(p, &["text"]));
    }
    if targets.is_empty() {
        return Err("goto needs the label of what to focus: mysid android goto \"USB Drive\" \"Durandhar\" (each gets focus, then OK; --no-ok only focuses)".to_string());
    }
    guard(ctx, p)?;
    let max = num(p, &["max"]).map_or(30, |m| (m as usize).clamp(1, 100));
    let select = !(flag(p, "no_ok") || flag(p, "no-ok"));
    let mut out: Vec<String> = Vec::new();
    let mut fresh: Option<String> = None;
    for q in &targets {
        match goto_one(ctx, p, q, max, select) {
            Ok((line, f)) => {
                out.push(line);
                fresh = f;
            }
            Err(e) => {
                out.push(e);
                let mut msg = out.join("\n");
                if let Ok(s) = capture(ctx, p, None) {
                    msg.push('\n');
                    msg.push_str(&publish(ctx, &s, "goto stopped"));
                }
                return Err(msg);
            }
        }
    }
    out.push(fresh.unwrap_or_else(|| focus_line(ctx)));
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version='1.0'?><hierarchy rotation="0">
<node index="0" text="" resource-id="" class="android.widget.FrameLayout" content-desc="" focusable="false" focused="false" clickable="false" visible-to-user="true" bounds="[0,0][1920,1080]">
 <node index="0" text="" resource-id="app:id/tabs" class="androidx.recyclerview.widget.RecyclerView" content-desc="" focusable="false" focused="false" clickable="false" visible-to-user="true" bounds="[0,0][1920,100]">
  <node index="0" text="" resource-id="app:id/tab" class="android.view.ViewGroup" content-desc="" focusable="true" focused="false" clickable="true" visible-to-user="true" bounds="[0,0][300,100]">
   <node text="Home" resource-id="" class="android.widget.TextView" content-desc="" focusable="false" focused="false" clickable="false" visible-to-user="true" bounds="[20,20][200,80]"/>
  </node>
  <node index="1" text="" resource-id="app:id/tab" class="android.view.ViewGroup" content-desc="" focusable="true" focused="false" clickable="true" visible-to-user="true" bounds="[300,0][600,100]">
   <node text="USB Drive" resource-id="" class="android.widget.TextView" content-desc="" focusable="false" focused="false" clickable="false" visible-to-user="true" bounds="[320,20][500,80]"/>
  </node>
 </node>
 <node index="1" text="" resource-id="app:id/list" class="android.widget.LinearLayout" content-desc="" focusable="false" focused="false" clickable="false" visible-to-user="true" bounds="[0,100][1920,1080]">
  <node text="" resource-id="app:id/row" class="android.view.ViewGroup" content-desc="" focusable="true" focused="true" clickable="true" visible-to-user="true" bounds="[0,200][1920,300]">
   <node text="Durandhar" resource-id="" class="android.widget.TextView" content-desc="" focusable="false" focused="false" clickable="false" visible-to-user="true" bounds="[20,220][500,280]"/>
   <node text="3 songs" resource-id="" class="android.widget.TextView" content-desc="" focusable="false" focused="false" clickable="false" visible-to-user="true" bounds="[600,220][900,280]"/>
  </node>
  <node text="Play all" resource-id="app:id/play_all" class="android.widget.Button" content-desc="" focusable="true" focused="false" clickable="true" visible-to-user="false" bounds="[0,400][400,500]"/>
 </node>
</node></hierarchy>"#;

    #[test]
    fn the_tree_keeps_parents_and_subtrees() {
        let n = parse_tree(XML);
        assert_eq!(n.len(), 11);
        assert_eq!(n[2].parent, Some(1));
        assert_eq!(n[2].end, 4, "the tab ends after its text child");
        assert_eq!(n[0].end, 11);
        assert!(!n[10].visible, "visible-to-user=false");
    }

    #[test]
    fn focus_names_the_focused_row_by_its_children_with_a_path() {
        let n = parse_tree(XML);
        let f = focused_index(&n).unwrap();
        assert_eq!(n[f].short_id(), "row");
        let line = focus_line_of(&n);
        assert!(line.starts_with("focus: \"Durandhar | 3 songs\" [ViewGroup] center (960,250)"), "{}", line);
        assert!(line.contains("id=row") && line.contains("path: "), "{}", line);
        assert!(focus_line_of(&parse_tree("<hierarchy></hierarchy>")).contains("none"));
    }

    #[test]
    fn a_label_on_a_child_resolves_to_its_focusable_parent_and_hidden_nodes_are_ignored() {
        let n = parse_tree(XML);
        let (t, level) = pick_target(&n, "USB Drive", None).unwrap();
        assert_eq!((n[t].short_id(), n[t].b.0, level), ("tab", 300, 2));
        // "Play all" exists but is not visible: not a target.
        assert!(pick_target(&n, "Play all", None).is_none());
        // Partial text matches when nothing is exact.
        assert!(pick_target(&n, "usb", None).is_some());
        // The focused row matches by its children's text.
        let f = focused_index(&n).unwrap();
        assert!(focus_level(&n, f, "durandhar") == 2 && focus_level(&n, f, "3 songs") == 2 && focus_level(&n, f, "zzz") == 0);
    }

    #[test]
    fn nth_picks_one_of_several_matching_rows_in_screen_order() {
        let row = |y: i64, title: &str, focused: bool| {
            format!(
                "<node text=\"\" resource-id=\"r\" class=\"V\" content-desc=\"\" focusable=\"true\" focused=\"{}\" clickable=\"true\" visible-to-user=\"true\" bounds=\"[0,{}][1000,{}]\"><node text=\"{}\" class=\"T\" content-desc=\"\" focusable=\"false\" focused=\"false\" clickable=\"false\" visible-to-user=\"true\" bounds=\"[0,{}][500,{}]\"/></node>",
                focused, y, y + 90, title, y, y + 90
            )
        };
        let xml = format!("<hierarchy><node class=\"F\" bounds=\"[0,0][1920,1080]\" visible-to-user=\"true\">{}{}{}</node></hierarchy>", row(100, "Song", false), row(200, "Song", true), row(300, "Song", false));
        let n = parse_tree(&xml);
        let (_, count) = choose(&n, "Song", None, None);
        assert_eq!(count, 3);
        let second = choose(&n, "Song", None, Some(2)).0.unwrap().0;
        assert_eq!(n[second].b.1, 200);
        assert_eq!(n[choose(&n, "Song", None, Some(3)).0.unwrap().0].b.1, 300);
        assert!(choose(&n, "Song", None, Some(4)).0.is_none());
        // The focused row is the 2nd match, and only that one counts as arrived.
        let f = focused_index(&n).unwrap();
        assert!(related(&n, second, f) && !related(&n, choose(&n, "Song", None, Some(1)).0.unwrap().0, f));
    }

    /// Equalizer page: tabs on top, "Bass boost" below and to the LEFT of the focused tab.
    fn eq_page() -> Vec<TNode> {
        let xml = r#"<hierarchy><node class="F" bounds="[0,0][1920,1080]" visible-to-user="true" focusable="false" focused="false">
<node class="Tabs" bounds="[0,0][1920,100]" visible-to-user="true" focusable="false" focused="false">
 <node class="Tab" bounds="[0,0][400,100]" focusable="true" focused="false" visible-to-user="true"><node text="Home" class="T" bounds="[10,10][300,90]" visible-to-user="true"/></node>
 <node class="Tab" bounds="[400,0][800,100]" focusable="true" focused="false" visible-to-user="true"><node text="Equalizer" class="T" bounds="[410,10][700,90]" visible-to-user="true"/></node>
 <node class="Tab" bounds="[800,0][1200,100]" focusable="true" focused="false" visible-to-user="true"><node text="Queue" class="T" bounds="[810,10][1100,90]" visible-to-user="true"/></node>
 <node class="Tab" bounds="[1200,0][1600,100]" focusable="true" focused="true" visible-to-user="true"><node text="Settings" class="T" bounds="[1210,10][1500,90]" visible-to-user="true"/></node>
</node>
<node class="Page" bounds="[0,100][1920,1080]" visible-to-user="true" focusable="false" focused="false">
 <node class="Row" bounds="[100,400][900,500]" focusable="true" focused="false" visible-to-user="true"><node text="Bass boost" class="T" bounds="[110,410][600,490]" visible-to-user="true"/></node>
 <node class="Row" bounds="[100,600][900,700]" focusable="true" focused="false" visible-to-user="true"><node text="Preset" class="T" bounds="[110,610][600,690]" visible-to-user="true"/></node>
</node></node></hierarchy>"#;
        parse_tree(xml)
    }

    #[test]
    fn a_target_below_the_tab_row_is_reached_by_leaving_the_row_first() {
        let n = eq_page();
        let f = focused_index(&n).unwrap();
        let (t, _) = pick_target(&n, "Bass boost", Some(f)).unwrap();
        let container = container_of(&n, f, (1920, 1080)).map(|c| n[c].b);
        assert_eq!(container, Some((0, 0, 1920, 100)), "the tab row holds focus");
        // The old rule pressed LEFT (the target is left of the tab); the way out of the row is DOWN.
        assert_eq!(dir_to(n[f].b, n[t].b, false), "LEFT");
        let order = key_order(n[f].b, n[t].b, container, mates(&n, f, t));
        assert_eq!(order[0], "DOWN");
        assert_eq!(order, vec!["DOWN", "LEFT", "RIGHT", "UP"], "then sideways towards it, sideways away, the key back");
        // A target inside the row keeps the sideways key.
        let (home, _) = pick_target(&n, "Home", Some(f)).unwrap();
        assert_eq!(key_order(n[f].b, n[home].b, container, mates(&n, f, home))[0], "LEFT");
    }

    #[test]
    fn a_row_is_left_vertically_even_when_the_target_is_beyond_its_left_edge() {
        // Tab row x 452-1840; "Bass boost" at x 212-399 (below and left of the whole row). From a tab: DOWN, not LEFT.
        let tab = (1301, 23, 1481, 119);
        let bass = (212, 116, 399, 212);
        let row = (452, 46, 1840, 95);
        assert_eq!(key_order(tab, bass, Some(row), (0, 0))[0], "DOWN");
        // A column (taller than wide) is left sideways.
        assert_eq!(key_order((100, 300, 400, 380), (900, 100, 1200, 200), Some((80, 100, 420, 900)), (0, 0))[0], "RIGHT");
    }

    #[test]
    fn rows_that_touch_by_a_few_pixels_are_not_the_same_row_and_flat_trees_still_leave_the_row_first() {
        // Compose-style flat tree: a row of tabs (y 23-119) and a row of presets (y 116-212) are siblings of one root; the rows
        // overlap by 3 px. From the last preset, a tab up and to the right is reached with UP, not with RIGHT.
        let item = |x0: i64, y0: i64, x1: i64, y1: i64, focused: bool| format!("<node class=\"V\" bounds=\"[{},{}][{},{}]\" focusable=\"true\" focused=\"{}\" visible-to-user=\"true\"><node text=\"t{}\" class=\"T\" bounds=\"[{},{}][{},{}]\" visible-to-user=\"true\"/></node>", x0, y0, x1, y1, focused, x0, x0, y0, x1, y1);
        let xml = format!(
            "<hierarchy><node class=\"F\" bounds=\"[0,0][1920,1080]\" visible-to-user=\"true\">{}{}{}{}{}{}</node></hierarchy>",
            item(452, 23, 589, 116, false),
            item(779, 23, 924, 116, false),
            item(1144, 23, 1285, 119, false),
            item(80, 116, 196, 212, false),
            item(415, 116, 619, 212, false),
            item(779, 116, 950, 212, true)
        );
        let n = parse_tree(&xml);
        let f = focused_index(&n).unwrap();
        let (t, _) = pick_target(&n, "t1144", Some(f)).unwrap();
        assert_eq!(container_of(&n, f, (1920, 1080)), None, "no group around them");
        // Target to the right: no preset lies to the right of the last one (0 ahead in the row); a tab sits above it (1 in the column).
        assert_eq!(mates(&n, f, t), (0, 1));
        assert_eq!(key_order(n[f].b, n[t].b, None, mates(&n, f, t))[0], "UP");
    }

    #[test]
    fn siblings_in_one_row_are_crossed_with_one_call() {
        let n = eq_page();
        let f = focused_index(&n).unwrap();
        let (home, _) = pick_target(&n, "Home", Some(f)).unwrap();
        // Settings -> Queue -> Equalizer -> Home: three presses, sent together.
        assert_eq!(steps_along(&n, f, home, "LEFT"), 3);
        let (bass, _) = pick_target(&n, "Bass boost", Some(f)).unwrap();
        assert_eq!(steps_along(&n, f, bass, "DOWN"), 1, "not siblings: one press at a time");
    }

    #[test]
    fn a_row_is_named_by_all_its_text_so_index_and_title_both_match() {
        let xml = r#"<hierarchy><node class="F" bounds="[0,0][1920,1080]" visible-to-user="true"><node class="Row" bounds="[0,200][1920,300]" focusable="true" focused="true" visible-to-user="true">
<node text="2" class="T" bounds="[0,200][100,300]" visible-to-user="true"/><node text="ab_short" class="T" bounds="[100,200][900,300]" visible-to-user="true"/><node text="0:42" class="T" bounds="[900,200][1000,300]" visible-to-user="true"/></node></node></hierarchy>"#;
        let n = parse_tree(xml);
        let f = focused_index(&n).unwrap();
        assert_eq!(label(&n, f), "2 | ab_short | 0:42");
        assert!(focus_level(&n, f, "ab_short") > 0 && focus_level(&n, f, "2") > 0);
        assert_eq!(pick_target(&n, "ab_short", Some(f)).map(|t| t.0), Some(f));
    }

    #[test]
    fn the_key_points_towards_the_target() {
        let row = (0, 200, 1920, 300);
        let tab = (300, 0, 600, 100);
        assert_eq!(dir_to(row, tab, false), "UP");
        assert_eq!(dir_to(tab, row, false), "DOWN");
        assert_eq!(dir_to((0, 0, 300, 100), (300, 0, 600, 100), false), "RIGHT", "same row: sideways");
        assert_eq!(dir_to((300, 0, 600, 100), (0, 0, 300, 100), false), "LEFT");
        assert_eq!(dir_to((0, 0, 300, 100), (300, 0, 600, 100), true), "DOWN", "after a press that did nothing: the other axis");
    }
}
