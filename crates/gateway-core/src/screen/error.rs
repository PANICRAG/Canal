//! Error bridging between canal-cv and gateway-core.

use crate::error::Error;
use canal_cv::ComputerUseError;

/// Convert a `ComputerUseError` into a gateway-core `Error`.
impl From<ComputerUseError> for Error {
    fn from(e: ComputerUseError) -> Self {
        match e {
            ComputerUseError::CaptureFailed(msg) => {
                Error::Internal(format!("Screen capture failed: {}", msg))
            }
            ComputerUseError::InputFailed(msg) => {
                Error::Internal(format!("Screen input failed: {}", msg))
            }
            ComputerUseError::PermissionDenied(msg) => Error::PermissionDenied(msg),
            ComputerUseError::NotConnected => {
                Error::Internal("Not connected to any screen surface".to_string())
            }
            ComputerUseError::Timeout(duration) => {
                Error::Timeout(format!("Screen operation timed out after {:?}", duration))
            }
            ComputerUseError::Other(e) => Error::Internal(format!("Screen error: {}", e)),
        }
    }
}
