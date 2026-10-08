pub mod card;
pub mod flow;
pub mod map;

/// Line-budgeted output buffer: keeps results small enough to be cheap for an LLM to read.
pub struct Out {
    lines: Vec<String>,
    cap: usize,
    dropped: usize,
}

impl Out {
    pub fn new(cap: usize) -> Self {
        Self { lines: Vec::new(), cap, dropped: 0 }
    }

    /// Returns false once the budget is spent (the line is counted, not stored).
    pub fn push(&mut self, line: impl Into<String>) -> bool {
        if self.lines.len() >= self.cap {
            self.dropped += 1;
            false
        } else {
            self.lines.push(line.into());
            true
        }
    }

    pub fn full(&self) -> bool {
        self.lines.len() >= self.cap
    }

    pub fn finish(mut self) -> String {
        if self.dropped > 0 {
            self.lines
                .push(format!("... {} more line(s) hidden - lower --depth or query a narrower symbol", self.dropped));
        }
        self.lines.join("\n")
    }
}
