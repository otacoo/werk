//! The agent's own task list: a short checklist the orchestrator keeps and
//! re-reads every turn (prompt state, not history; subagents never touch it).

use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::permissions::ApprovalKey;
use crate::tools::Tool;

pub const TODO_CAP: usize = 10;
const TEXT_CAP: usize = 120;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    pub done: bool,
}

/// Shared list: the tool mutates it, the loop renders it, werk saves it.
pub type TodoList = Arc<Mutex<Vec<TodoItem>>>;

/// Prompt block for the current list; None when there is nothing to show.
pub fn render_block(items: &[TodoItem]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let done = items.iter().filter(|i| i.done).count();
    let mut out = format!("Current task list ({done}/{}):", items.len());
    for (i, item) in items.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. [{}] {}",
            i + 1,
            if item.done { "x" } else { " " },
            item.text
        ));
    }
    out.push_str("\nKeep it current: complete or drop items as you go.");
    Some(out)
}

/// Tool result: a short confirmation plus the rendered list.
fn confirmation(prefix: &str, items: &[TodoItem]) -> String {
    match render_block(items) {
        Some(block) => format!("{prefix}\n{block}"),
        None => format!("{prefix}\nTask list is empty."),
    }
}

pub struct TodoTool {
    list: TodoList,
}

impl TodoTool {
    pub fn new(list: TodoList) -> Self {
        Self { list }
    }
}

impl Tool for TodoTool {
    fn name(&self) -> String {
        "todo".to_string()
    }
    fn description(&self) -> String {
        "Maintain your short task list for this run (max 10 items): action 'add' with text, \
         'complete' or 'drop' with the item number. Use it for multi-step work and keep it current."
            .to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["add", "complete", "drop"] },
                "text": { "type": "string", "description": "Short item text (add only)" },
                "item": { "type": "integer", "minimum": 1, "description": "1-based item number (complete/drop)" }
            },
            "required": ["action"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let mut items = self.list.lock().unwrap();
        match action.as_str() {
            "add" => {
                let text = args
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    bail!("todo add needs non-empty 'text'");
                }
                if items.len() >= TODO_CAP {
                    bail!("Task list is full ({TODO_CAP} items) — complete or drop one first");
                }
                let text: String = text.chars().take(TEXT_CAP).collect();
                items.push(TodoItem { text, done: false });
                Ok(confirmation("Added.", &items))
            }
            "complete" | "drop" => {
                if items.is_empty() {
                    bail!("Task list is empty — nothing to {action}");
                }
                let item = args.get("item").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if item == 0 || item > items.len() {
                    bail!("todo {action} needs 'item' between 1 and {}", items.len());
                }
                if action == "complete" {
                    items[item - 1].done = true;
                    Ok(confirmation("Completed.", &items))
                } else {
                    let removed = items.remove(item - 1);
                    Ok(confirmation(&format!("Dropped \"{}\".", removed.text), &items))
                }
            }
            other => bail!("Unknown todo action '{other}' (add, complete, drop)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> (TodoList, TodoTool) {
        let list: TodoList = Arc::new(Mutex::new(Vec::new()));
        (list.clone(), TodoTool::new(list))
    }

    #[test]
    fn add_complete_and_drop() {
        let (list, tool) = tool();
        let out = tool.execute(&json!({"action": "add", "text": "fix parser"})).unwrap();
        assert!(out.contains("1. [ ] fix parser"), "{out}");
        tool.execute(&json!({"action": "add", "text": "add tests"})).unwrap();
        let out = tool.execute(&json!({"action": "complete", "item": 1})).unwrap();
        assert!(out.contains("1. [x] fix parser") && out.contains("(1/2)"), "{out}");
        let out = tool.execute(&json!({"action": "drop", "item": 2})).unwrap();
        assert!(out.contains("Dropped \"add tests\"") && !out.contains("add tests\n"), "{out}");
        assert_eq!(list.lock().unwrap().len(), 1);
    }

    #[test]
    fn cap_and_validation() {
        let (_list, tool) = tool();
        for i in 0..TODO_CAP {
            tool.execute(&json!({"action": "add", "text": format!("item {i}")})).unwrap();
        }
        let err = tool.execute(&json!({"action": "add", "text": "one more"})).unwrap_err();
        assert!(err.to_string().contains("full"), "{err}");
        let err = tool.execute(&json!({"action": "add", "text": "  "})).unwrap_err();
        assert!(err.to_string().contains("text"), "{err}");
        let err = tool.execute(&json!({"action": "complete", "item": 99})).unwrap_err();
        assert!(err.to_string().contains("between 1 and"), "{err}");
        let err = tool.execute(&json!({"action": "wat"})).unwrap_err();
        assert!(err.to_string().contains("Unknown todo action"), "{err}");
    }

    #[test]
    fn empty_list_has_no_block() {
        assert!(render_block(&[]).is_none());
        let block = render_block(&[TodoItem { text: "x".into(), done: false }]).unwrap();
        assert!(block.contains("(0/1)") && block.contains("1. [ ] x"), "{block}");
    }
}
