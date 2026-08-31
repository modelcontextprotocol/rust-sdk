use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use oauth2::{ClientSecret, HttpResponse, RefreshToken};
use serde_json::{Value, json};

use super::{
    EmaExchangeStage::{IdentityProvider as Idp, ResourceAuthorizationServer as ResourceServer},
    *,
};
use crate::transport::auth::{OAuthHttpClientError, OAuthHttpClientFuture, OAuthHttpRequest};

const IDP: &str = "https://idp.example?private-query";
const AS: &str = "https://as.example?private-query";
const RESOURCE: &str = "https://mcp.example?private-query";
const IDP_TOKEN: &str = "https://idp.example/token?private-query";
const AS_TOKEN: &str = "https://as.example/token?private-query";
const BAD_SCOPES: &[&str] = &["", "files\tread", "\"", "\\", "\0", "读"];

#[derive(Default)]
struct MockHttp {
    requests: Mutex<Vec<OAuthHttpRequest>>,
    response: Mutex<Option<Result<HttpResponse, OAuthHttpClientError>>>,
}

impl MockHttp {
    fn new(status: u16, body: Value) -> Self {
        Self {
            response: Mutex::new(Some(Ok(oauth2::http::Response::builder()
                .status(status)
                .body(serde_json::to_vec(&body).unwrap())
                .unwrap()))),
            ..Self::default()
        }
    }
}

impl OAuthHttpClient for MockHttp {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        self.requests.lock().unwrap().push(request);
        let response = self.response.lock().unwrap().take();
        let response = response.expect("unexpected HTTP");
        Box::pin(async move { response })
    }
}

fn jwt(header: Value, claims: &Value) -> String {
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string()),
        URL_SAFE_NO_PAD.encode(b"synthetic-signature")
    )
}

fn claims() -> Value {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    json!({"iss":IDP,"aud":AS,"sub":"user","client_id":"mcp",
        "jti":"jag-id","iat":now,"exp":now + 3600,"resource":RESOURCE,"scope":"files.read"})
}

fn jag(claims: &Value) -> Value {
    let mut body = json!({"access_token":jwt(json!({"alg":"ES256","typ":"oauth-id-jag+jwt"}), claims),
        "issued_token_type":ID_JAG_TOKEN_TYPE,"token_type":"N_A","resource":claims["resource"]});
    if let Some(scope) = claims.get("scope") {
        body["scope"] = scope.clone();
    }
    body
}

async fn exchange(idp: &MockHttp, scopes: &str) -> Result<EmaIdJag, EmaError> {
    let scopes = scopes
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let refresh = RefreshToken::new("refresh-token".into());
    let resource_as = EmaAuthorizationServer::new(AS, AS_TOKEN, "mcp");
    assert!(!format!("{resource_as:?}").contains("private-query"));
    let request = EmaExchangeRequest::new(
        EmaAuthorizationServer::new(IDP, IDP_TOKEN, "idp"),
        resource_as,
        RESOURCE,
        &refresh,
    )
    .scopes(&scopes);
    assert!(!format!("{request:?}").contains(refresh.secret()));
    assert!(!format!("{request:?}").contains("private-query"));
    let future = request.exchange_id_jag(idp);
    fn is_send<T: Send>(_: &T) {}
    is_send(&future);
    future.await
}

fn form(request: &OAuthHttpRequest) -> BTreeMap<String, String> {
    assert!(!request.request.headers().contains_key("authorization"));
    authenticated_form(request)
}

fn authenticated_form(request: &OAuthHttpRequest) -> BTreeMap<String, String> {
    assert_eq!(request.request.method(), "POST");
    let headers = request.request.headers();
    assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
    assert_eq!(headers["accept"], "application/json");
    assert_eq!(request.redirect_policy, OAuthHttpRedirectPolicy::Stop);
    assert_eq!(request.timeout, Some(Duration::from_secs(30)));
    let pairs = url::form_urlencoded::parse(request.request.body())
        .into_owned()
        .collect::<Vec<_>>();
    let fields = pairs.iter().cloned().collect::<BTreeMap<_, _>>();
    assert_eq!(pairs.len(), fields.len(), "duplicate form fields");
    fields
}

