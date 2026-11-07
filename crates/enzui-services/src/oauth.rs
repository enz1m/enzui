//! OAuth authorization-code requests with S256 PKCE for native applications.
//!
//! Build a pending request with [`ClientConfig::begin`], open its authorization
//! URL through the system browser, and validate the callback with
//! [`PendingAuthorization::complete`]. [`TokenRequest`] supplies the encoded
//! token body; the caller owns HTTP transport and token storage. On Linux,
//! [`loopback::LoopbackRedirect`] can receive a browser redirect off the UI thread.
//!
//! Verifiers, codes, token bodies, and token fields use [`SecretText`], whose
//! owned allocation is wiped after its last handle drops. Authorization URLs
//! contain state and optional identifying parameters in an ordinary String;
//! callback and token parsing also create ordinary string copies. Redacted
//! Debug output does not imply that every copy is wiped. Provider error fields
//! and configuration endpoints are printable.
//!
//! Callback validation does not consume the pending request or enforce local
//! replay protection. Dispose of it after processing the sign-in outcome.

use std::fmt;

use enzui_security::random::{RandomError, fill_random};
pub use enzui_security::secret::SecretText;
use sha2::{Digest, Sha256};

use enzui_security::redirect_capture::{RedirectCapture, RedirectCaptureError, RedirectKind};

#[cfg(target_os = "linux")]
pub mod loopback;

#[cfg(test)]
mod tests;

/// The PKCE challenge method this module sends. `plain` is never offered:
/// it gives no protection when the authorization request is observed.
pub const CODE_CHALLENGE_METHOD: &str = "S256";

/// The longest token-endpoint reply [`TokenResponse::parse`] reads.
pub const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

/// The longest callback URL [`PendingAuthorization::complete`] reads.
pub const MAX_CALLBACK_BYTES: usize = 8 * 1024;

/// Why a configuration or request was refused before anything was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OAuthError {
    /// An endpoint is not `https://`, or is plain `http://` to a host that is
    /// not loopback.
    InsecureEndpoint,
    /// An endpoint could not be read as an absolute URL, or carries a
    /// fragment, which RFC 6749 §3.1 forbids.
    MalformedUrl,
    /// The redirect URI is not one a native app may use (RFC 8252 §7).
    Redirect(RedirectCaptureError),
    /// A client id, scope or extra parameter is empty, or a scope contains a
    /// space or a byte outside RFC 6749 §3.3's set.
    InvalidParameter,
    /// An extra parameter tried to set one this module owns (`state`,
    /// `code_challenge`, `redirect_uri` and so on).
    ReservedParameter(String),
    /// A supplied PKCE verifier is not 43 to 128 unreserved characters.
    InvalidVerifier,
    /// The kernel random source refused to produce the verifier or `state`;
    /// carries the [`RandomError`] text.
    Random(String),
}

impl fmt::Display for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsecureEndpoint => f.write_str("OAuth endpoint is not HTTPS"),
            Self::MalformedUrl => {
                f.write_str("OAuth URL is not an absolute URL without a fragment")
            }
            Self::Redirect(error) => write!(f, "OAuth {error}"),
            Self::InvalidParameter => f.write_str("OAuth parameter is empty or malformed"),
            Self::ReservedParameter(name) => {
                write!(f, "OAuth parameter `{name}` is set by the flow itself")
            }
            Self::InvalidVerifier => {
                f.write_str("PKCE verifier is not 43-128 unreserved characters")
            }
            Self::Random(error) => write!(f, "no entropy for the OAuth request: {error}"),
        }
    }
}

impl std::error::Error for OAuthError {}

impl From<RandomError> for OAuthError {
    fn from(error: RandomError) -> Self {
        Self::Random(error.to_string())
    }
}

/// A PKCE verifier and its `S256` challenge (RFC 7636).
pub struct Pkce {
    verifier: SecretText,
    challenge: String,
}

