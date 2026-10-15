//! Help text: the short list, the remaining commands, per-client guides, and one page per command.

mod pages;
mod root;

pub(crate) use pages::get_tool_help;
pub(crate) use root::*;
