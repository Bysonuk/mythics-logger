//! A log's share link (the live report) against a stub server on 127.0.0.1:
//! never a real one. The token is made up.

mod common;

use common::Stub;
use mythics_logger_core::api::{Api, ApiError};

const TOKEN: &str = "AbCdEfGhIjKlMnOpQr_-12";

fn api(stub: &Stub) -> Api {
    Api::new(&stub.origin).with_token(Some("app-token".into()))
}

#[tokio::test]
async fn makes_a_link_with_the_app_token_and_keeps_the_token() {
    let stub = Stub::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/api/logger/sessions/12/share") => (
            201,
            format!(
                r#"{{"id": 3, "token": "{TOKEN}", "path": "/shared/{TOKEN}/", "createdAt": "2026-10-02T20:10:00+00:00"}}"#
            ),
        ),
        _ => (500, "{}".into()),
    });
    let link = api(&stub).create_share("12").await.unwrap();
    assert_eq!(link.token, TOKEN);
    assert_eq!(link.path, format!("/shared/{TOKEN}/"));
    let sent = stub.taken();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].authorization.as_deref(), Some("Bearer app-token"));
}

#[tokio::test]
async fn a_path_that_isnt_the_tokens_own_page_is_refused() {
    let stub = Stub::start(|_| {
        (
            201,
            format!(r#"{{"id": 3, "token": "{TOKEN}", "path": "/account/logs/"}}"#),
        )
    });
    assert!(matches!(
        api(&stub).create_share("12").await,
        Err(ApiError::BadReply)
    ));
    stub.set_handler(|_| {
        (
            201,
            r#"{"id": 3, "token": "../x", "path": "/shared/../x/"}"#.into(),
        )
    });
    assert!(matches!(
        api(&stub).create_share("12").await,
        Err(ApiError::BadReply)
    ));
}

#[tokio::test]
async fn a_log_with_no_public_upload_is_refused_with_its_code() {
    let stub = Stub::start(|_| {
        (
            409,
            r#"{"detail": "Only a log with a Public upload can be shared by link.", "code": "log_not_public"}"#.into(),
        )
    });
    match api(&stub).create_share("12").await {
        Err(ApiError::Refused { status: 409, code }) => {
            assert_eq!(code.as_deref(), Some("log_not_public"))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_server_without_share_links_is_a_404_with_no_code() {
    // FastAPI's own answer for a route it doesn't have.
    let stub = Stub::start(|_| (404, r#"{"detail": "Not Found"}"#.into()));
    match api(&stub).create_share("12").await {
        Err(ApiError::Refused { status: 404, code }) => assert_eq!(code, None),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn revokes_the_link() {
    let stub = Stub::start(|r| match (r.method.as_str(), r.path.as_str()) {
        ("DELETE", "/api/logger/sessions/12/share") => (204, String::new()),
        _ => (500, "{}".into()),
    });
    api(&stub).revoke_share("12").await.unwrap();
    stub.set_handler(|_| {
        (
            404,
            r#"{"detail": "This log has no link.", "code": "share_not_found"}"#.into(),
        )
    });
    match api(&stub).revoke_share("12").await {
        Err(ApiError::Refused { status: 404, code }) => {
            assert_eq!(code.as_deref(), Some("share_not_found"))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn checks_a_link_as_a_stranger_would() {
    let stub = Stub::start(|r| match r.path.as_str() {
        p if p == format!("/api/shared/{TOKEN}") => (200, r#"{"id": 12, "live": true}"#.into()),
        _ => (500, "{}".into()),
    });
    assert!(api(&stub).share_works(TOKEN).await.unwrap());
    // Never with the app's token: anyone with the link reads it signed out.
    assert_eq!(stub.taken()[0].authorization, None);
    stub.set_handler(|_| {
        (
            404,
            r#"{"detail": "This link isn't valid any more.", "code": "share_not_found"}"#.into(),
        )
    });
    assert!(!api(&stub).share_works(TOKEN).await.unwrap());
    stub.set_handler(|_| (404, r#"{"detail": "Not Found"}"#.into()));
    assert!(matches!(
        api(&stub).share_works(TOKEN).await,
        Err(ApiError::Refused {
            status: 404,
            code: None
        })
    ));
}