impl Pkce {
    /// Generate a 43-character verifier from 32 random bytes.
    ///
    /// # Errors
    /// Returns [`OAuthError::Random`] if the random source fails.
    pub fn generate() -> Result<Self, OAuthError> {
        let mut bytes = [0u8; 32];
        fill_random(&mut bytes)?;
        let verifier = base64url(&bytes);
        enzui_security::secret::wipe_bytes(&mut bytes);
        Ok(Self::from_valid(SecretText::new(verifier)))
    }

    /// Restore a persisted verifier after checking its syntax.
    ///
    /// # Errors
    /// Returns [`OAuthError::InvalidVerifier`] unless it contains 43 through
    /// 128 ASCII unreserved characters.
    pub fn from_verifier(verifier: SecretText) -> Result<Self, OAuthError> {
        let text = verifier.expose_secret();
        let valid = (43..=128).contains(&text.len()) && text.bytes().all(is_unreserved);
        if !valid {
            return Err(OAuthError::InvalidVerifier);
        }
        Ok(Self::from_valid(verifier))
    }

    fn from_valid(verifier: SecretText) -> Self {
        let challenge = base64url(&Sha256::digest(verifier.expose_secret().as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }

    /// The `code_verifier` sent to the token endpoint.
    #[must_use]
    pub fn verifier(&self) -> &SecretText {
        &self.verifier
    }

    /// The `code_challenge` sent in the authorization URL.
    #[must_use]
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &self.verifier)
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// A registered OAuth client: where to send the person and where to trade the
/// code.
#[derive(Clone)]
pub struct ClientConfig {
    authorization_endpoint: String,
    token_endpoint: String,
    client_id: String,
    client_secret: Option<SecretText>,
    scopes: Vec<String>,
    issuer: Option<String>,
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("authorization_endpoint", &self.authorization_endpoint)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("scopes", &self.scopes)
            .field("issuer", &self.issuer)
            .finish()
    }
}

impl ClientConfig {
    /// A public client (no secret). Both endpoints must be HTTPS; plain HTTP
    /// is accepted only to a loopback host.
    ///
    /// Existing endpoint query parameters are retained without checking
    /// for reserved names.
    ///
    /// # Errors
    /// Refuses unsupported or malformed endpoint targets and an empty client
    /// id or one containing ASCII control bytes.
    pub fn new(
        authorization_endpoint: impl Into<String>,
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Result<Self, OAuthError> {
        let authorization_endpoint = authorization_endpoint.into();
        let token_endpoint = token_endpoint.into();
        let client_id = client_id.into();
        check_endpoint(&authorization_endpoint)?;
        check_endpoint(&token_endpoint)?;
        if client_id.is_empty() || client_id.bytes().any(|b| b.is_ascii_control()) {
            return Err(OAuthError::InvalidParameter);
        }
        Ok(Self {
            authorization_endpoint,
            token_endpoint,
            client_id,
            client_secret: None,
            scopes: Vec::new(),
            issuer: None,
        })
    }

    /// Include a client secret in authorization-code and refresh token bodies.
    #[must_use]
    pub fn with_client_secret(mut self, secret: SecretText) -> Self {
        self.client_secret = Some(secret);
        self
    }

    /// The scopes to request, in order. Each must be a non-empty RFC 6749
    /// §3.3 scope token. An empty iterator clears the requested scopes.
    ///
    /// # Errors
    /// Returns [`OAuthError::InvalidParameter`] for an empty or invalid token.
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Result<Self, OAuthError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let scopes: Vec<String> = scopes.into_iter().map(Into::into).collect();
        if scopes.iter().any(|scope| !is_scope_token(scope)) {
            return Err(OAuthError::InvalidParameter);
        }
        self.scopes = scopes;
        Ok(self)
    }

