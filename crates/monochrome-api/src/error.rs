use std::fmt;

#[derive(Debug)]
pub enum ApiError {
    Network(String),
    Status { code: u16, message: String },
    Decode(String),
    Unauthorized,
    NoInstances,
    NoSourceEnabled,
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
            ApiError::NoSourceEnabled => write!(f, "every playback source is switched off"),
            ApiError::AllInstancesFailed(reasons) => {
                write!(f, "every catalog instance failed")?;
                if let Some(first) = reasons.first() {
                    write!(f, " ({first})")?;
                }
                Ok(())
            }
            ApiError::TurnstileRequired => write!(f, "a browser check is needed before playing"),
            ApiError::CredentialRejected => {
                write!(
                    f,
                    "the stored credential was refused, it is wrong or expired"
                )
            }
            ApiError::NotFound => write!(f, "not found"),
        }
    }
}

impl ApiError {
    pub fn is_temporary(&self) -> bool {
        match self {
            ApiError::Network(_) => true,
            ApiError::Status { code, .. } => *code >= 500 || *code == 429,
            _ => false,
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        let detail = describe_chain(&error);
        if error.is_decode() {
            ApiError::Decode(detail)
        } else {
            ApiError::Network(detail)
        }
    }
}

fn describe_chain(error: &dyn std::error::Error) -> String {
    let mut detail = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !detail.contains(&text) {
            detail.push_str(": ");
            detail.push_str(&text);
        }
        source = cause.source();
    }
    detail
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16) -> ApiError {
        ApiError::Status {
            code,
            message: String::new(),
        }
    }

    #[test]
    fn a_service_that_is_briefly_unwell_is_worth_waiting_for() {
        for code in [500, 502, 503, 504, 530, 429] {
            assert!(
                status(code).is_temporary(),
                "{code} says come back later, not give up"
            );
        }
        assert!(ApiError::Network("connection reset".into()).is_temporary());
    }

    #[test]
    fn a_refusal_the_client_caused_will_not_get_better_on_its_own() {
        for code in [400, 403, 404, 422] {
            assert!(!status(code).is_temporary(), "{code} is our own mistake");
        }
        for error in [
            ApiError::Unauthorized,
            ApiError::NotFound,
            ApiError::NoSourceEnabled,
            ApiError::Decode("bad json".into()),
        ] {
            assert!(!error.is_temporary(), "{error} will not fix itself");
        }
    }
}