#[tokio::test]
async fn refresh_exchange_preserves_exact_forms_and_signed_narrowing() {
    for scopes in ["files.read files.write", ""] {
        let response = jag(&claims());
        let idp = MockHttp::new(200, response.clone());
        let result = exchange(&idp, scopes).await.unwrap();
        assert_eq!(
            result.assertion().secret(),
            response["access_token"].as_str().unwrap()
        );
        assert_eq!(result.scopes(), &HashSet::from(["files.read".to_owned()]));
        assert!(!format!("{result:?}").contains(result.assertion().secret()));
        assert!(!format!("{result:?}").contains("private-query"));
        let requests = idp.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].request.uri(), IDP_TOKEN);
        let mut fields = form(&requests[0]);
        assert_eq!(
            fields.remove("scope"),
            (!scopes.is_empty()).then(|| scopes.to_owned())
        );
        assert_eq!(
            serde_json::to_value(fields).unwrap(),
            json!({
                "grant_type":"urn:ietf:params:oauth:grant-type:token-exchange",
                "requested_token_type":ID_JAG_TOKEN_TYPE,"subject_token":"refresh-token",
                "subject_token_type":"urn:ietf:params:oauth:token-type:refresh_token",
                "audience":AS,"resource":RESOURCE,"client_id":"idp"
            })
        );
    }
}

#[tokio::test]
async fn scopes_may_be_omitted_but_never_widened() {
    for (requested, signed, echoed, valid) in [
        ("", Some("files.read"), None, true),
        ("", None, None, true),
        ("files.read", Some("files.read"), None, true),
        ("files.read files.write", Some("files.read"), None, false),
        (
            "files.read",
            Some("files.read files.write"),
            Some("files.read files.write"),
            false,
        ),
        ("files.read", None, None, false),
        ("", Some("files.read"), Some("files.write"), false),
        ("", Some(" \t"), None, false),
        ("", Some("files.read files.read"), None, false),
        ("", Some("files.read"), Some(" \t"), false),
        ("", Some("files.read"), Some("files.read files.read"), false),
    ] {
        let mut claims = claims();
        claims.as_object_mut().unwrap().remove("scope");
        if let Some(scope) = signed {
            claims["scope"] = json!(scope);
        }
        let mut response = jag(&claims);
        response.as_object_mut().unwrap().remove("scope");
        if let Some(scope) = echoed {
            response["scope"] = json!(scope);
        }
        let result = exchange(&MockHttp::new(200, response), requested).await;
        assert_eq!(
            result.is_ok(),
            valid,
            "requested={requested:?}, signed={signed:?}, echoed={echoed:?}"
        );
        if let Ok(result) = result {
            let expected = signed.into_iter().map(str::to_owned).collect();
            assert_eq!(result.scopes(), &expected);
        }
    }
}

#[tokio::test]
async fn invalid_jags_never_escape_validation() {
    let original = claims();
    let mut cases = Vec::new();
    let invalid = json!({"iss":"https://other.example","aud":[AS,"other"],
        "client_id":"other","sub":"","jti":" \t","exp":0,"iat":u64::MAX,"resource":[RESOURCE,"other"]});
    for (key, value) in invalid.as_object().unwrap() {
        let mut changed = original.clone();
        changed[key] = value.clone();
        cases.push(jag(&changed));
    }
    for scope in BAD_SCOPES {
        let mut changed = original.clone();
        changed["scope"] = json!(scope);
        cases.push(jag(&changed));
    }
    for header in [
        json!({"alg":"ES256","typ":"JWT"}),
        json!({"alg":"ES256"}),
        json!({"alg":"none","typ":"oauth-id-jag+jwt"}),
    ] {
        let mut response = jag(&original);
        response["access_token"] = json!(jwt(header, &original));
        cases.push(response);
    }
    let invalid = json!({"issued_token_type":"Bearer","token_type":"Bearer",
        "refresh_token":"unsupported","resource":"https://other.example","access_token":"a.b.c.d"});
    for (key, value) in invalid.as_object().unwrap() {
        let mut response = jag(&original);
        response[key] = value.clone();
        cases.push(response);
    }
    for signature in ["", "signature", "not+base64url", "c2ln="] {
        let mut response = jag(&original);
        let assertion = response["access_token"].as_str().unwrap();
        let (signed, _) = assertion.rsplit_once('.').unwrap();
        response["access_token"] = json!(format!("{signed}.{signature}"));
        cases.push(response);
    }
    for response in cases {
        let result = exchange(&MockHttp::new(200, response), "files.read").await;
        assert!(matches!(
            result,
            Err(EmaError::InvalidResponse { stage: Idp, .. })
        ));
    }
}

