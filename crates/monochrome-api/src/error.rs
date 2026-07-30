use std::fmt;

#[derive(Debug)]
pub enum ApiError {
    Network(String),
    Status { code: u16, message: String },
    Decode(String),
    Unauthorized,
    NoInstances,
    AllInstancesFailed(Vec<String>),
    TurnstileRequired,
    CredentialRejected,
    NotFound,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Network(detail) => write!(f, "network error: {detail}"),
            ApiError::Status { code, message } if message.is_empty() => {
                write!(f, "server returned {code}")
            }
            ApiError::Status { code, message } => write!(f, "server returned {code}: {message}"),
            ApiError::Decode(detail) => write!(f, "unexpected response: {detail}"),
            ApiError::Unauthorized => write!(f, "session expired, sign in again"),
            ApiError::NoInstances => write!(f, "no catalog instances configured"),
            ApiError::AllInstancesFailed(reasons) => {
                write!(f, "every catalog instance failed")?;
                if let Some(first) = reasons.first() {
                    write!(f, " ({first})")?;
                }
                Ok(())
            }
            ApiError::TurnstileRequired => write!(f, "amazon gateway needs verification"),
            ApiError::CredentialRejected => {
                write!(f, "the amazon token was rejected, it is wrong or expired")
            }
            ApiError::NotFound => write!(f, "not found"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_decode() {
            ApiError::Decode(error.to_string())
        } else {
            ApiError::Network(error.to_string())
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
