use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    /// `ECHO:` output from `echo()`.
    Echo,
    /// Anything else OpenSCAD printed (progress, statistics, …).
    Info,
}

/// One message from OpenSCAD's console, with a source location if it has one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
}

/// Parse OpenSCAD console output (stderr) into diagnostics.
///
/// Handles both the classic form
/// `ERROR: Parser error in file "/a.scad", line 3: syntax error` and the newer
/// `WARNING: Ignoring unknown variable 'x' in file a.scad, line 5`.
pub fn parse_console(output: &str) -> Vec<Diagnostic> {
    output
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let (severity, rest) = if let Some(r) = line.strip_prefix("ERROR:") {
                (Severity::Error, r)
            } else if let Some(r) = line.strip_prefix("WARNING:") {
                (Severity::Warning, r)
            } else if let Some(r) = line.strip_prefix("ECHO:") {
                (Severity::Echo, r)
            } else if let Some(r) = line.strip_prefix("TRACE:") {
                (Severity::Info, r)
            } else {
                (Severity::Info, line)
            };
            let rest = rest.trim();
            let (file, line_no) = if severity == Severity::Echo {
                (None, None)
            } else {
                location(rest)
            };
            Diagnostic {
                severity,
                message: rest.to_owned(),
                file,
                line: line_no,
            }
        })
        .collect()
}

fn location(msg: &str) -> (Option<String>, Option<usize>) {
    let Some(idx) = msg.rfind("in file ") else {
        return (None, None);
    };
    let tail = &msg[idx + "in file ".len()..];
    let (file, after) = if let Some(quoted) = tail.strip_prefix('"') {
        match quoted.find('"') {
            Some(end) => (&quoted[..end], &quoted[end + 1..]),
            None => (quoted, ""),
        }
    } else {
        match tail.find(", line ") {
            Some(end) => (&tail[..end], &tail[end..]),
            None => (tail.trim_end_matches(['.', ':']), ""),
        }
    };
    let line = after.find("line ").and_then(|i| {
        let digits: String = after[i + 5..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    });
    let file = Some(file.trim().to_owned()).filter(|f| !f.is_empty());
    (file, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        let out = "ERROR: Parser error in file \"/tmp/a b.scad\", line 3: syntax error\n\
                   WARNING: Ignoring unknown variable 'q' in file main.scad, line 12\n\
                   ECHO: \"in file x, line 4\"\n\
                   Rendering Polygon Mesh using Manifold...\n\
                   \n";
        let d = parse_console(out);
        assert_eq!(d.len(), 4);
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].file.as_deref(), Some("/tmp/a b.scad"));
        assert_eq!(d[0].line, Some(3));
        assert_eq!(d[1].severity, Severity::Warning);
        assert_eq!(d[1].file.as_deref(), Some("main.scad"));
        assert_eq!(d[1].line, Some(12));
        assert_eq!(d[2].severity, Severity::Echo);
        assert_eq!(d[2].line, None);
        assert_eq!(d[3].severity, Severity::Info);
    }
}
