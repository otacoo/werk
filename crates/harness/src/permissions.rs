//! Permission engine: reads run free, mutations need a scoped grant.
//! Approvals gate whether a tool runs, never where (see `sandbox`).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Dangerous-call identity; `command` scopes shell heads, else tool-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalKey {
    pub tool: String,
    pub command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Once,
    Session,
    Project,
    Global,
}

impl Scope {
    pub fn ttl_secs(self) -> Option<u64> {
        match self {
            Scope::Once => None,
            Scope::Session => Some(30 * 60),
            Scope::Project | Scope::Global => Some(30 * 24 * 60 * 60),
        }
    }

    pub fn persistable(self) -> bool {
        matches!(self, Scope::Project | Scope::Global)
    }
}

/// A granted approval; `command` must match exactly when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    NeedsApproval,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// In-memory grants shared across runs (`Arc<Mutex<..>>`); `Once` grants
/// are consumed by the check that allows them. Persistence lives in the
/// driver (save `persistable()` on change, `load_persisted` at startup).
#[derive(Debug, Default)]
pub struct PermissionEngine {
    grants: Vec<Grant>,
}

impl PermissionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn grant(
        &mut self,
        tool: &str,
        command: Option<&str>,
        scope: Scope,
        project: Option<String>,
    ) {
        let now = now_secs();
        let expires = scope.ttl_secs().map(|ttl| now + ttl as i64);
        let same_key = |g: &Grant| g.tool == tool && g.command.as_deref() == command;
        let live = |g: &Grant| g.expires.map(|e| e > now).unwrap_or(true);
        // A live global grant already covers narrower scopes.
        if scope == Scope::Project
            && self
                .grants
                .iter()
                .any(|g| same_key(g) && g.scope == Scope::Global && live(g))
        {
            return;
        }
        // A new global grant makes matching project grants redundant.
        if scope == Scope::Global {
            self.grants.retain(|g| !(same_key(g) && g.scope == Scope::Project));
        }
        // Refresh the same permission instead of stacking duplicates, so the
        // persisted file stays one entry per unique permission.
        self.grants.retain(|g| !(same_key(g) && g.scope == scope && g.project == project));
        self.grants.push(Grant {
            tool: tool.to_string(),
            command: command.map(str::to_string),
            scope,
            expires,
            project,
        });
    }

    pub fn check(&mut self, key: &ApprovalKey, project: Option<&str>) -> Decision {
        let now = now_secs();
        let hit = self.grants.iter().position(|g| {
            if g.tool != key.tool || g.command != key.command {
                return false;
            }
            if let Some(exp) = g.expires {
                if exp <= now {
                    return false;
                }
            }
            match g.scope {
                Scope::Once | Scope::Session => true,
                Scope::Project | Scope::Global => {
                    g.project.as_deref().is_none()
                        || project.is_none()
                        || g.project.as_deref() == project
                }
            }
        });
        match hit {
            // Single-use grants are consumed by the check that allows them.
            Some(i) if self.grants[i].scope == Scope::Once => {
                self.grants.remove(i);
                Decision::Allowed
            }
            Some(_) => Decision::Allowed,
            None => Decision::NeedsApproval,
        }
    }

    pub fn persistable(&self) -> Vec<Grant> {
        let now = now_secs();
        self.grants
            .iter()
            .filter(|g| {
                g.scope.persistable() && g.expires.map(|exp| exp > now).unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    pub fn load_persisted(&mut self, grants: Vec<Grant>) {
        let now = now_secs();
        for g in grants
            .into_iter()
            .filter(|g| g.expires.map(|exp| exp > now).unwrap_or(true))
        {
            let dup = self.grants.iter().position(|e| {
                e.tool == g.tool
                    && e.command == g.command
                    && e.scope == g.scope
                    && e.project == g.project
            });
            match dup {
                // Old files stacked duplicates; keep the newest expiry.
                Some(i) => {
                    if g.expires > self.grants[i].expires {
                        self.grants[i] = g;
                    }
                }
                None => self.grants.push(g),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(tool: &str) -> ApprovalKey {
        ApprovalKey { tool: tool.into(), command: None }
    }

    #[test]
    fn once_grants_single_use() {
        let mut e = PermissionEngine::new();
        assert_eq!(e.check(&key("write_file"), Some("p")), Decision::NeedsApproval);
        e.grant("write_file", None, Scope::Once, Some("p".into()));
        assert_eq!(e.check(&key("write_file"), Some("p")), Decision::Allowed);
        assert_eq!(
            e.check(&key("write_file"), Some("p")),
            Decision::NeedsApproval,
            "consumed"
        );
    }

    #[test]
    fn command_scope_must_match_exactly() {
        let mut e = PermissionEngine::new();
        e.grant("exec", Some("git"), Scope::Session, None);
        let git = ApprovalKey { tool: "exec".into(), command: Some("git".into()) };
        let npm = ApprovalKey { tool: "exec".into(), command: Some("npm".into()) };
        assert_eq!(e.check(&git, None), Decision::Allowed);
        assert_eq!(e.check(&npm, None), Decision::NeedsApproval);
        assert_eq!(e.check(&key("exec"), None), Decision::NeedsApproval);
    }

    #[test]
    fn project_grants_stay_home() {
        let mut e = PermissionEngine::new();
        e.grant("write_file", None, Scope::Project, Some("a".into()));
        assert_eq!(e.check(&key("write_file"), Some("a")), Decision::Allowed);
        assert_eq!(e.check(&key("write_file"), Some("b")), Decision::NeedsApproval);
    }

    #[test]
    fn expired_grants_denied_never_persisted_loaded() {
        let mut e = PermissionEngine::new();
        e.load_persisted(vec![Grant {
            tool: "x".into(),
            command: None,
            scope: Scope::Project,
            expires: Some(now_secs() - 1),
            project: None,
        }]);
        assert_eq!(e.check(&key("x"), None), Decision::NeedsApproval);
        assert!(e.persistable().is_empty());
    }

    #[test]
    fn repeated_grants_refresh_instead_of_stacking() {
        let mut e = PermissionEngine::new();
        e.grant("write_file", None, Scope::Project, Some("p".into()));
        e.grant("write_file", None, Scope::Project, Some("p".into()));
        e.grant("write_file", None, Scope::Project, Some("q".into()));
        assert_eq!(e.persistable().len(), 2, "one per unique project");
    }

    #[test]
    fn global_grants_cover_and_replace_project_grants() {
        let mut e = PermissionEngine::new();
        e.grant("exec", Some("git"), Scope::Project, Some("p".into()));
        e.grant("exec", Some("git"), Scope::Global, None);
        assert_eq!(e.persistable().len(), 1, "project entry is redundant");
        // A later project grant is skipped while the global one is live.
        e.grant("exec", Some("git"), Scope::Project, Some("p".into()));
        assert_eq!(e.persistable().len(), 1);
        let git = ApprovalKey { tool: "exec".into(), command: Some("git".into()) };
        assert_eq!(e.check(&git, Some("p")), Decision::Allowed);
    }

    #[test]
    fn loading_dedupes_and_keeps_the_newest_expiry() {
        let now = now_secs();
        let g = |exp| Grant {
            tool: "x".into(),
            command: None,
            scope: Scope::Project,
            expires: Some(exp),
            project: Some("p".into()),
        };
        let mut e = PermissionEngine::new();
        e.load_persisted(vec![g(now + 10), g(now + 99)]);
        assert_eq!(e.persistable().len(), 1);
        assert_eq!(e.persistable()[0].expires, Some(now + 99));
    }
}
