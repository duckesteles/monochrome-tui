use crate::error::{ApiError, ApiResult};
use monochrome_core::library::{SyncDocument, SyncField};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::time::Duration;

pub const DEFAULT_AUTH_URL: &str = "https://auth.monochrome.tf";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl User {
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.email.as_deref())
            .unwrap_or(&self.id)
    }
}

#[derive(Debug, Deserialize)]
struct SignInResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    user: Option<User>,
}

#[derive(Debug, Deserialize)]
struct SessionResponse {
    #[serde(default)]
    user: Option<User>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub struct AuthClient {
    client: reqwest::Client,
    base: String,
}

impl AuthClient {
    pub fn new(base: impl Into<String>) -> ApiResult<Self> {
        let base = base.into().trim_end_matches('/').to_string();
        if !crate::is_transport_allowed(&base) {
            return Err(ApiError::Network(
                "the auth server must be reached over https".into(),
            ));
        }
        crate::use_ring_for_tls();
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(crate::USER_AGENT)
            .build()?;
        Ok(Self { client, base })
    }

    pub fn with_default_url() -> ApiResult<Self> {
        Self::new(DEFAULT_AUTH_URL)
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub async fn sign_in(&self, email: &str, password: &str) -> ApiResult<(String, User)> {
        let response = self
            .client
            .post(format!("{}/api/auth/sign-in/email", self.base))
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(status_error(status.as_u16(), &body));
        }

        let parsed: SignInResponse =
            serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))?;
        let token = parsed
            .token
            .ok_or_else(|| ApiError::Decode("the auth server returned no token".into()))?;
        let user = parsed
            .user
            .ok_or_else(|| ApiError::Decode("the auth server returned no user".into()))?;
        Ok((token, user))
    }

    pub async fn me(&self, token: &str) -> ApiResult<User> {
        let response = self
            .client
            .get(format!("{}/api/me", self.base))
            .bearer_auth(token)
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(status_error(status.as_u16(), &body));
        }

        let parsed: SessionResponse =
            serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))?;
        parsed
            .user
            .ok_or_else(|| ApiError::Decode("the session carried no user".into()))
    }

    pub async fn sign_out(&self, token: &str) -> ApiResult<()> {
        let _ = self
            .client
            .post(format!("{}/api/auth/sign-out", self.base))
            .bearer_auth(token)
            .send()
            .await;
        Ok(())
    }

    pub async fn load_sync(&self, token: &str) -> ApiResult<SyncDocument> {
        let response = self
            .client
            .get(format!("{}/api/sync", self.base))
            .bearer_auth(token)
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(status_error(status.as_u16(), &body));
        }
        serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))
    }

    pub async fn push_sync(
        &self,
        token: &str,
        changes: &[(SyncField, Value)],
    ) -> ApiResult<SyncDocument> {
        let mut payload = Map::new();
        for (field, value) in changes {
            payload.insert(field.wire_name().to_string(), value.clone());
        }

        let response = self
            .client
            .patch(format!("{}/api/sync", self.base))
            .bearer_auth(token)
            .json(&Value::Object(payload))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(status_error(status.as_u16(), &body));
        }
        serde_json::from_str(&body).map_err(|error| ApiError::Decode(error.to_string()))
    }
}

fn status_error(code: u16, body: &str) -> ApiError {
    if code == 401 {
        return ApiError::Unauthorized;
    }
    let message = serde_json::from_str::<ErrorBody>(body)
        .ok()
        .and_then(|parsed| parsed.message.or(parsed.error))
        .unwrap_or_default();
    ApiError::Status { code, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_auth_servers_are_rejected() {
        assert!(AuthClient::new("http://auth.example").is_err());
    }

    #[test]
    fn trailing_slashes_are_trimmed() {
        let client = AuthClient::new("https://auth.example/").expect("client");
        assert_eq!(client.base_url(), "https://auth.example");
    }

    #[test]
    fn a_401_always_maps_to_unauthorized() {
        assert!(matches!(status_error(401, "{}"), ApiError::Unauthorized));
    }

    #[test]
    fn error_messages_are_extracted_from_the_body() {
        let error = status_error(400, r#"{"message":"Invalid email or password"}"#);
        assert_eq!(
            error.to_string(),
            "server returned 400: Invalid email or password"
        );
    }

    #[test]
    fn a_body_without_a_message_still_reports_the_code() {
        assert_eq!(status_error(500, "boom").to_string(), "server returned 500");
    }

    #[test]
    fn display_name_prefers_name_then_email() {
        let named = User {
            id: "u1".into(),
            email: Some("a@b.co".into()),
            name: Some("Ada".into()),
        };
        assert_eq!(named.display_name(), "Ada");
        let anonymous = User {
            id: "u1".into(),
            email: Some("a@b.co".into()),
            name: None,
        };
        assert_eq!(anonymous.display_name(), "a@b.co");
    }
}
