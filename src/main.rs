use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    client: Client,
    api_keys: Arc<Vec<String>>,
    github_client_id: String,
    github_client_secret: String,
    github_redirect_uri: String,
    oauth_states: Arc<RwLock<HashMap<String, String>>>,
    linked_accounts: Arc<RwLock<HashMap<String, String>>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GithubUser {
    login: String,
    avatar_url: String,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GithubTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    read_only: bool,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug, Deserialize)]
struct OAuthCallback {
    code: String,
    state: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let api_keys = env::var("API_KEYS")
        .expect("API_KEYS must be set")
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if api_keys.is_empty() {
        panic!("API_KEYS must contain at least one key");
    }

    let state = AppState {
        client: Client::builder()
            .user_agent("github-image-api/0.1")
            .build()
            .expect("failed to build HTTP client"),
        api_keys: Arc::new(api_keys),
        github_client_id: required_env("GITHUB_CLIENT_ID"),
        github_client_secret: required_env("GITHUB_CLIENT_SECRET"),
        github_redirect_uri: required_env("GITHUB_REDIRECT_URI"),
        oauth_states: Arc::new(RwLock::new(HashMap::new())),
        linked_accounts: Arc::new(RwLock::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/oauth/github/start", get(github_start))
        .route("/oauth/github/callback", get(github_callback))
        .route("/v1/users/:username", get(fetch_user))
        .route("/v1/users/:username/avatar", get(fetch_avatar))
        .route("/v1/users/:username/repos", get(fetch_repositories))
        .route(
            "/v1/users/:username/repositories",
            get(fetch_repository_links),
        )
        .route(
            "/v1/users/:username/repos/:repo/tree",
            get(fetch_repository_tree),
        )
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_owned());
    let port = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_owned())
        .parse::<u16>()
        .expect("PORT must be a valid number");
    let address: SocketAddr = format!("{host}:{port}")
        .parse()
        .expect("invalid HOST or PORT");

    println!("GitHub image API listening on http://{address}");
    println!("Read-only avatar endpoint: GET /v1/users/{{username}}/avatar");

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("failed to bind server");
    axum::serve(listener, app).await.expect("server failed");
}

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        read_only: true,
    })
}

async fn github_start(State(state): State<AppState>) -> Response {
    let state_token = Uuid::new_v4().to_string();
    state
        .oauth_states
        .write()
        .await
        .insert(state_token.clone(), "pending".to_owned());

    let query = format!(
        "client_id={}&redirect_uri={}&scope=read%3Auser%20repo&state={}",
        encode(&state.github_client_id),
        encode(&state.github_redirect_uri),
        encode(&state_token)
    );
    Redirect::temporary(&format!("https://github.com/login/oauth/authorize?{query}"))
        .into_response()
}

async fn github_callback(
    State(state): State<AppState>,
    Query(callback): Query<OAuthCallback>,
) -> Response {
    let is_valid_state = state
        .oauth_states
        .write()
        .await
        .remove(&callback.state)
        .is_some();
    if !is_valid_state {
        return error_response(StatusCode::BAD_REQUEST, "invalid or expired OAuth state");
    }

    let token = match state
        .client
        .post("https://github.com/login/oauth/access_token")
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", state.github_client_id.as_str()),
            ("client_secret", state.github_client_secret.as_str()),
            ("code", callback.code.as_str()),
            ("redirect_uri", state.github_redirect_uri.as_str()),
        ])
        .send()
        .await
    {
        Ok(response) => match response.json::<GithubTokenResponse>().await {
            Ok(payload) => match payload.access_token {
                Some(access_token) => access_token,
                None => {
                    return error_response(
                        StatusCode::BAD_GATEWAY,
                        payload.error_description.as_deref().unwrap_or(
                            payload
                                .error
                                .as_deref()
                                .unwrap_or("GitHub token exchange failed"),
                        ),
                    )
                }
            },
            Err(_) => {
                return error_response(StatusCode::BAD_GATEWAY, "invalid GitHub token response")
            }
        },
        Err(_) => return error_response(StatusCode::BAD_GATEWAY, "could not reach GitHub"),
    };

    let github_user = match state
        .client
        .get("https://api.github.com/user")
        .bearer_auth(&token)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => match response.json::<GithubUser>().await
        {
            Ok(user) => user,
            Err(_) => {
                return error_response(StatusCode::BAD_GATEWAY, "invalid GitHub user response")
            }
        },
        _ => return error_response(StatusCode::BAD_GATEWAY, "could not fetch GitHub user"),
    };

    state
        .linked_accounts
        .write()
        .await
        .insert(github_user.login.clone(), token);

    Json(serde_json::json!({
        "message": "GitHub account connected",
        "username": github_user.login,
        "next": "Use GET /v1/users/{username}/avatar with your X-API-Key header"
    }))
    .into_response()
}

