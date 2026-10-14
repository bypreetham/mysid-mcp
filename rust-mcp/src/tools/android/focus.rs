//! D-pad navigation for screens without touch (Android TV): which element has focus, and `goto`, which presses the
//! D-pad until a named element has focus. Reads the accessibility tree (`uiautomator dump`), so a key press is answered
//! with one line of text instead of a screenshot.

use super::touch::*;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
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

fn tree(ctx: &Ctx) -> Result<Vec<TNode>, String> {
    dump_xml(ctx).map(|x| parse_tree(&x))
}

/// Texts of the descendants (a focusable row is usually labelled by its children).
fn subtree_texts(nodes: &[TNode], i: usize) -> Vec<String> {
    (i + 1..nodes[i].end).filter(|&j| nodes[j].visible).filter_map(|j| Some(nodes[j].own()).filter(|s| !s.is_empty())).take(8).collect()
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

fn label(nodes: &[TNode], i: usize) -> String {
    let own = nodes[i].own();
    if !own.is_empty() {
        return clip(&own, 40);
    }
    let kids = subtree_texts(nodes, i);
    if !kids.is_empty() {
        return clip(&kids[0], 40);
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
    ) + &also(nodes, i)
}

/// The other texts of a row labelled by its first child (`also: 3 songs | 4:12`).
fn also(nodes: &[TNode], i: usize) -> String {
    if !nodes[i].own().is_empty() {
        return String::new();
    }
    let rest: Vec<String> = subtree_texts(nodes, i).iter().skip(1).take(3).map(|s| clip(s, 24)).collect();
    if rest.is_empty() { String::new() } else { format!(" also: {}", rest.join(" | ")) }
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

/// Waits until two screen captures in a row are equal (or `cap_ms` passes). Returns (ms waited, settled).
pub(super) fn wait_settled(ctx: &Ctx, cap_ms: u64) -> (u64, bool) {
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

/// Which D-pad key moves focus from `f` towards `t`. `alt` flips to the other axis (used after a press that did nothing).
fn dir_to(f: (i64, i64, i64, i64), t: (i64, i64, i64, i64), alt: bool) -> &'static str {
    let (fx, fy) = ((f.0 + f.2) / 2, (f.1 + f.3) / 2);
    let (tx, ty) = ((t.0 + t.2) / 2, (t.1 + t.3) / 2);
    let (dx, dy) = (tx - fx, ty - fy);
    let y_overlap = f.1 < t.3 && t.1 < f.3;
    let x_overlap = f.0 < t.2 && t.0 < f.2;
    let horizontal = if y_overlap && !x_overlap {
        true
    } else if x_overlap && !y_overlap {
        false
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

fn press(ctx: &Ctx, key: &str) -> Result<(), String> {
    shell_input(ctx, &["keyevent", &format!("KEYCODE_DPAD_{}", key)])
}

fn visible_focusables(nodes: &[TNode]) -> String {
    let labels: Vec<String> = (0..nodes.len()).filter(|&i| nodes[i].visible && nodes[i].focusable).take(10).map(|i| format!("\"{}\"", label(nodes, i))).collect();
    if labels.is_empty() { "no focusable element is visible".to_string() } else { format!("focusable on screen: {}", labels.join(", ")) }
}

/// One target: press the D-pad until it has focus. Returns the summary line.
/// Returns the summary line and, when no key was pressed after the last read, the focus line of that read.
fn goto_one(ctx: &Ctx, p: &Value, q: &str, max: usize, select: bool) -> Result<(String, Option<String>), String> {
    let scroll_key = text(p, &["scroll", "dir"]).map(|s| s.to_uppercase()).filter(|s| matches!(s.as_str(), "UP" | "DOWN" | "LEFT" | "RIGHT")).unwrap_or_else(|| "DOWN".to_string());
    let mut pressed: Vec<String> = Vec::new();
    let mut last: Option<(String, Option<(i64, i64)>)> = None;
    let nth = num(p, &["nth", "row"]).map(|n| n.max(1.0) as usize);
    let mut stuck = 0;
    let mut alt = false;
    let mut seen: HashMap<(i64, i64), u32> = HashMap::new();
    let trail = |pressed: &Vec<String>| if pressed.is_empty() { "no presses".to_string() } else { format!("{} press(es): {}", pressed.len(), pressed.join(" ")) };
    loop {
        let nodes = tree(ctx).map_err(|e| format!("goto \"{}\": cannot read the screen after {}: {}", q, trail(&pressed), e))?;
        let fi = focused_index(&nodes);
        let fpos = fi.map(|i| (nodes[i].cx(), nodes[i].cy()));
        let now = fi.map_or("nothing".to_string(), |i| format!("\"{}\"", label(&nodes, i)));

        if let Some(pos) = fpos {
            let n = seen.entry(pos).or_insert(0);
            *n += 1;
            if *n >= 4 {
                return Err(format!("goto \"{}\": focus is going in circles around {} ({}). {}", q, now, trail(&pressed), visible_focusables(&nodes)));
            }
        }
        if let Some((key, before)) = last.take() {
            match (fpos, before) {
                (None, _) => return Err(format!("goto \"{}\": focus disappeared after pressing {} ({}). The screen may have changed.", q, key, trail(&pressed))),
                (Some(pos), Some(b)) if pos == b => {
                    stuck += 1;
                    if stuck >= 2 {
                        if pick_target(&nodes, q, None).is_none() {
                            return Err(format!("goto \"{}\": it is not on this screen, and focus cannot move further {} ({}). It stays on {}. {}", q, key, trail(&pressed), now, visible_focusables(&nodes)));
                        }
                        return Err(format!("goto \"{}\": focus did not move after pressing {} ({}). It stays on {}. {}", q, key, trail(&pressed), now, visible_focusables(&nodes)));
                    }
                    alt = true;
                }
                (Some(pos), Some(b)) => {
                    stuck = 0;
                    alt = false;
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
                    let (ms, settled) = wait_settled(ctx, SETTLE_CAP_MS);
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
        let key = match (fi, target) {
            (_, None) => scroll_key.clone(),
            (Some(f), Some((t, _))) => dir_to(nodes[f].b, nodes[t].b, alt).to_string(),
            (None, Some((t, _))) => dir_to((0, 0, ctx.dev_w, ctx.dev_h), nodes[t].b, false).to_string(),
        };
        press(ctx, &key)?;
        pressed.push(key.clone());
        last = Some((key, fpos));
        // The next accessibility read is the check; a screen capture per press would only slow the walk.
        thread::sleep(Duration::from_millis(250));
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
        assert!(line.starts_with("focus: \"Durandhar\" [ViewGroup] center (960,250)"), "{}", line);
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