#[tokio::test]
async fn invalid_inputs_fail_before_http() {
    // Each server occupies issuer, token endpoint, and client ID slots.
    let original = [IDP, IDP_TOKEN, "idp", AS, AS_TOKEN, "mcp", RESOURCE, "rt"];
    let mut cases = Vec::new();
    for index in [0, 1, 3, 4, 6] {
        for value in [
            "invalid",
            "http://idp.example/token",
            "https://user:pass@idp.example/token",
            "https://idp.example/token#fragment",
        ] {
            let mut fields = original;
            fields[index] = value;
            cases.push((fields, vec![]));
        }
    }
    for (index, value) in [(0, AS), (2, " "), (5, ""), (7, " \t")] {
        let mut fields = original;
        fields[index] = value;
        cases.push((fields, vec![]));
    }
    for scope in BAD_SCOPES.iter().copied().chain(["files.read files.write"]) {
        cases.push((original, vec![scope]));
    }
    cases.push((original, vec!["files.read", "files.read"]));
    for (fields, scopes) in cases {
        let http = MockHttp::default();
        let scopes = scopes.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let result = EmaExchangeRequest::new(
            EmaAuthorizationServer::new(fields[0], fields[1], fields[2]),
            EmaAuthorizationServer::new(fields[3], fields[4], fields[5]),
            fields[6],
            &RefreshToken::new(fields[7].into()),
        )
        .scopes(&scopes)
        .exchange_id_jag(&http)
        .await;
        assert!(matches!(result, Err(EmaError::InvalidRequest(_))));
        assert!(http.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn errors_and_redirects_cannot_reflect_credentials() {
    const SECRET: &str = "secret-error-sentinel";
    for (status, code) in [
        (400, "invalid_grant"),
        (400, "insufficient_user_authentication"),
        (400, "invalid_client"),
        (400, SECRET),
        (302, SECRET),
    ] {
        let failure = json!({"error":code,"error_description":SECRET});
        let error = exchange(&MockHttp::new(status, failure), "")
            .await
            .unwrap_err();
        match code {
            "invalid_grant" => assert_eq!(error, EmaError::InvalidGrant(Idp)),
            "insufficient_user_authentication" => {
                assert_eq!(error, EmaError::InsufficientUserAuthentication(Idp))
            }
            _ => assert!(
                matches!(error, EmaError::OAuthRejected {stage: Idp, status: actual, ..} if actual == status)
            ),
        }
        assert!(!format!("{error:?} {error}").contains(SECRET));
    }
    for adapter_failure in [false, true] {
        let failure = MockHttp::new(
            200,
            json!({"access_token":SECRET,"issued_token_type":SECRET}),
        );
        if adapter_failure {
            *failure.response.lock().unwrap() = Some(Err(SECRET.into()));
        }
        let error = exchange(&failure, "").await.unwrap_err();
        assert!(!format!("{error:?} {error}").contains(SECRET));
        assert!(std::error::Error::source(&error).is_none());
        if adapter_failure {
            assert_eq!(error, EmaError::RequestFailed(Idp));
        }
    }
    let mut oversized = jag(&claims());
    oversized["ignored"] = json!("x".repeat(1024 * 1024));
    let error = exchange(&MockHttp::new(200, oversized), "")
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        EmaError::InvalidResponse { stage: Idp, .. }
    ));
}

#[derive(Clone, Copy)]
enum AssertionBehavior {
    Success,
    Error,
    Empty(&'static str),
}

struct RecordingAssertionProvider {
    calls: Mutex<Vec<(String, String, String)>>,
    behavior: AssertionBehavior,
}

impl RecordingAssertionProvider {
    fn new(behavior: AssertionBehavior) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            behavior,
        })
    }
}

fn client_assertion(client_id: &str, issuer: &str, sequence: usize) -> String {
    jwt(
        json!({"alg":"ES256","typ":"client-authentication+jwt"}),
        &json!({"iss":client_id,"sub":client_id,"aud":issuer,
            "exp":4102444800_u64,"jti":format!("client-assertion-{sequence}")}),
    )
}

