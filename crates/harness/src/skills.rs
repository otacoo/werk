//! Skills: folders with a SKILL.md (name + description + instructions).
//! Only names/descriptions reach the prompt; bodies load on demand.

use std::path::PathBuf;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::permissions::ApprovalKey;
use crate::tools::Tool;

pub const SKILL_BODY_CAP: usize = 30_000;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
        })
}

/// Parse name/description from frontmatter (`name:`/`description:` lines).
fn parse_skill_md(text: &str) -> Option<(String, String)> {
    let mut name = None;
    let mut description = None;
    for line in text.lines().take(20) {
        let line = line.trim().trim_start_matches("---").trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(v.trim().trim_matches('"').to_string());
        }
        if name.is_some() && description.is_some() {
            break;
        }
    }
    Some((name?, description?))
}

/// Discover skills under the given roots; later roots win on conflicts.
pub fn discover(roots: &[PathBuf]) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let md = dir.join("SKILL.md");
            if !md.is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&md) else {
                continue;
            };
            let Some((name, description)) = parse_skill_md(&text) else {
                continue;
            };
            if !valid_name(&name) {
                continue;
            }
            out.retain(|s| s.name != name);
            out.push(Skill { name, description, path: md });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Prompt listing (`- name: description` lines) for the system prompt.
pub fn system_prompt_listing(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nAvailable skills (load with the skill tool):");
    for s in skills {
        out.push_str(&format!("\n- {}: {}", s.name, s.description));
    }
    out
}

/// On-demand skill body loader.
pub struct SkillTool {
    skills: Vec<Skill>,
}

impl SkillTool {
    pub fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }
}

impl Tool for SkillTool {
    fn name(&self) -> String {
        "skill".to_string()
    }
    fn description(&self) -> String {
        "Load a learned skill by name (procedures the agent saved earlier).".to_string()
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        })
    }
    fn approval_key(&self, _args: &Value) -> Option<ApprovalKey> {
        None
    }
    fn execute(&self, args: &Value) -> Result<String> {
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
        if name.is_empty() {
            bail!("'name' must not be empty");
        }
        let Some(skill) = self.skills.iter().find(|s| s.name == name) else {
            let known: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
            bail!("Unknown skill '{name}' (known: {})", known.join(", "));
        };
        let text = std::fs::read_to_string(&skill.path)?;
        let chars: String = text.chars().take(SKILL_BODY_CAP).collect();
        Ok(if chars.len() < text.chars().count() {
            format!("{chars}\n[…truncated]")
        } else {
            chars
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_dir(tag: &str, folder: &str, md: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("werk-skills-{tag}-{}", std::process::id()));
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), md).unwrap();
        root
    }

    #[test]
    fn discovers_valid_skills_project_beats_global() {
        let global = skill_dir("g", "deploy", "---\nname: deploy\ndescription: global deploy\n---\n# Deploy\n");
        let project = skill_dir("p", "deploy", "---\nname: deploy\ndescription: project deploy\n---\n# Deploy\n");
        std::fs::create_dir_all(project.join("bad")).unwrap();
        std::fs::write(project.join("bad").join("SKILL.md"), "no frontmatter here\n").unwrap();
        let defs = discover(&[global.clone(), project.clone()]);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].description, "project deploy");
        let _ = std::fs::remove_dir_all(&global);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn skill_tool_loads_body() {
        let root = skill_dir("load", "lint", "---\nname: lint\ndescription: lint things\n---\nRun the linter.\n");
        let defs = discover(std::slice::from_ref(&root));
        let tool = SkillTool::new(defs);
        let out = tool.execute(&json!({"name": "lint"})).unwrap();
        assert!(out.contains("linter"));
        assert!(tool.execute(&json!({"name": "nope"})).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