    /// Require an exact `iss` value in callbacks. The configured string is
    /// stored as supplied, without issuer discovery or URL validation.
    #[must_use]
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// The client id.
    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Starts one sign-in: fresh PKCE and `state`, and the URL to open.
    ///
    /// `extra` adds provider parameters such as `login_hint`,
    /// `prompt=consent` or `access_type=offline`. It cannot override what the
    /// flow sets through `extra`; preexisting endpoint query parameters are
    /// retained separately.
    ///
    /// # Errors
    /// Returns [`OAuthError`] for unavailable randomness, a refused redirect,
    /// or invalid extra parameters.
    pub fn begin(
        &self,
        redirect_uri: &str,
        extra: &[(&str, &str)],
    ) -> Result<PendingAuthorization, OAuthError> {
        let pkce = Pkce::generate()?;
        let mut state = [0u8; 24];
        fill_random(&mut state)?;
        let state_text = SecretText::new(base64url(&state));
        enzui_security::secret::wipe_bytes(&mut state);
        self.begin_with(redirect_uri, extra, pkce, state_text)
    }

    /// [`begin`](Self::begin) with a caller-supplied verifier and `state`, for
    /// a flow restored after a restart and for tests. The supplied state must
    /// be nonempty; freshness and entropy remain the caller's responsibility.
    ///
    /// # Errors
    /// Refuses an invalid redirect, empty state, reserved extra names, empty
    /// extra names, or extra values containing ASCII control bytes.
    pub fn begin_with(
        &self,
        redirect_uri: &str,
        extra: &[(&str, &str)],
        pkce: Pkce,
        state: SecretText,
    ) -> Result<PendingAuthorization, OAuthError> {
        let redirect = RedirectCapture::parse(redirect_uri).map_err(OAuthError::Redirect)?;
        if state.is_empty() {
            return Err(OAuthError::InvalidParameter);
        }
        for (name, value) in extra {
            if RESERVED.contains(name) {
                return Err(OAuthError::ReservedParameter((*name).to_owned()));
            }
            if name.is_empty() || value.bytes().any(|b| b.is_ascii_control()) {
                return Err(OAuthError::InvalidParameter);
            }
        }
        let mut url = self.authorization_endpoint.clone();
        url.push(if url.contains('?') { '&' } else { '?' });
        let mut query = FormWriter::new(&mut url);
        query.pair("response_type", "code");
        query.pair("client_id", &self.client_id);
        query.pair("redirect_uri", redirect_uri);
        if !self.scopes.is_empty() {
            query.pair("scope", &self.scopes.join(" "));
        }
        query.pair("state", state.expose_secret());
        query.pair("code_challenge", pkce.challenge());
        query.pair("code_challenge_method", CODE_CHALLENGE_METHOD);
        for (name, value) in extra {
            query.pair(name, value);
        }
        Ok(PendingAuthorization {
            url,
            redirect_uri: redirect_uri.to_owned(),
            redirect,
            state,
            pkce,
            client: self.clone(),
        })
    }

    /// Build a refresh-token form. An empty scope slice omits `scope`;
    /// otherwise the supplied tokens are joined without validation. The
    /// provider determines whether they are permitted by the grant.
    #[must_use]
    pub fn refresh_request(&self, refresh_token: &SecretText, scopes: &[&str]) -> TokenRequest {
        let mut body = String::new();
        let mut form = FormWriter::new(&mut body);
        form.pair("grant_type", "refresh_token");
        form.pair("refresh_token", refresh_token.expose_secret());
        if !scopes.is_empty() {
            form.pair("scope", &scopes.join(" "));
        }
        self.client_fields(&mut form);
        TokenRequest {
            endpoint: self.token_endpoint.clone(),
            body: SecretText::new(body),
        }
    }