#[async_trait::async_trait]
impl EmaClientAssertionProvider for RecordingAssertionProvider {
    async fn create_assertion(
        &self,
        server: &EmaAuthorizationServer,
    ) -> Result<EmaClientAssertion, Box<dyn std::error::Error + Send + Sync>> {
        let sequence = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((
                server.issuer.clone(),
                server.token_endpoint.clone(),
                server.client_id.clone(),
            ));
            calls.len()
        };
        match self.behavior {
            AssertionBehavior::Error => return Err("client-assertion-provider-secret".into()),
            AssertionBehavior::Empty(value) => return Ok(EmaClientAssertion::new(value.into())),
            AssertionBehavior::Success => {}
        }
        Ok(EmaClientAssertion::new(client_assertion(
            &server.client_id,
            &server.issuer,
            sequence,
        )))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthenticationMethod {
    Public,
    Basic,
    Post,
    Jwt,
}

impl AuthenticationMethod {
    fn configure(
        self,
        secret: &str,
        provider: &Arc<RecordingAssertionProvider>,
    ) -> EmaClientAuthentication {
        match self {
            Self::Public => EmaClientAuthentication::None,
            Self::Basic => {
                EmaClientAuthentication::ClientSecretBasic(ClientSecret::new(secret.into()))
            }
            Self::Post => {
                EmaClientAuthentication::ClientSecretPost(ClientSecret::new(secret.into()))
            }
            Self::Jwt => EmaClientAuthentication::JwtAssertion(provider.clone()),
        }
    }
}

fn remove_client_authentication(
    request: &OAuthHttpRequest,
    method: AuthenticationMethod,
    client_id: &str,
    secret: &str,
    encoded_basic: &str,
    assertion: &str,
) -> BTreeMap<String, String> {
    let mut fields = authenticated_form(request);
    let authorization = request.request.headers().get("authorization");
    if method == AuthenticationMethod::Basic {
        let authorization = authorization.expect("Basic authentication must use a header");
        assert!(authorization.is_sensitive());
        let encoded = authorization
            .to_str()
            .unwrap()
            .strip_prefix("Basic ")
            .unwrap();
        assert_eq!(STANDARD.decode(encoded).unwrap(), encoded_basic.as_bytes());
        assert!(!fields.contains_key("client_id"));
    } else {
        assert!(authorization.is_none());
        assert_eq!(fields.remove("client_id").as_deref(), Some(client_id));
    }
    assert_eq!(
        fields.remove("client_secret").as_deref(),
        (method == AuthenticationMethod::Post).then_some(secret)
    );
    assert_eq!(
        fields.remove("client_assertion_type").as_deref(),
        (method == AuthenticationMethod::Jwt)
            .then_some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer")
    );
    assert_eq!(
        fields.remove("client_assertion").as_deref(),
        (method == AuthenticationMethod::Jwt).then_some(assertion)
    );
    fields
}

#[tokio::test]
async fn idp_client_authentication_is_explicit_and_separate_from_the_grant() {
    const IDP_CLIENT: &str = "idp:+ %é";
    const IDP_SECRET: &str = "idp-secret:+ %é";
    for method in [
        AuthenticationMethod::Public,
        AuthenticationMethod::Basic,
        AuthenticationMethod::Post,
        AuthenticationMethod::Jwt,
    ] {
        let provider = RecordingAssertionProvider::new(AssertionBehavior::Success);
        let server = EmaAuthorizationServer::new(IDP, IDP_TOKEN, IDP_CLIENT)
            .with_client_authentication(method.configure(IDP_SECRET, &provider));
        let refresh = RefreshToken::new("refresh-token".into());
        assert!(provider.calls.lock().unwrap().is_empty());
        // Reusing configuration must ask the signer for a fresh assertion.
        for attempt in 1..=2 {
            let idp = MockHttp::new(200, jag(&claims()));
            EmaExchangeRequest::new(
                server.clone(),
                EmaAuthorizationServer::new(AS, AS_TOKEN, "mcp"),
                RESOURCE,
                &refresh,
            )
            .exchange_id_jag(&idp)
            .await
            .unwrap();
            let requests = idp.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].request.uri(), IDP_TOKEN);
            let fields = remove_client_authentication(
                &requests[0],
                method,
                IDP_CLIENT,
                IDP_SECRET,
                "idp%3A%2B+%25%C3%A9:idp-secret%3A%2B+%25%C3%A9",
                &client_assertion(IDP_CLIENT, IDP, attempt),
            );
            assert_eq!(
                serde_json::to_value(fields).unwrap(),
                json!({
                    "grant_type":"urn:ietf:params:oauth:grant-type:token-exchange",
                    "requested_token_type":ID_JAG_TOKEN_TYPE,"subject_token":"refresh-token",
                    "subject_token_type":"urn:ietf:params:oauth:token-type:refresh_token",
                    "audience":AS,"resource":RESOURCE
                })
            );
            let expected_calls = if method == AuthenticationMethod::Jwt {
                vec![(IDP.into(), IDP_TOKEN.into(), IDP_CLIENT.into()); attempt]
            } else {
                Vec::new()
            };
            assert_eq!(*provider.calls.lock().unwrap(), expected_calls);
        }
    }
}

