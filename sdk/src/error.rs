use std::fmt;

#[derive(Debug)]
pub enum ManifestSdkError {
    Rpc(String),
    Parse(String),
    Oracle(String),
}

impl fmt::Display for ManifestSdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestSdkError::Rpc(msg) => write!(f, "RPC error: {msg}"),
            ManifestSdkError::Parse(msg) => write!(f, "Parse error: {msg}"),
            ManifestSdkError::Oracle(msg) => write!(f, "Oracle error: {msg}"),
        }
    }
}

impl std::error::Error for ManifestSdkError {}

impl From<solana_client::client_error::ClientError> for ManifestSdkError {
    fn from(e: solana_client::client_error::ClientError) -> Self {
        ManifestSdkError::Rpc(e.to_string())
    }
}