    fn client_fields(&self, form: &mut FormWriter<'_>) {
        form.pair("client_id", &self.client_id);
        if let Some(secret) = &self.client_secret {
            form.pair("client_secret", secret.expose_secret());
        }
    }
}

const RESERVED: &[&str] = &[
    "response_type",
    "client_id",
    "redirect_uri",
    "scope",
    "state",
    "code_challenge",
    "code_challenge_method",
    // Callback parsing only reads the query, so extra parameters cannot request another response mode.
    "response_mode",
];

/// One sign-in in flight: what to open and what the callback must match.
pub struct PendingAuthorization {
    url: String,
    redirect_uri: String,
    redirect: RedirectCapture,
    state: SecretText,
    pkce: Pkce,
    client: ClientConfig,
}

impl fmt::Debug for PendingAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base = self.url.split('?').next().unwrap_or_default();
        f.debug_struct("PendingAuthorization")
            .field("authorization_endpoint", &base)
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl PendingAuthorization {
    /// The URL to open in the system browser.
    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.url
    }

    /// The redirect URI this request named.
    #[must_use]
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// The `state` to persist with the verifier when the callback may arrive
    /// in a new process.
    #[must_use]
    pub fn state(&self) -> &SecretText {
        &self.state
    }

    /// The PKCE verifier and challenge.
    #[must_use]
    pub fn pkce(&self) -> &Pkce {
        &self.pkce
    }

    /// Reads the URL the provider redirected to.
    ///
    /// It must be this request's redirect URI (scheme, host, port and path;
    /// a loopback port is compared too), carry each parameter at most once,
    /// carry this request's `state`, and the configured `iss` if any. A
    /// provider `error` is reported as [`CallbackError::Provider`] only once
    /// `state` and any configured issuer matched. Fragments are ignored, and
    /// parameter names are compared after form decoding. The request remains
    /// reusable after validation.
    ///
    /// # Errors
    /// Returns [`CallbackError`] for oversized or malformed callbacks,
    /// duplicate parameters, redirect/state/issuer mismatches, provider errors,
    /// or a missing or empty authorization code.
    pub fn complete(&self, callback: &SecretText) -> Result<AuthorizationCode, CallbackError> {
        let callback = callback.expose_secret();
        if callback.len() > MAX_CALLBACK_BYTES {
            return Err(CallbackError::TooLong);
        }
        // The fragment goes first: a `?` after `#` is part of the fragment,
        // not a query.
        let without_fragment = callback.split('#').next().unwrap_or_default();
        let (target, query) = without_fragment
            .split_once('?')
            .unwrap_or((without_fragment, ""));
        if !self.redirect.matches(target) {
            return Err(CallbackError::WrongRedirect);
        }
        let params = parse_form(query).ok_or(CallbackError::Malformed)?;
        let mut seen: Vec<&str> = Vec::with_capacity(params.len());
        for (name, _) in &params {
            if seen.contains(&name.as_str()) {
                return Err(CallbackError::DuplicateParameter(name.clone()));
            }
            seen.push(name);
        }
        let get = |name: &str| {
            params
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        let Some(state) = get("state") else {
            return Err(CallbackError::StateMismatch);
        };
        if !constant_time_eq(state.as_bytes(), self.state.expose_secret().as_bytes()) {
            return Err(CallbackError::StateMismatch);
        }
        if let Some(expected) = &self.client.issuer
            && get("iss") != Some(expected.as_str())
        {
            return Err(CallbackError::IssuerMismatch);
        }
        if let Some(error) = get("error") {
            return Err(CallbackError::Provider(ProviderError {
                error: error.to_owned(),
                description: get("error_description").map(str::to_owned),
            }));
        }
        match get("code") {
            Some(code) if !code.is_empty() => {
                Ok(AuthorizationCode(SecretText::new(code.to_owned())))
            }
            _ => Err(CallbackError::MissingCode),
        }
    }

    /// Whether a callback belongs to this redirect and state. Success,
    /// provider errors, issuer mismatches, and missing codes all count, so a
    /// listener can finish and the caller can report the validation outcome.
    /// Use with [`loopback::LoopbackRedirect::wait_for`].
    #[must_use]
    pub fn is_callback(&self, callback: &SecretText) -> bool {
        // Only outcomes reached after redirect and state validation end the listener wait.
        matches!(
            self.complete(callback),
            Ok(_)
                | Err(CallbackError::Provider(_)
                    | CallbackError::IssuerMismatch
                    | CallbackError::MissingCode)
        )
    }

    /// The form body that trades `code` for tokens (RFC 6749 §4.1.3 with the
    /// RFC 7636 verifier).
    #[must_use]
    pub fn token_request(&self, code: &AuthorizationCode) -> TokenRequest {
        let mut body = String::new();
        let mut form = FormWriter::new(&mut body);
        form.pair("grant_type", "authorization_code");
        form.pair("code", code.0.expose_secret());
        form.pair("redirect_uri", &self.redirect_uri);
        form.pair("code_verifier", self.pkce.verifier().expose_secret());
        self.client.client_fields(&mut form);
        TokenRequest {
            endpoint: self.client.token_endpoint.clone(),
            body: SecretText::new(body),
        }
    }
}

/// A callback authorization code. This cloneable wrapper does not enforce
/// expiry or single use; exchange it according to the provider's rules.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizationCode(SecretText);

impl AuthorizationCode {
    /// The code itself.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for AuthorizationCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthorizationCode([redacted])")
    }
}

