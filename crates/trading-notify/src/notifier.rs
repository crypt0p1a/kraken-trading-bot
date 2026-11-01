use crate::EventKind;
use ntfy::{Auth, Dispatcher, Payload, Priority, dispatcher::Async};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use ntfy::Priority as NotificationPriority;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub event: EventKind,
    pub title: String,
    pub message: String,
    /// ntfy tags: the emojis shown before the title, e.g. "moneybag"
    pub tags: Vec<&'static str>,
    pub priority: Priority,
}

impl Notification {
    pub fn payload(&self, topic: &str) -> Payload {
        Payload::new(topic)
            .title(&self.title)
            .message(&self.message)
            .tags(self.tags.iter().copied())
            .priority(self.priority)
    }
}

pub type Recorded = Arc<Mutex<Vec<Notification>>>;

/// Where the notifications go
pub enum Notifier {
    /// Nowhere
    Silent,
    /// To a topic of a ntfy server
    Ntfy {
        dispatcher: Dispatcher<Async>,
        topic: String,
    },
    /// In memory, for the tests
    Recording(Recorded),
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // the topic is a secret: whoever knows it reads the notifications
        match self {
            Notifier::Silent => formatter.write_str("Silent"),
            Notifier::Ntfy { .. } => formatter.write_str("Ntfy(***)"),
            Notifier::Recording(_) => formatter.write_str("Recording"),
        }
    }
}

impl Notifier {
    /// `server`: "https://ntfy.sh" or a self-hosted server; `token`: access token of a
    /// protected topic
    pub fn ntfy(server: &str, topic: &str, token: Option<&str>) -> Result<Self, String> {
        let topic = topic.trim();
        if topic.is_empty()
            || !topic
                .chars()
                .all(|it| it.is_ascii_alphanumeric() || it == '-' || it == '_')
        {
            return Err("the ntfy topic must be made of letters, digits, '-' and '_'".into());
        }
        let mut builder = ntfy::dispatcher::builder(server.trim().trim_end_matches('/'));
        if let Some(token) = token.map(str::trim).filter(|it| !it.is_empty()) {
            builder = builder.credentials(Auth::token(token));
        }
        let client = reqwest::ClientBuilder::new()
            .timeout(Duration::from_secs(10))
            .user_agent("trading-notify");
        let dispatcher = builder
            .build_async_with_client(client)
            .map_err(|error| error.to_string())?;
        Ok(Notifier::Ntfy {
            dispatcher,
            topic: topic.to_string(),
        })
    }

    pub fn recording() -> (Self, Recorded) {
        let recorded = Recorded::default();
        (Notifier::Recording(recorded.clone()), recorded)
    }

    /// Sends the notification; a failure is logged, never returned: the caller goes on
    pub async fn send(&self, notification: Notification) {
        match self {
            Notifier::Silent => {}
            Notifier::Recording(recorded) => recorded.lock().unwrap().push(notification),
            Notifier::Ntfy { dispatcher, topic } => {
                if let Err(error) = dispatcher.send(&notification.payload(topic)).await {
                    tracing::warn!(%error, title = %notification.title, "the notification couldn't be sent");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification() -> Notification {
        Notification {
            event: EventKind::Filled,
            title: "Sell filled · #1 XBTUSD".into(),
            message: "gain +1.5 USD".into(),
            tags: vec!["moneybag"],
            priority: Priority::High,
        }
    }

    #[test]
    fn builds_the_payload_of_ntfy() {
        let payload = serde_json::to_value(notification().payload("kraken-bot")).unwrap();
        assert_eq!(payload["topic"], "kraken-bot");
        assert_eq!(payload["title"], "Sell filled · #1 XBTUSD");
        assert_eq!(payload["message"], "gain +1.5 USD");
        assert_eq!(payload["tags"], serde_json::json!(["moneybag"]));
        assert_eq!(payload["priority"], 4);
    }

    #[test]
    fn refuses_the_invalid_topics_and_hides_the_valid_ones() {
        for topic in ["", " ", "a/b", "a b", "é"] {
            assert!(
                Notifier::ntfy("https://ntfy.sh", topic, None).is_err(),
                "{topic:?}"
            );
        }
        assert!(Notifier::ntfy("not a url", "topic", None).is_err());
        let notifier =
            Notifier::ntfy("https://ntfy.sh/", "the-secret-topic", Some("the-token")).unwrap();
        assert_eq!(format!("{notifier:?}"), "Ntfy(***)");
    }

    #[tokio::test]
    async fn records_or_drops_the_notifications() {
        let (notifier, recorded) = Notifier::recording();
        notifier.send(notification()).await;
        Notifier::Silent.send(notification()).await;
        assert_eq!(*recorded.lock().unwrap(), [notification()]);
    }
}
