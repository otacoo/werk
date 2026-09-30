//! The read-only `lsp` tool: precise navigation via the project's language server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::client::Client;
use super::servers::{language_id, ServerSpec};
use super::{rel_display, uri, LspManager};
use crate::permissions::ApprovalKey;
use crate::sandbox::PathJail;
use crate::tools::Tool;

const QUERY_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_LOCATIONS: usize = 20;
const MAX_SYMBOLS: usize = 50;
const MAX_SNIPPET: usize = 120;
const MAX_HOVER: usize = 2000;

pub struct LspTool {
    jail: Arc<PathJail>,
    lsp: Arc<LspManager>,
}

impl LspTool {
    pub fn new(jail: Arc<PathJail>, lsp: Arc<LspManager>) -> Self {
        Self { jail, lsp }
    }
}

impl Tool for LspTool {
    fn name(&self) -> String {
        "lsp".to_string()
    }

    fn description(&self) -> String {
        "Language-server code intelligence: jump to definitions, find references, hover \
         docs, and list document or workspace symbols. Prefer it over grep for navigating \
         code. Line and character are 1-based."
            .to_string()
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["definition", "references", "hover", "document_symbol", "workspace_symbol", "implementation"]
                },
                "file": {"type": "string", "description": "Project-relative path of a source file."},
                "line": {"type": "integer", "description": "1-based line for positional ops."},
                "character": {"type": "integer", "description": "1-based column for positional ops."},
                "query": {"type": "string", "description": "Symbol query for workspace_symbol."}
            },
            "required": ["op", "file"]
        })
    }

    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }

    fn execute(&self, args: &Value) -> Result<String> {
        let op = args.get("op").and_then(Value::as_str).unwrap_or("");
        let file_arg = args.get("file").and_then(Value::as_str).unwrap_or("");
        if file_arg.is_empty() {
            bail!("'file' is required");
        }
        let root = self.jail.root().to_path_buf();
        let path = self.jail.check_read(file_arg)?;
        let (client, spec) = self
            .lsp
            .client(&path, &root)
            .ok_or_else(|| anyhow::anyhow!("no language server available for {file_arg}"))?;
        let doc_uri = uri::path_to_uri(&path);

        match op {
            "workspace_symbol" => {
                let query = args.get("query").and_then(Value::as_str).unwrap_or("");
                if query.trim().is_empty() {
                    bail!("'query' is required for workspace_symbol");
                }
                let result = client.request(
                    "workspace/symbol",
                    json!({"query": query}),
                    QUERY_TIMEOUT,
                )?;
                Ok(format_symbols(&root, &result, false, None))
            }
            "document_symbol" => {
                open_current(&client, &spec, &path, &doc_uri)?;
                let result = client.request(
                    "textDocument/documentSymbol",
                    json!({"textDocument": {"uri": doc_uri}}),
                    QUERY_TIMEOUT,
                )?;
                Ok(format_symbols(&root, &result, true, Some(&path)))
            }
            "definition" | "implementation" | "references" | "hover" => {
                open_current(&client, &spec, &path, &doc_uri)?;
                let line = args.get("line").and_then(Value::as_u64).unwrap_or(0);
                let character = args.get("character").and_then(Value::as_u64).unwrap_or(0);
                if line == 0 || character == 0 {
                    bail!("'line' and 'character' are 1-based and required for {op}");
                }
                let position = json!({
                    "textDocument": {"uri": doc_uri},
                    "position": {"line": line - 1, "character": character - 1}
                });
                let (method, params) = match op {
                    "definition" => ("textDocument/definition", position),
                    "implementation" => ("textDocument/implementation", position),
                    "references" => {
                        let mut params = position;
                        params["context"] = json!({"includeDeclaration": true});
                        ("textDocument/references", params)
                    }
                    _ => ("textDocument/hover", position),
                };
                let result = client.request(method, params, QUERY_TIMEOUT)?;
                if op == "hover" {
                    Ok(format_hover(&result))
                } else {
                    Ok(format_locations(&root, &result))
                }
            }
            other => bail!("unknown lsp op '{other}'"),
        }
    }
}