#[tokio::test]
async fn invalid_static_client_secrets_fail_before_any_http_or_signing() {
    for stage in [Idp, ResourceServer] {
        for method in [AuthenticationMethod::Basic, AuthenticationMethod::Post] {
            for secret in ["", " \t"] {
                let provider = RecordingAssertionProvider::new(AssertionBehavior::Success);
                let valid = EmaClientAuthentication::JwtAssertion(provider.clone());
                let invalid = method.configure(secret, &provider);
                let (idp_auth, resource_auth) = match stage {
                    Idp => (invalid, valid),
                    _ => (valid, invalid),
                };
                let idp = MockHttp::default();
                let error = EmaExchangeRequest::new(
                    EmaAuthorizationServer::new(IDP, IDP_TOKEN, "idp")
                        .with_client_authentication(idp_auth),
                    EmaAuthorizationServer::new(AS, AS_TOKEN, "mcp")
                        .with_client_authentication(resource_auth),
                    RESOURCE,
                    &RefreshToken::new("refresh-token".into()),
                )
                .exchange_id_jag(&idp)
                .await
                .unwrap_err();
                assert!(matches!(error, EmaError::InvalidRequest(_)));
                assert!(idp.requests.lock().unwrap().is_empty());
                assert!(provider.calls.lock().unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn assertion_provider_failures_and_empty_results_never_reach_http_or_escape_errors() {
    for behavior in [
        AssertionBehavior::Error,
        AssertionBehavior::Empty(""),
        AssertionBehavior::Empty(" \t"),
    ] {
        let provider = RecordingAssertionProvider::new(behavior);
        let idp = MockHttp::default();
        let error = EmaExchangeRequest::new(
            EmaAuthorizationServer::new(IDP, IDP_TOKEN, "idp").with_client_authentication(
                EmaClientAuthentication::JwtAssertion(provider.clone()),
            ),
            EmaAuthorizationServer::new(AS, AS_TOKEN, "mcp"),
            RESOURCE,
            &RefreshToken::new("refresh-token".into()),
        )
        .exchange_id_jag(&idp)
        .await
        .unwrap_err();
        if matches!(behavior, AssertionBehavior::Error) {
            assert_eq!(error, EmaError::RequestFailed(Idp));
        } else {
            assert!(matches!(error, EmaError::InvalidRequest(_)));
        }
        assert!(!format!("{error:?} {error}").contains("client-assertion-provider-secret"));
        assert!(std::error::Error::source(&error).is_none());
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert!(idp.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn rejected_client_authentication_never_falls_back_or_retries() {
    for method in [
        AuthenticationMethod::Basic,
        AuthenticationMethod::Post,
        AuthenticationMethod::Jwt,
    ] {
        let provider = RecordingAssertionProvider::new(AssertionBehavior::Success);
        let idp = MockHttp::new(
            401,
            json!({"error":"invalid_client","error_description":"client-authentication-secret"}),
        );
        let error = EmaExchangeRequest::new(
            EmaAuthorizationServer::new(IDP, IDP_TOKEN, "idp")
                .with_client_authentication(method.configure("idp-client-secret", &provider)),
            EmaAuthorizationServer::new(AS, AS_TOKEN, "mcp"),
            RESOURCE,
            &RefreshToken::new("refresh-token".into()),
        )
        .exchange_id_jag(&idp)
        .await
        .unwrap_err();
        assert_eq!(
            error,
            EmaError::OAuthRejected {
                stage: Idp,
                status: 401,
                code: "invalid_client"
            }
        );
        assert!(!format!("{error:?} {error}").contains("client-authentication-secret"));
        assert_eq!(idp.requests.lock().unwrap().len(), 1);
        assert_eq!(
            provider.calls.lock().unwrap().len(),
            usize::from(method == AuthenticationMethod::Jwt)
        );
    }
}

#[test]
fn client_authentication_debug_output_redacts_credentials_and_provider_state() {
    const SECRET: &str = "client-authentication-secret-sentinel";
    let provider = RecordingAssertionProvider::new(AssertionBehavior::Empty(SECRET));
    let assertion = EmaClientAssertion::new(SECRET.into());
    assert!(!format!("{assertion:?}").contains(SECRET));
    for authentication in [
        EmaClientAuthentication::ClientSecretBasic(ClientSecret::new(SECRET.into())),
        EmaClientAuthentication::ClientSecretPost(ClientSecret::new(SECRET.into())),
        EmaClientAuthentication::JwtAssertion(provider.clone()),
    ] {
        assert!(!format!("{authentication:?}").contains(SECRET));
        let server = EmaAuthorizationServer::new(IDP, IDP_TOKEN, "idp")
            .with_client_authentication(authentication);
        let refresh = RefreshToken::new("refresh-token".into());
        let request = EmaExchangeRequest::new(server.clone(), server.clone(), RESOURCE, &refresh);
        assert!(!format!("{server:?} {request:?}").contains(SECRET));
        assert!(!format!("{server:?} {request:?}").contains("private-query"));
    }
}