async fn fetch_avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(username): Path<String>,
) -> Response {
    if !authorized(&headers, &state.api_keys) {
        return error_response(StatusCode::UNAUTHORIZED, "missing or invalid X-API-Key");
    }

    let username = username.trim();
    if username.is_empty() || username.len() > 39 {
        return error_response(StatusCode::BAD_REQUEST, "invalid GitHub username");
    }

    let github_user = match github_user(&state.client, username).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    let image = match state.client.get(github_user.avatar_url).send().await {
        Ok(response) if response.status().is_success() => {
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .cloned()
                .unwrap_or_else(|| header::HeaderValue::from_static("image/jpeg"));
            match response.bytes().await {
                Ok(bytes) => (content_type, bytes),
                Err(_) => {
                    return error_response(StatusCode::BAD_GATEWAY, "could not read GitHub image")
                }
            }
        }
        _ => return error_response(StatusCode::BAD_GATEWAY, "could not fetch GitHub image"),
    };

    let mut response = image.1.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, image.0);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=300"),
    );
    response
}

async fn fetch_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(username): Path<String>,
) -> Response {
    if !authorized(&headers, &state.api_keys) {
        return error_response(StatusCode::UNAUTHORIZED, "missing or invalid X-API-Key");
    }

    let username = username.trim();
    if username.is_empty() || username.len() > 39 {
        return error_response(StatusCode::BAD_REQUEST, "invalid GitHub username");
    }

    match github_user(&state.client, username).await {
        Ok(user) => Json(user).into_response(),
        Err(response) => response,
    }
}

async fn fetch_repositories(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(username): Path<String>,
) -> Response {
    if !authorized(&headers, &state.api_keys) {
        return error_response(StatusCode::UNAUTHORIZED, "missing or invalid X-API-Key");
    }

    let username = username.trim();
    if username.is_empty() || username.len() > 39 {
        return error_response(StatusCode::BAD_REQUEST, "invalid GitHub username");
    }

    let token = match connected_token(&state, username).await {
        Ok(token) => token,
        Err(response) => return response,
    };

    match state
        .client
        .get("https://api.github.com/user/repos?visibility=all&affiliation=owner%2Ccollaborator%2Corganization_member&per_page=100")
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => match response.json::<serde_json::Value>().await {
            Ok(repositories) => Json(repositories).into_response(),
            Err(_) => error_response(StatusCode::BAD_GATEWAY, "invalid GitHub repositories response"),
        },
        Ok(response) if response.status() == reqwest::StatusCode::FORBIDDEN => error_response(
            StatusCode::FORBIDDEN,
            "GitHub denied repository access; reconnect with private repository permission",
        ),
        _ => error_response(StatusCode::BAD_GATEWAY, "could not fetch GitHub repositories"),
    }
}

