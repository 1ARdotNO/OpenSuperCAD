use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Agent,
    /// Agent reasoning ("thoughts"), shown collapsed.
    Thought,
    /// A tool call and its outcome, rendered as a card.
    Tool,
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// Unix timestamp (seconds).
    pub time: u64,
    /// Checkpoint commit taken after this turn, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

/// An AI conversation belonging to one project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub title: String,
    /// Agent id from the agent registry.
    pub agent: String,
    /// ACP session id, used to resume the session with agents supporting
    /// `session/load`.
    #[serde(default)]
    pub session_id: Option<String>,
    pub created: u64,
    pub updated: u64,
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub agent: String,
    pub updated: u64,
}

impl Thread {
    pub fn new(agent: impl Into<String>) -> Self {
        let now = crate::now();
        Self {
            id: new_id(),
            title: "New thread".into(),
            agent: agent.into(),
            session_id: None,
            created: now,
            updated: now,
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) -> &mut Message {
        let text = text.into();
        if role == Role::User && self.messages.iter().all(|m| m.role != Role::User) {
            self.title = title_from(&text);
        }
        self.updated = crate::now();
        self.messages.push(Message {
            role,
            text,
            time: self.updated,
            checkpoint: None,
        });
        self.messages.last_mut().expect("just pushed")
    }

    pub fn summary(&self) -> ThreadSummary {
        ThreadSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            agent: self.agent.clone(),
            updated: self.updated,
        }
    }
}

fn title_from(text: &str) -> String {
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut title: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        title.push('…');
    }
    if title.is_empty() {
        "New thread".into()
    } else {
        title
    }
}

fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_follows_first_user_message() {
        let mut t = Thread::new("claude");
        t.push(Role::System, "hello");
        assert_eq!(t.title, "New thread");
        t.push(
            Role::User,
            "\n  Make a phone stand with a 70° angle\nand cable slot",
        );
        assert_eq!(t.title, "Make a phone stand with a 70° angle");
        t.push(Role::User, "second");
        assert_eq!(t.title, "Make a phone stand with a 70° angle");
        assert_ne!(Thread::new("a").id, Thread::new("a").id);
    }
}