fn open_current(client: &Client, spec: &ServerSpec, path: &Path, doc_uri: &str) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    client.ensure_open(doc_uri, language_id(spec, path), &text);
    Ok(())
}

/// One line per location: `path:line:col: source text`.
fn format_locations(root: &Path, result: &Value) -> String {
    let items: Vec<&Value> = match result {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        single => vec![single],
    };
    let mut lines = Vec::new();
    let mut file_cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for item in items.iter().take(MAX_LOCATIONS) {
        let target = item
            .get("targetUri")
            .or_else(|| item.get("uri"))
            .and_then(Value::as_str);
        let range = item
            .get("targetSelectionRange")
            .or_else(|| item.get("range"));
        let Some(target) = target else { continue };
        let Some(path) = uri::uri_to_path(target) else {
            continue;
        };
        let line = range
            .and_then(|r| r.pointer("/start/line"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        let col = range
            .and_then(|r| r.pointer("/start/character"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        let snippet = line_text(&mut file_cache, &path, line);
        let where_ = format!("{}:{line}:{col}", rel_display(root, &path));
        lines.push(if snippet.is_empty() {
            where_
        } else {
            format!("{where_}: {snippet}")
        });
    }
    if lines.is_empty() {
        return "No results.".to_string();
    }
    if items.len() > MAX_LOCATIONS {
        lines.push(format!("… {} more", items.len() - MAX_LOCATIONS));
    }
    lines.join("\n")
}

fn line_text(cache: &mut HashMap<PathBuf, Vec<String>>, path: &Path, line: u64) -> String {
    let lines = cache.entry(path.to_path_buf()).or_insert_with(|| {
        std::fs::read_to_string(path)
            .map(|text| text.lines().map(str::to_string).collect())
            .unwrap_or_default()
    });
    let raw = lines
        .get(line.saturating_sub(1) as usize)
        .map(|l| l.trim().to_string())
        .unwrap_or_default();
    raw.chars().take(MAX_SNIPPET).collect()
}

/// `kind name — path:line:col`, children indented (document symbols).
/// `fallback` is the queried file for document symbols, which carry no URI.
fn format_symbols(root: &Path, result: &Value, hierarchical: bool, fallback: Option<&Path>) -> String {
    fn kind(k: u64) -> &'static str {
        match k {
            1 => "file",
            2 => "module",
            3 => "namespace",
            4 => "package",
            5 => "class",
            6 => "method",
            7 => "property",
            8 => "field",
            9 => "constructor",
            10 => "enum",
            11 => "interface",
            12 => "function",
            13 => "variable",
            14 => "constant",
            15 => "string",
            16 => "number",
            17 => "boolean",
            18 => "array",
            19 => "object",
            20 => "key",
            21 => "null",
            22 => "enum member",
            23 => "struct",
            24 => "event",
            25 => "operator",
            26 => "type parameter",
            _ => "symbol",
        }
    }

    fn location(root: &Path, node: &Value, fallback: Option<&Path>) -> String {
        let uri = node.pointer("/location/uri").and_then(Value::as_str);
        let range = node
            .pointer("/location/range")
            .or_else(|| node.get("range"));
        let (line, col) = range
            .map(|r| {
                (
                    r.pointer("/start/line")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        + 1,
                    r.pointer("/start/character")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        + 1,
                )
            })
            .unwrap_or((0, 0));
        let path = uri
            .and_then(uri::uri_to_path)
            .or_else(|| fallback.map(Path::to_path_buf));
        match path {
            Some(path) => format!("{}:{line}:{col}", rel_display(root, &path)),
            None => format!("{line}:{col}"),
        }
    }

    fn walk(
        root: &Path,
        node: &Value,
        depth: usize,
        out: &mut Vec<String>,
        hierarchical: bool,
        fallback: Option<&Path>,
    ) {
        if out.len() >= MAX_SYMBOLS {
            return;
        }
        let name = node.get("name").and_then(Value::as_str).unwrap_or("");
        let kind_id = node.get("kind").and_then(Value::as_u64).unwrap_or(0);
        let container = node
            .get("containerName")
            .and_then(Value::as_str)
            .map(|c| format!("{c}."))
            .unwrap_or_default();
        out.push(format!(
            "{}{} {container}{name} — {}",
            "  ".repeat(depth),
            kind(kind_id),
            location(root, node, fallback)
        ));
        if hierarchical {
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                for child in children {
                    walk(root, child, depth + 1, out, hierarchical, fallback);
                }
            }
        }
    }

    let nodes: Vec<&Value> = match result {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        single => vec![single],
    };
    let total = nodes.len();
    let mut out = Vec::new();
    for node in nodes {
        walk(root, node, 0, &mut out, hierarchical, fallback);
    }
    if out.is_empty() {
        return "No results.".to_string();
    }
    if total > 1 && out.len() >= MAX_SYMBOLS {
        out.push(format!("… capped at {MAX_SYMBOLS} symbols"));
    }
    out.join("\n")
}

/// Hover contents: string, `{value}`, `{language, value}`, or a list of those.
fn format_hover(result: &Value) -> String {
    fn piece(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Object(_) => {
                let text = value.get("value").and_then(Value::as_str)?;
                match value.get("language").and_then(Value::as_str) {
                    Some(language) => Some(format!("```{language}\n{text}\n```")),
                    None => Some(text.to_string()),
                }
            }
            _ => None,
        }
    }
    let contents = result.get("contents").unwrap_or(result);
    let raw = match contents {
        Value::Null => String::new(),
        Value::Array(items) => items.iter().filter_map(piece).collect::<Vec<_>>().join("\n\n"),
        other => piece(other).unwrap_or_default(),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "No hover information.".to_string();
    }
    trimmed.chars().take(MAX_HOVER).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn locations_render_with_snippets() {
        let dir = std::env::temp_dir().join(format!("werk-lsp-fmt-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let file = dir.join("src").join("main.rs");
        std::fs::write(&file, "fn main() {\n    greet();\n}\n").unwrap();
        let result = json!([
            {"uri": uri::path_to_uri(&file), "range": {"start": {"line": 1, "character": 4}}}
        ]);
        let out = format_locations(&dir, &result);
        assert!(out.contains("src/main.rs:2:5: greet();"), "{out}");

        let links = json!([
            {
                "targetUri": uri::path_to_uri(&file),
                "targetSelectionRange": {"start": {"line": 0, "character": 3}},
                "targetRange": {"start": {"line": 0, "character": 0}}
            }
        ]);
        assert!(format_locations(&dir, &links).contains("src/main.rs:1:4: fn main() {"));
        assert_eq!(format_locations(&dir, &json!(null)), "No results.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn symbols_and_hover_render() {
        let root = std::env::temp_dir().join(format!("werk-lsp-sym-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = uri::path_to_uri(&root.join("lib.rs"));
        let result = json!([
            {
                "name": "Thing",
                "kind": 23,
                "range": {"start": {"line": 4, "character": 0}},
                "children": [
                    {"name": "run", "kind": 6, "range": {"start": {"line": 6, "character": 4}}}
                ]
            },
            {
                "name": "helper",
                "kind": 12,
                "location": {"uri": file, "range": {"start": {"line": 19, "character": 0}}}
            }
        ]);
        let out = format_symbols(&root, &result, true, Some(&root.join("lib.rs")));
        assert!(out.contains("struct Thing — lib.rs:5:1"), "{out}");
        assert!(out.contains("  method run — lib.rs:7:5"), "{out}");
        assert!(out.contains("function helper — lib.rs:20:1"), "{out}");

        let hover = json!({
            "contents": {"kind": "markdown", "value": "`fn greet()` — says hi"}
        });
        assert_eq!(format_hover(&hover), "`fn greet()` — says hi");
        let hover = json!({"contents": ["plain", {"language": "rust", "value": "let x"}]});
        assert!(format_hover(&hover).contains("```rust\nlet x\n```"));
        assert_eq!(format_hover(&json!(null)), "No hover information.");
    }
}
