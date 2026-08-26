use std::future::Future;
use std::time::Duration;

pub const PROPERTY_TIMEOUT: Duration = Duration::from_secs(2);
pub const LAYOUT_TIMEOUT: Duration = Duration::from_secs(5);
pub const ACTION_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub enum CallError {
    Timeout {
        label: &'static str,
        after: Duration,
    },
    Dbus {
        label: &'static str,
        source: zbus::Error,
    },
}

impl CallError {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Timeout { label, .. } | Self::Dbus { label, .. } => label,
        }
    }

    pub fn is_transient(&self) -> bool {
        match self {
            Self::Timeout { .. } => true,
            Self::Dbus { source, .. } => !matches!(
                source,
                zbus::Error::MethodError(..) | zbus::Error::InterfaceNotFound
            ),
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { label, after } => {
                write!(f, "{label} timed out after {}ms", after.as_millis())
            }
            Self::Dbus { label, source } => write!(f, "{label} failed: {source}"),
        }
    }
}

impl std::error::Error for CallError {}

pub async fn with_timeout<T, E: Into<zbus::Error>>(
    timeout: Duration,
    label: &'static str,
    fut: impl Future<Output = Result<T, E>>,
) -> Result<T, CallError> {
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(source)) => Err(CallError::Dbus {
            label,
            source: source.into(),
        }),
        Err(_) => Err(CallError::Timeout {
            label,
            after: timeout,
        }),
    }
}

pub async fn optional<T: Default, E: Into<zbus::Error>>(
    label: &'static str,
    fut: impl Future<Output = Result<T, E>>,
) -> T {
    match with_timeout(PROPERTY_TIMEOUT, label, fut).await {
        Ok(value) => value,
        Err(err) => {
            tracing::debug!(error = %err, "optional property unavailable");
            T::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct Backoff {
    attempts: u32,
    state: u64,
}

impl Backoff {
    pub const BASE: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_mins(1);

    pub fn new(seed: u64) -> Self {
        Self {
            attempts: 0,
            state: seed | 1,
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    pub fn next_delay(&mut self) -> Duration {
        let step = Self::BASE
            .saturating_mul(1u32 << self.attempts.min(6))
            .min(Self::MAX);
        self.attempts = self.attempts.saturating_add(1);

        let jitter = self.next_random() % 25;
        let millis = u64::try_from(step.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(millis - millis * jitter / 100)
    }

    fn next_random(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timeout_is_reported_as_timeout() {
        let err = with_timeout(Duration::from_millis(10), "test", async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<(), zbus::Error>(())
        })
        .await
        .unwrap_err();

        assert!(matches!(err, CallError::Timeout { .. }));
        assert!(err.is_transient());
    }

    #[tokio::test]
    async fn optional_falls_back_to_default() {
        let value: String = optional("test", async { Err(zbus::Error::InterfaceNotFound) }).await;
        assert_eq!(value, "");
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let mut backoff = Backoff::new(7);
        let mut previous = Duration::ZERO;
        for _ in 0..12 {
            let delay = backoff.next_delay();
            assert!(delay <= Backoff::MAX);
            assert!(delay >= previous.mul_f32(0.75) || delay == Backoff::MAX.mul_f32(0.75));
            previous = delay;
        }
        assert_eq!(backoff.attempts(), 12);
        backoff.reset();
        assert_eq!(backoff.attempts(), 0);
    }

    #[test]
    fn method_errors_are_not_retried() {
        let err = CallError::Dbus {
            label: "x",
            source: zbus::Error::InterfaceNotFound,
        };
        assert!(!err.is_transient());
    }
}
