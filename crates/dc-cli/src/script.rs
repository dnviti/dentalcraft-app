//! Commands given with `--cmd` and `--cmd-file`.

use std::path::Path;

use serde_json::Value;

/// One command to run, with where it came from (for error messages).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// Command id.
    pub id: String,
    /// Parameters (`null` when none were given).
    pub params: Value,
    /// `--cmd N` or `FILE:LINE`.
    pub origin: String,
}

/// Parses `id=JSON` or a bare `id`.
pub fn parse_cmd(text: &str, origin: &str) -> Result<Step, String> {
    let text = text.trim();
    let (id, params) = match text.split_once('=') {
        Some((id, json)) => {
            let params: Value = serde_json::from_str(json.trim())
                .map_err(|e| format!("{origin}: parameters of {}: {e}", id.trim()))?;
            if !params.is_object() && !params.is_null() {
                return Err(format!(
                    "{origin}: parameters of {} must be a JSON object",
                    id.trim()
                ));
            }
            (id.trim(), params)
        }
        None => (text, Value::Null),
    };
    if id.is_empty() || id.contains(char::is_whitespace) {
        return Err(format!(
            "{origin}: expected COMMAND or COMMAND=JSON, got {text:?}"
        ));
    }
    Ok(Step {
        id: id.to_owned(),
        params,
        origin: origin.to_owned(),
    })
}

/// Parses one line of a command file: `{"id": ..., "params": {...}}` (JSON
/// lines), `id=JSON` or `id`. `None` for blank lines and `#` comments.
pub fn parse_line(line: &str, origin: &str) -> Option<Result<Step, String>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    if !line.starts_with('{') {
        return Some(parse_cmd(line, origin));
    }
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(Err(format!("{origin}: {e}"))),
    };
    let Some(id) = v.get("id").and_then(Value::as_str) else {
        return Some(Err(format!("{origin}: missing string field \"id\"")));
    };
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    if !params.is_object() && !params.is_null() {
        return Some(Err(format!("{origin}: \"params\" must be a JSON object")));
    }
    Some(Ok(Step {
        id: id.to_owned(),
        params,
        origin: origin.to_owned(),
    }))
}

/// Reads a command file (UTF-8, optional BOM, LF or CRLF).
pub fn read_cmd_file(path: &Path) -> Result<Vec<Step>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| parse_line(l, &format!("{}:{}", path.display(), i + 1)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cmd_forms() {
        let s = parse_cmd("project.new={\"patient_code\":\"A=1\"}", "--cmd 1").unwrap();
        assert_eq!(s.id, "project.new");
        assert_eq!(s.params, json!({"patient_code": "A=1"}));
        assert_eq!(parse_cmd("edit.undo", "x").unwrap().params, Value::Null);
        assert!(parse_cmd("a=[1]", "x").is_err());
        assert!(parse_cmd("a={", "x").unwrap_err().starts_with("x: "));
        assert!(parse_cmd("", "x").is_err());
    }

    #[test]
    fn file_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.jsonl");
        std::fs::write(
            &p,
            "\u{feff}# setup\r\n{\"id\":\"project.new\",\"params\":{}}\r\n\r\nedit.undo\nview.set={\"view\":\"iso\"}\n",
        )
        .unwrap();
        let steps = read_cmd_file(&p).unwrap();
        let ids: Vec<&str> = steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["project.new", "edit.undo", "view.set"]);
        assert!(
            steps[0].origin.ends_with("c.jsonl:2"),
            "{}",
            steps[0].origin
        );
        std::fs::write(&p, "{\"params\":{}}\n").unwrap();
        assert!(read_cmd_file(&p).unwrap_err().contains("c.jsonl:1"));
    }
}
