use std::path::PathBuf;

pub struct DaemonState {
    pub socket: PathBuf,
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            socket: arctis_client::socket_path(),
        }
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new()
    }
}
