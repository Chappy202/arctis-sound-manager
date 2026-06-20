use thiserror::Error;

/// Top-level error type for the engine orchestrator.
///
/// Wraps errors from sub-crates; device errors are stringified because
/// device operations are best-effort and the device error type is not `Send`.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Config(#[from] arctis_config::ConfigError),
    #[error(transparent)]
    Audio(#[from] arctis_audio::AudioError),
    /// Device transport errors (stringified — device is best-effort).
    #[error("device: {0}")]
    Device(String),
    #[error("reconcile failed: {0}")]
    Reconcile(String),
    #[error("ipc error: {0}")]
    Ipc(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_error_displays_audio_variant() {
        let ae = arctis_audio::AudioError::Invalid("test".into());
        let ee = EngineError::Audio(ae);
        assert!(ee.to_string().contains("test"));
    }

    #[test]
    fn engine_error_displays_config_variant() {
        let ce = arctis_config::ConfigError::ProfileNotFound("missing".into());
        let ee = EngineError::Config(ce);
        assert!(ee.to_string().contains("missing"));
    }

    #[test]
    fn engine_error_from_audio_error() {
        let ae = arctis_audio::AudioError::Invalid("from_audio".into());
        let ee: EngineError = ae.into();
        assert!(matches!(ee, EngineError::Audio(_)));
    }

    #[test]
    fn engine_error_from_config_error() {
        let ce = arctis_config::ConfigError::Invalid("from_config".into());
        let ee: EngineError = ce.into();
        assert!(matches!(ee, EngineError::Config(_)));
    }
}