/// Why a callback was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallbackError {
    /// Longer than [`MAX_CALLBACK_BYTES`].
    TooLong,
    /// Not this request's redirect URI.
    WrongRedirect,
    /// The query is not `application/x-www-form-urlencoded` UTF-8.
    Malformed,
    /// A parameter appears twice (RFC 6749 §3.1).
    DuplicateParameter(String),
    /// `state` is missing or is not this request's. Either a stale callback or
    /// a forged one; nothing in it is trusted.
    StateMismatch,
    /// `iss` is missing or names another issuer (RFC 9207).
    IssuerMismatch,
    /// The provider refused (RFC 6749 §4.1.2.1), for example `access_denied`
    /// when the person declined.
    Provider(ProviderError),
    /// No `code` and no `error`.
    MissingCode,
}

impl fmt::Display for CallbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => f.write_str("OAuth callback is too long"),
            Self::WrongRedirect => f.write_str("OAuth callback is not this request's redirect"),
            Self::Malformed => f.write_str("OAuth callback query is malformed"),
            Self::DuplicateParameter(name) => write!(f, "OAuth callback repeats `{name}`"),
            Self::StateMismatch => f.write_str("OAuth callback state does not match"),
            Self::IssuerMismatch => f.write_str("OAuth callback issuer does not match"),
            Self::Provider(error) => write!(f, "OAuth provider refused: {error}"),
            Self::MissingCode => f.write_str("OAuth callback carries no code"),
        }
    }
}

impl std::error::Error for CallbackError {}

/// An error a provider reported, in a callback or a token reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    /// The `error` code, such as `access_denied` or `invalid_grant`.
    pub error: String,
    /// The human-readable `error_description`, if the provider sent one.
    pub description: Option<String>,
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.description {
            Some(description) => write!(f, "{} ({description})", self.error),
            None => f.write_str(&self.error),
        }
    }
}

/// A POST to the token endpoint, as `application/x-www-form-urlencoded`.
pub struct TokenRequest {
    endpoint: String,
    body: SecretText,
}

impl TokenRequest {
    /// The `Content-Type` to send.
    pub const CONTENT_TYPE: &'static str = "application/x-www-form-urlencoded";

    /// The token endpoint URL.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Encoded credentials: authorization-code bodies include a verifier;
    /// refresh bodies include the refresh token. Both may include a client secret.
    #[must_use]
    pub fn body(&self) -> &SecretText {
        &self.body
    }
}

impl fmt::Debug for TokenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenRequest")
            .field("endpoint", &self.endpoint)
            .field("body", &self.body)
            .finish()
    }
}

