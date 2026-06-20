use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("daemon unavailable: {0}")]
    DaemonUnavailable(String),
    #[error("daemon error: {0}")]
    Daemon(String),
}

impl Serialize for CommandError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl From<arctis_client::ClientError> for CommandError {
    fn from(e: arctis_client::ClientError) -> Self {
        CommandError::DaemonUnavailable(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_unavailable_formats_correctly() {
        let e = CommandError::DaemonUnavailable("connection refused".into());
        assert_eq!(e.to_string(), "daemon unavailable: connection refused");
    }

    #[test]
    fn daemon_error_formats_correctly() {
        let e = CommandError::Daemon("profile not found".into());
        assert_eq!(e.to_string(), "daemon error: profile not found");
    }

    #[test]
    fn command_error_serializes_to_string() {
        let e = CommandError::Daemon("oops".into());
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#""daemon error: oops""#);
    }
}