async fn fetch_repository_links(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(username): Path<String>,
) -> Response {
    if !authorized(&headers, &state.api_keys) {
        return error_response(StatusCode::UNAUTHORIZED, "missing or invalid X-API-Key");
    }

    let username = username.trim();
    if username.is_empty() || username.len() > 39 {
        return error_response(StatusCode::BAD_REQUEST, "invalid GitHub username");
    }

    let token = match connected_token(&state, username).await {
        Ok(token) => token,
        Err(response) => return response,
    };

    match state
        .client
        .get("https://api.github.com/user/repos?visibility=all&affiliation=owner%2Ccollaborator%2Corganization_member&per_page=100")
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            match response.json::<Vec<serde_json::Value>>().await {
                Ok(repositories) => {
                    let links = repositories
                        .into_iter()
                        .filter_map(|repository| {
                            Some(serde_json::json!({
                                "name": repository.get("full_name")?.as_str()?,
                                "link": repository.get("html_url")?.as_str()?,
                            }))
                        })
                        .collect::<Vec<_>>();
                    Json(links).into_response()
                }
                Err(_) => error_response(
                    StatusCode::BAD_GATEWAY,
                    "invalid GitHub repositories response",
                ),
            }
        }
        _ => error_response(StatusCode::BAD_GATEWAY, "could not fetch GitHub repositories"),
    }
}

#[derive(Debug, Deserialize)]
struct TreeQuery {
    branch: Option<String>,
}

async fn fetch_repository_tree(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((username, repo)): Path<(String, String)>,
    Query(query): Query<TreeQuery>,
) -> Response {
    if !authorized(&headers, &state.api_keys) {
        return error_response(StatusCode::UNAUTHORIZED, "missing or invalid X-API-Key");
    }

    let username = username.trim();
    let repo = repo.trim();
    if username.is_empty() || username.len() > 39 || repo.is_empty() || repo.len() > 100 {
        return error_response(StatusCode::BAD_REQUEST, "invalid GitHub repository path");
    }

    let token = match connected_token(&state, username).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    let branch = query.branch.unwrap_or_else(|| "HEAD".to_owned());
    let tree_url = format!(
        "https://api.github.com/repos/{username}/{repo}/git/trees/{}?recursive=1",
        encode_path(&branch)
    );

    match state.client.get(tree_url).bearer_auth(token).send().await {
        Ok(response) if response.status().is_success() => {
            match response.json::<serde_json::Value>().await {
                Ok(tree) => {
                    let entries = tree
                        .get("tree")
                        .and_then(serde_json::Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| {
                                    let path = item.get("path")?.as_str()?;
                                    let kind = item.get("type")?.as_str()?;
                                    let object = if kind == "tree" { "tree" } else { "blob" };
                                    Some(serde_json::json!({
                                        "name": path,
                                        "link": format!("https://github.com/{username}/{repo}/{object}/{branch}/{path}"),
                                    }))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Json(entries).into_response()
                }
                Err(_) => error_response(StatusCode::BAD_GATEWAY, "invalid GitHub tree response"),
            }
        }
        Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => error_response(
            StatusCode::NOT_FOUND,
            "GitHub repository or branch not found",
        ),
        _ => error_response(
            StatusCode::BAD_GATEWAY,
            "could not fetch GitHub repository tree",
        ),
    }
}

async fn connected_token(state: &AppState, username: &str) -> Result<String, Response> {
    state
        .linked_accounts
        .read()
        .await
        .get(username)
        .cloned()
        .ok_or_else(|| {
            error_response(
                StatusCode::FORBIDDEN,
                "connect this GitHub account before reading its repositories",
            )
        })
}

async fn github_user(client: &Client, username: &str) -> Result<GithubUser, Response> {
    match client
        .get(format!("https://api.github.com/users/{username}"))
        .send()
        .await
    {
        Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => Err(error_response(
            StatusCode::NOT_FOUND,
            "GitHub user not found",
        )),
        Ok(response) if response.status().is_success() => match response.json::<GithubUser>().await
        {
            Ok(user) => Ok(user),
            Err(_) => Err(error_response(
                StatusCode::BAD_GATEWAY,
                "invalid GitHub user response",
            )),
        },
        _ => Err(error_response(
            StatusCode::BAD_GATEWAY,
            "could not reach GitHub",
        )),
    }
}

fn authorized(headers: &HeaderMap, api_keys: &[String]) -> bool {
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|provided| api_keys.iter().any(|key| key == provided))
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn encode_path(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.to_owned(),
        }),
    )
        .into_response()
}