/// A successful token-endpoint reply (RFC 6749 §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenResponse {
    /// The access token.
    pub access_token: SecretText,
    /// Its type; `Bearer` in practice.
    pub token_type: String,
    /// Seconds until the access token expires, if the provider said.
    pub expires_in: Option<u64>,
    /// A refresh token, if one was issued.
    pub refresh_token: Option<SecretText>,
    /// The returned scope string, without comparison to the requested scope.
    pub scope: Option<String>,
    /// An OpenID Connect ID token, if `openid` was requested. Verifying its
    /// signature is the caller's job; this module only carries it.
    pub id_token: Option<SecretText>,
}

/// Why a token reply was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenError {
    /// Longer than [`MAX_TOKEN_RESPONSE_BYTES`].
    TooLong,
    /// Not a JSON object, or a field has the wrong type.
    Malformed,
    /// An absent, null, or empty access token, or an absent or null token type.
    MissingField(&'static str),
    /// The provider refused (RFC 6749 §5.2), such as `invalid_grant` when a
    /// refresh token was revoked.
    Provider(ProviderError),
    /// A `token_type` other than `Bearer`.
    UnsupportedTokenType(String),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => f.write_str("token reply is too long"),
            Self::Malformed => f.write_str("token reply is not a valid JSON object"),
            Self::MissingField(field) => write!(f, "token reply has no `{field}`"),
            Self::Provider(error) => write!(f, "token endpoint refused: {error}"),
            Self::UnsupportedTokenType(kind) => {
                write!(f, "token type `{kind}` is not a bearer token")
            }
        }
    }
}

impl std::error::Error for TokenError {}

impl TokenResponse {
    /// Reads a token-endpoint reply body, success or error.
    ///
    /// Duplicate top-level keys are refused. Unknown members are parsed and
    /// discarded, with nesting bounded to 16 value levels. `expires_in` accepts
    /// number text parsed directly as u64 or a string parseable as u64. Null optional fields
    /// are absent; only a case-insensitive Bearer token type is supported.
    /// The numeric parser accepts some noncanonical JSON number spellings.
    ///
    /// # Errors
    /// Returns [`TokenError`] for oversized bodies, parse or field failures,
    /// unsupported token types, or provider error replies.
    pub fn parse(body: &str) -> Result<Self, TokenError> {
        if body.len() > MAX_TOKEN_RESPONSE_BYTES {
            return Err(TokenError::TooLong);
        }
        let members = json::object(body).ok_or(TokenError::Malformed)?;
        let mut names: Vec<&str> = Vec::with_capacity(members.len());
        for (name, _) in &members {
            if names.contains(&name.as_str()) {
                return Err(TokenError::Malformed);
            }
            names.push(name);
        }
        let get = |name: &str| members.iter().find(|(key, _)| key == name).map(|(_, v)| v);
        let string = |name: &str| -> Result<Option<String>, TokenError> {
            match get(name) {
                None | Some(json::Value::Null) => Ok(None),
                Some(json::Value::String(value)) => Ok(Some(value.clone())),
                Some(_) => Err(TokenError::Malformed),
            }
        };
        if let Some(error) = string("error")? {
            return Err(TokenError::Provider(ProviderError {
                error,
                description: string("error_description")?,
            }));
        }
        let access_token = string("access_token")?
            .filter(|token| !token.is_empty())
            .ok_or(TokenError::MissingField("access_token"))?;
        let token_type = string("token_type")?.ok_or(TokenError::MissingField("token_type"))?;
        if !token_type.eq_ignore_ascii_case("bearer") {
            return Err(TokenError::UnsupportedTokenType(token_type));
        }
        let expires_in = match get("expires_in") {
            None | Some(json::Value::Null) => None,
            Some(json::Value::Number(number)) => Some(*number),
            Some(json::Value::String(text)) => {
                Some(text.parse().map_err(|_| TokenError::Malformed)?)
            }
            Some(_) => return Err(TokenError::Malformed),
        };
        Ok(Self {
            access_token: SecretText::new(access_token),
            token_type,
            expires_in,
            refresh_token: string("refresh_token")?.map(SecretText::new),
            scope: string("scope")?,
            id_token: string("id_token")?.map(SecretText::new),
        })
    }
}

