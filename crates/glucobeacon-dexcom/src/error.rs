//! What can go wrong talking to Share, and what to do about it.

use crate::model::ShareFault;

/// A `Result` from this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// A failure fetching data from Share.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request never completed: DNS, TCP, TLS, or timeout.
    #[error("could not reach the Dexcom Share service")]
    Transport(#[source] reqwest::Error),

    /// Share rejected the credentials. Retrying will not help.
    #[error("Dexcom rejected the credentials ({code})")]
    Credentials {
        /// Share's error code.
        code: String,
        /// Share's message, when it supplied one.
        message: String,
    },

    /// The session ID expired. The client re-logs in and retries once on its
    /// own; seeing this means that retry also failed.
    #[error("the Dexcom session expired and could not be renewed ({code})")]
    SessionExpired {
        /// Share's error code.
        code: String,
    },

    /// Share answered, but with a fault.
    #[error("Dexcom Share returned an error ({code}): {message}")]
    Service {
        /// Share's error code.
        code: String,
        /// Share's message.
        message: String,
    },

    /// Share answered with something this client could not make sense of.
    #[error("unexpected response from Dexcom Share: {0}")]
    Protocol(String),
}

impl Error {
    /// Builds the right variant for a fault body.
    pub(crate) fn from_fault(fault: ShareFault) -> Self {
        let code = fault.code().to_owned();
        let message = fault.message.clone().unwrap_or_default();
        if fault.is_credential_failure() {
            Self::Credentials { code, message }
        } else if fault.is_stale_session() {
            Self::SessionExpired { code }
        } else {
            Self::Service { code, message }
        }
    }

    /// How the caller should react.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Transport(_) => ErrorKind::Network,
            Self::Credentials { .. } => ErrorKind::Credentials,
            Self::SessionExpired { .. } | Self::Service { .. } => ErrorKind::Service,
            Self::Protocol(_) => ErrorKind::Protocol,
        }
    }

    /// Whether backing off and trying again could plausibly succeed.
    ///
    /// Bad credentials are the one case where it cannot: the gateway should
    /// stop hammering Share and say so on the display instead.
    pub fn is_retryable(&self) -> bool {
        self.kind() != ErrorKind::Credentials
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error)
    }
}

/// The category of a [`Error`], for deciding what to do next.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum ErrorKind {
    /// The network is the problem.
    Network,
    /// The account name or password is wrong. Needs a human.
    Credentials,
    /// Share itself is unhappy.
    Service,
    /// Share said something unexpected. Probably an API change.
    Protocol,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fault(code: &str) -> ShareFault {
        serde_json::from_str(&format!(r#"{{"Code":"{code}","Message":"nope"}}"#)).expect("parse")
    }

    #[test]
    fn bad_credentials_are_not_retryable() {
        let error = Error::from_fault(fault("SSO_AuthenticatePasswordInvalid"));
        assert_eq!(error.kind(), ErrorKind::Credentials);
        assert!(!error.is_retryable());
    }

    #[test]
    fn an_expired_session_is_retryable() {
        let error = Error::from_fault(fault("SessionIdNotFound"));
        assert_eq!(error.kind(), ErrorKind::Service);
        assert!(error.is_retryable());
        assert!(matches!(error, Error::SessionExpired { .. }));
    }

    #[test]
    fn unrecognized_faults_become_service_errors() {
        let error = Error::from_fault(fault("MonitoredReceiverNotAssigned"));
        assert!(matches!(error, Error::Service { .. }));
        assert!(error.is_retryable());
    }

    #[test]
    fn messages_name_the_code() {
        let error = Error::from_fault(fault("SSO_AuthenticateMaxAttemptsExceeded"));
        assert!(
            error
                .to_string()
                .contains("SSO_AuthenticateMaxAttemptsExceeded"),
            "{error}"
        );
    }
}
