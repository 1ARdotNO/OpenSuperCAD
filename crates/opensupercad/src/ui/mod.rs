//! The GPUI user interface (work in progress, see #7).

use std::path::PathBuf;

pub fn run(_path: Option<PathBuf>) -> anyhow::Result<()> {
    anyhow::bail!(
        "the graphical interface is not built yet (tracked in #7); \
         `opensupercad mcp` and `opensupercad doctor` are available"
    )
}
