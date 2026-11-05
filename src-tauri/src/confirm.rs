//! Confirmation outside the webview. Launching a program or a project, and clearing the activity
//! log, are confirmed in a native Windows window that Rust opens, with text Rust writes. A
//! compromised page cannot click it, because it is not part of the page. The default button is
//! Cancel. The window sits behind this trait so tests need no real window.

use std::future::Future;
use std::pin::Pin;

use crate::text;

/// What the native window shows. Every field is cleaned of control, bidirectional and invisible
/// characters here, whatever it was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub title: String,
    pub body: String,
}

const MAX_BODY_CHARS: usize = 1200;

impl Prompt {
    /// `lines` come from the app and the registry. `allow_label` says what OK does.
    pub fn new(title: &str, lines: &[String], allow_label: &str) -> Self {
        let mut body = lines
            .iter()
            .map(|l| text::multi_line(l))
            .collect::<Vec<_>>()
            .join("\n");
        if body.chars().count() > MAX_BODY_CHARS {
            body = body.chars().take(MAX_BODY_CHARS).collect::<String>() + "\u{2026}";
        }
        body.push_str(&format!(
            "\n\nChoose OK to {allow_label}. Cancel (the default) refuses. Moby's chat window cannot answer this for you."
        ));
        Self {
            title: text::short_line(title, 80),
            body,
        }
    }
}

pub type Pending<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

pub trait Confirmer: Send + Sync {
    /// Resolves to true only when the person explicitly allowed it. Anything else (Cancel, the
    /// close button, Escape, an error, no window available) is false. If the returned future is
    /// dropped before it resolves, the window must be closed.
    fn confirm(&self, prompt: Prompt) -> Pending<'_>;
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    type Hook = Box<dyn Fn(usize) + Send + Sync>;

    /// Answers from a script and records every prompt it was shown. `before` runs, with the
    /// prompt's number, while the "window" is open and before the answer is given.
    pub struct Scripted {
        pub answers: Mutex<Vec<bool>>,
        pub shown: Mutex<Vec<Prompt>>,
        pub delay: Duration,
        pub before: Option<Hook>,
        /// Runs after the delay, just before the answer is returned: the window is already
        /// closed, so only the checks after the answer can still stop the call.
        pub after: Option<Hook>,
    }

    impl Scripted {
        pub fn new(answers: &[bool]) -> Self {
            Self {
                answers: Mutex::new(answers.iter().rev().copied().collect()),
                shown: Mutex::new(Vec::new()),
                delay: Duration::ZERO,
                before: None,
                after: None,
            }
        }

        pub fn shown(&self) -> Vec<Prompt> {
            self.shown.lock().unwrap().clone()
        }
    }

    impl Confirmer for Scripted {
        fn confirm(&self, prompt: Prompt) -> Pending<'_> {
            let number = {
                let mut shown = self.shown.lock().unwrap();
                shown.push(prompt);
                shown.len()
            };
            Box::pin(async move {
                if let Some(hook) = &self.before {
                    hook(number);
                }
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                if let Some(hook) = &self.after {
                    hook(number);
                }
                self.answers.lock().unwrap().pop().unwrap_or(false)
            })
        }
    }

    #[tokio::test]
    async fn prompts_are_cleaned_bounded_and_default_to_refusal() {
        let prompt = Prompt::new(
            "Moby\u{202E} asks",
            &[
                "Open \u{202E}evil\u{200B}name\u{0}".to_owned(),
                "x".repeat(5000),
            ],
            "open it",
        );
        assert!(!prompt.title.contains('\u{202E}'));
        assert!(
            !prompt
                .body
                .chars()
                .any(|c| crate::text::is_unsafe_char(c) && c != '\n')
        );
        assert!(prompt.body.contains("Open evilname"));
        assert!(prompt.body.chars().count() < MAX_BODY_CHARS + 200);
        assert!(prompt.body.contains("Cancel (the default) refuses"));
        assert!(!Scripted::new(&[]).confirm(prompt).await);
        assert!(!Scripted::new(&[]).confirm(Prompt::new("t", &[], "x")).await);
    }
}
