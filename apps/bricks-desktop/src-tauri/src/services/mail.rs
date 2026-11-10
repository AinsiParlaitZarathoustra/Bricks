//! The messages conversations send each other, and what became of them.
//!
//! A message goes through three steps: `sent` (it waits for the conversation
//! to be free), `in_progress` (the conversation's run it started is going),
//! `answered` (that run ended; its final answer is the reply). It may also
//! fail (the conversation was closed or deleted, or refused it). Whether a
//! message was read is never known, so it is never said.
//!
//! The log of each sender is kept in `desktop/messages/<sender>.json`, so it
//! can be read like any file. A message still under way when the application
//! stops is marked as interrupted at the next start.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailState {
    Sent,
    InProgress,
    Answered,
    Failed,
}

impl MailState {
    pub fn is_final(self) -> bool {
        matches!(self, MailState::Answered | MailState::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub from: String,
    pub to: String,
    pub to_title: String,
    pub text: String,
    pub state: MailState,
    /// The final answer of the run the message started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// How that run ended (`succeeded`, `cancelled`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Why the message failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub sent_at: i64,
    pub updated_at: i64,
}

pub struct Mailbox {
    dir: PathBuf,
    mails: parking_lot::Mutex<HashMap<String, Mail>>,
    /// Conversations answering a message: they may not send one meanwhile
    /// (their answer goes back by itself).
    replying: parking_lot::Mutex<HashMap<String, String>>,
    changed: tokio::sync::Notify,
    /// Serialises the writes of the logs, so the last state is the one kept.
    writing: parking_lot::Mutex<()>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Mailbox {
    /// The logs kept under `dir`; messages left under way are interrupted.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let mut mails = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(list) = serde_json::from_str::<Vec<Mail>>(&text) else {
                    continue;
                };
                for mut mail in list {
                    if !mail.state.is_final() {
                        mail.state = MailState::Failed;
                        mail.reason = Some("interrupted: Bricks was closed before the answer".into());
                    }
                    mails.insert(mail.id.clone(), mail);
                }
            }
        }
        Self {
            dir,
            mails: parking_lot::Mutex::new(mails),
            replying: parking_lot::Mutex::new(HashMap::new()),
            changed: tokio::sync::Notify::new(),
            writing: parking_lot::Mutex::new(()),
        }
    }

    /// Records a new message, as sent.
    pub fn record(&self, from: &str, to: &str, to_title: &str, text: &str) -> Mail {
        let now = now_ms();
        let mail = Mail {
            id: format!("msg-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            from: from.to_string(),
            to: to.to_string(),
            to_title: to_title.to_string(),
            text: text.to_string(),
            state: MailState::Sent,
            answer: None,
            outcome: None,
            reason: None,
            sent_at: now,
            updated_at: now,
        };
        self.mails.lock().insert(mail.id.clone(), mail.clone());
        self.persist(from);
        mail
    }

    pub fn get(&self, id: &str) -> Option<Mail> {
        self.mails.lock().get(id).cloned()
    }

    /// The messages `from` sent, oldest first.
    pub fn sent_by(&self, from: &str) -> Vec<Mail> {
        let mut out: Vec<Mail> = self
            .mails
            .lock()
            .values()
            .filter(|m| m.from == from)
            .cloned()
            .collect();
        out.sort_by_key(|m| m.sent_at);
        out
    }

    /// The run the message started has begun.
    pub fn started(&self, id: &str) {
        let to = self.update(id, |m| m.state = MailState::InProgress);
        if let Some(to) = to {
            self.replying.lock().insert(to, id.to_string());
        }
    }

    /// The run the message started has ended with `answer`.
    pub fn answered(&self, id: &str, answer: String, outcome: String) {
        self.update(id, |m| {
            m.state = MailState::Answered;
            m.answer = Some(answer);
            m.outcome = Some(outcome);
        });
        self.release(id);
    }

    /// The message will get no answer, because of `reason`.
    pub fn failed(&self, id: &str, reason: String) {
        self.update(id, |m| {
            if !m.state.is_final() {
                m.state = MailState::Failed;
                m.reason = Some(reason);
            }
        });
        self.release(id);
    }

    /// Whether `session` is answering a message now.
    pub fn is_replying(&self, session: &str) -> bool {
        self.replying.lock().contains_key(session)
    }

    /// The message once final, or as it is when `timeout` ran out.
    pub async fn wait(&self, id: &str, timeout: Duration) -> Option<Mail> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let mail = self.get(id)?;
            if mail.state.is_final() {
                return Some(mail);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.get(id);
            }
        }
    }

    fn release(&self, id: &str) {
        self.replying.lock().retain(|_, m| m != id);
    }

    /// Applies `change` to the message, saves its sender's log, wakes the
    /// waiters. Returns the message's recipient.
    fn update(&self, id: &str, change: impl FnOnce(&mut Mail)) -> Option<String> {
        let (from, to) = {
            let mut mails = self.mails.lock();
            let mail = mails.get_mut(id)?;
            change(mail);
            mail.updated_at = now_ms();
            (mail.from.clone(), mail.to.clone())
        };
        self.persist(&from);
        self.changed.notify_waiters();
        Some(to)
    }

    fn persist(&self, from: &str) {
        let _writing = self.writing.lock();
        let list = self.sent_by(from);
        let path = log_path(&self.dir, from);
        if let Ok(text) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::create_dir_all(&self.dir);
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

/// The log of the messages `from` sent.
pub fn log_path(dir: &Path, from: &str) -> PathBuf {
    dir.join(format!("{from}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_message_goes_sent_in_progress_answered_and_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        let mailbox = Mailbox::at(dir.path());
        let mail = mailbox.record("a", "b", "B", "bonjour");
        assert_eq!(mail.state, MailState::Sent);
        mailbox.started(&mail.id);
        assert!(mailbox.is_replying("b"));
        assert_eq!(mailbox.get(&mail.id).unwrap().state, MailState::InProgress);
        mailbox.answered(&mail.id, "salut".into(), "succeeded".into());
        assert!(!mailbox.is_replying("b"));
        let done = mailbox.wait(&mail.id, Duration::from_millis(10)).await.unwrap();
        assert_eq!(done.answer.as_deref(), Some("salut"));
        let log = std::fs::read_to_string(log_path(dir.path(), "a")).unwrap();
        assert!(log.contains("\"answered\""), "{log}");
    }

    #[tokio::test]
    async fn waiting_is_bounded_and_wakes_on_the_answer() {
        let dir = tempfile::tempdir().unwrap();
        let mailbox = std::sync::Arc::new(Mailbox::at(dir.path()));
        let mail = mailbox.record("a", "b", "B", "x");
        let early = mailbox.wait(&mail.id, Duration::from_millis(30)).await.unwrap();
        assert_eq!(early.state, MailState::Sent, "a timeout returns the message as it is");
        let m = std::sync::Arc::clone(&mailbox);
        let id = mail.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            m.answered(&id, "ok".into(), "succeeded".into());
        });
        let done = mailbox.wait(&mail.id, Duration::from_secs(5)).await.unwrap();
        assert_eq!(done.state, MailState::Answered);
    }

    #[test]
    fn a_failure_never_replaces_an_answer_and_restarts_interrupt_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let mailbox = Mailbox::at(dir.path());
        let answered = mailbox.record("a", "b", "B", "1");
        mailbox.answered(&answered.id, "r".into(), "succeeded".into());
        mailbox.failed(&answered.id, "late".into());
        assert_eq!(mailbox.get(&answered.id).unwrap().state, MailState::Answered);
        let pending = mailbox.record("a", "c", "C", "2");
        drop(mailbox);
        let again = Mailbox::at(dir.path());
        let p = again.get(&pending.id).unwrap();
        assert_eq!(p.state, MailState::Failed);
        assert!(p.reason.unwrap().contains("interrupted"));
        assert_eq!(again.get(&answered.id).unwrap().state, MailState::Answered);
    }
}