/// The SASL `XOAUTH2` initial response for IMAP and SMTP (Google and
/// Microsoft), before base64: `user=…\x01auth=Bearer …\x01\x01`. `None`
/// when the user or token holds a control character (a `\x01` would end the
/// field early) or either is empty.
#[must_use]
pub fn xoauth2_initial_response(user: &str, access_token: &SecretText) -> Option<SecretText> {
    let token = access_token.expose_secret();
    let invalid = |text: &str| text.is_empty() || text.bytes().any(|b| b.is_ascii_control());
    if invalid(user) || invalid(token) {
        return None;
    }
    Some(SecretText::new(format!(
        "user={user}\u{1}auth=Bearer {token}\u{1}\u{1}"
    )))
}

fn check_endpoint(url: &str) -> Result<(), OAuthError> {
    if url.contains('#') {
        return Err(OAuthError::MalformedUrl);
    }
    let base = url.split('?').next().unwrap_or_default();
    match RedirectCapture::parse(base).map(|target| target.kind()) {
        Ok(RedirectKind::Https | RedirectKind::Loopback) => Ok(()),
        Ok(_) | Err(RedirectCaptureError::InsecureHttp | RedirectCaptureError::RefusedScheme) => {
            Err(OAuthError::InsecureEndpoint)
        }
        Err(_) => Err(OAuthError::MalformedUrl),
    }
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

// Scope tokens permit !, # through [, and ] through ~.
fn is_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .bytes()
            .all(|b| b == 0x21 || (0x23..=0x5B).contains(&b) || (0x5D..=0x7E).contains(&b))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let symbols = chunk.len() + 1;
        for i in 0..symbols {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

struct FormWriter<'a> {
    out: &'a mut String,
    first: bool,
}

impl<'a> FormWriter<'a> {
    fn new(out: &'a mut String) -> Self {
        let first = out.is_empty() || out.ends_with('?') || out.ends_with('&');
        Self { out, first }
    }

    fn pair(&mut self, name: &str, value: &str) {
        if !self.first {
            self.out.push('&');
        }
        self.first = false;
        percent_encode(self.out, name);
        self.out.push('=');
        percent_encode(self.out, value);
    }
}

fn percent_encode(out: &mut String, text: &str) {
    for b in text.bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit(u32::from(b >> 4), 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit(u32::from(b & 15), 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
        }
    }
}

fn parse_form(query: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        out.push((percent_decode(name)?, percent_decode(value)?));
    }
    Some(out)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

mod json {
    const MAX_DEPTH: usize = 16;

    #[derive(Debug)]
    pub(super) enum Value {
        String(String),
        Number(u64),
        Null,
        Other,
    }

    pub(super) fn object(text: &str) -> Option<Vec<(String, Value)>> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            at: 0,
        };
        parser.ws();
        let members = parser.members(0)?;
        parser.ws();
        (parser.at == parser.bytes.len()).then_some(members)
    }

    struct Parser<'a> {
        bytes: &'a [u8],
        at: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<u8> {
            self.bytes.get(self.at).copied()
        }

        fn eat(&mut self, b: u8) -> Option<()> {
            (self.peek()? == b).then(|| self.at += 1)
        }

        fn ws(&mut self) {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                self.at += 1;
            }
        }

        fn members(&mut self, depth: usize) -> Option<Vec<(String, Value)>> {
            self.eat(b'{')?;
            let mut out = Vec::new();
            self.ws();
            if self.eat(b'}').is_some() {
                return Some(out);
            }
            loop {
                self.ws();
                let name = self.string()?;
                self.ws();
                self.eat(b':')?;
                self.ws();
                let value = self.value(depth)?;
                out.push((name, value));
                self.ws();
                if self.eat(b',').is_some() {
                    continue;
                }
                self.eat(b'}')?;
                return Some(out);
            }
        }

        fn value(&mut self, depth: usize) -> Option<Value> {
            if depth >= MAX_DEPTH {
                return None;
            }
            match self.peek()? {
                b'"' => self.string().map(Value::String),
                b'{' => self.members(depth + 1).map(|_| Value::Other),
                b'[' => {
                    self.at += 1;
                    self.ws();
                    if self.eat(b']').is_some() {
                        return Some(Value::Other);
                    }
                    loop {
                        self.ws();
                        self.value(depth + 1)?;
                        self.ws();
                        if self.eat(b',').is_some() {
                            continue;
                        }
                        self.eat(b']')?;
                        return Some(Value::Other);
                    }
                }
                b't' => self.literal(b"true").map(|()| Value::Other),
                b'f' => self.literal(b"false").map(|()| Value::Other),
                b'n' => self.literal(b"null").map(|()| Value::Null),
                b'-' | b'0'..=b'9' => self.number(),
                _ => None,
            }
        }

        fn literal(&mut self, word: &[u8]) -> Option<()> {
            let end = self.at.checked_add(word.len())?;
            (self.bytes.get(self.at..end)? == word).then(|| self.at = end)
        }

        fn number(&mut self) -> Option<Value> {
            let start = self.at;
            while matches!(
                self.peek(),
                Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
            ) {
                self.at += 1;
            }
            let text = std::str::from_utf8(&self.bytes[start..self.at]).ok()?;
            if let Ok(whole) = text.parse::<u64>() {
                return Some(Value::Number(whole));
            }
            // Floating-point spellings are tolerated in ignored values, but cannot supply expires_in.
            text.parse::<f64>().ok().map(|_| Value::Other)
        }

        fn string(&mut self) -> Option<String> {
            self.eat(b'"')?;
            let mut out = String::new();
            loop {
                let b = self.peek()?;
                self.at += 1;
                match b {
                    b'"' => return Some(out),
                    b'\\' => {
                        let escape = self.peek()?;
                        self.at += 1;
                        match escape {
                            b'"' => out.push('"'),
                            b'\\' => out.push('\\'),
                            b'/' => out.push('/'),
                            b'b' => out.push('\u{8}'),
                            b'f' => out.push('\u{c}'),
                            b'n' => out.push('\n'),
                            b'r' => out.push('\r'),
                            b't' => out.push('\t'),
                            b'u' => {
                                let high = self.hex4()?;
                                let c = if (0xD800..0xDC00).contains(&high) {
                                    self.eat(b'\\')?;
                                    self.eat(b'u')?;
                                    let low = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&low) {
                                        return None;
                                    }
                                    char::from_u32(
                                        0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00),
                                    )?
                                } else {
                                    char::from_u32(high)?
                                };
                                out.push(c);
                            }
                            _ => return None,
                        }
                    }
                    0x00..=0x1F => return None,
                    _ => {
                        let start = self.at - 1;
                        let width = match b {
                            0x00..=0x7F => 1,
                            0xC0..=0xDF => 2,
                            0xE0..=0xEF => 3,
                            _ => 4,
                        };
                        let end = start.checked_add(width)?;
                        let chunk = std::str::from_utf8(self.bytes.get(start..end)?).ok()?;
                        out.push_str(chunk);
                        self.at = end;
                    }
                }
            }
        }

        fn hex4(&mut self) -> Option<u32> {
            let end = self.at.checked_add(4)?;
            let hex = self.bytes.get(self.at..end)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            let hex = std::str::from_utf8(hex).ok()?;
            self.at = end;
            u32::from_str_radix(hex, 16).ok()
        }
    }
}
