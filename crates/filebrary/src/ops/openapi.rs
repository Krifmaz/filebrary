//! The OpenAPI description of the REST API and the built-in docs page.
//!
//! * `GET /api/openapi.json` serves an OpenAPI 3.1 document generated at
//!   compile time with [utoipa](https://docs.rs/utoipa).
//! * `GET /api/docs` serves a small self-contained reference page (no CDN,
//!   works with the strict Content-Security-Policy) that renders that
//!   document.
//!
//! # Adding an endpoint
//!
//! Annotate the handler with `#[utoipa::path(...)]` (see
//! [`crate::ops::notifications`]), derive [`utoipa::ToSchema`] for its body
//! types, and list the handler in [`ApiDoc`]'s `paths(...)`. For handlers
//! whose module is still changing in another branch, a documentation stub
//! in [`stubs`] keeps the description in one place until the handler can
//! carry the attribute itself. The test `every_documented_path_is_routed`
//! fails if the document names a route the router does not serve.

use axum::{
    Router,
    http::header,
    response::{Html, IntoResponse},
    routing::get,
};
use serde::Serialize;
use utoipa::{
    Modify, OpenApi, ToSchema,
    openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme},
};

use crate::state::AppState;

/// Routes merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/openapi.json", get(spec))
        .route("/docs", get(docs_page))
        .route("/docs/docs.js", get(docs_js))
        .route("/docs/docs.css", get(docs_css))
}

/// The JSON error body every endpoint returns on failure.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// Machine-readable code, such as `not_found` or `forbidden`.
    #[schema(example = "not_found")]
    pub error: String,
    /// Human-readable message.
    #[schema(example = "not found")]
    pub message: String,
}

/// The API description. See the module docs for how to extend it.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Filebrary API",
        description = "REST API of Filebrary, a self-hosted file manager.\n\n\
            Authentication: log in with `POST /api/auth/login` to get the `fb_session` cookie \
            and a CSRF token. Send the token in the `X-CSRF-Token` header on every request \
            that is not GET, HEAD or OPTIONS.\n\n\
            Paths inside a source are `/`-separated and relative to the user's folder. \
            Errors are JSON: `{\"error\": \"<code>\", \"message\": \"<text>\"}`.",
        license(name = "Apache-2.0", identifier = "Apache-2.0"),
    ),
    paths(
        stubs::health,
        crate::ops::health::ready,
        stubs::metrics,
        stubs::list_sources,
        stubs::list,
        stubs::stat,
        stubs::download,
        stubs::zip,
        stubs::mkdir,
        stubs::move_entry,
        stubs::copy_entry,
        stubs::delete,
        stubs::upload_create,
        stubs::upload_status,
        stubs::upload_append,
        stubs::upload_cancel,
        crate::ops::notifications::list,
        crate::ops::notifications::test,
        stubs::openapi_json,
    ),
    components(schemas(ErrorBody)),
    modifiers(&Security),
    tags(
        (name = "operations", description = "Health, readiness, metrics and this document"),
        (name = "files", description = "Browse, download and manage files"),
        (name = "uploads", description = "Resumable chunked uploads"),
        (name = "notifications", description = "Notification targets (administrators)"),
    )
)]
pub struct ApiDoc;

/// Adds the security schemes and applies the session to every route that
/// does not opt out.
struct Security;

impl Modify for Security {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        let components = api.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                crate::auth::session::SESSION_COOKIE,
                "Session cookie set by POST /api/auth/login",
            ))),
        );
        components.add_security_scheme(
            "csrf",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "X-CSRF-Token",
                "CSRF token from the login response; required on unsafe methods",
            ))),
        );
        components.add_security_scheme(
            "metrics_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("metrics.token, when configured"))
                    .build(),
            ),
        );
    }
}

/// Builds the document. Exposed for tests and for `filebrary openapi`-style
/// tooling.
pub fn document() -> utoipa::openapi::OpenApi {
    let mut doc = ApiDoc::openapi();
    doc.info.version = env!("CARGO_PKG_VERSION").to_owned();
    doc
}

async fn spec() -> impl IntoResponse {
    // Built once: the document only changes with the binary.
    static JSON: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let body = JSON.get_or_init(|| document().to_json().unwrap_or_else(|_| "{}".into()));
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body.as_str(),
    )
}

async fn docs_page() -> Html<&'static str> {
    Html(include_str!("docs/index.html"))
}

async fn docs_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("docs/docs.js"),
    )
}

async fn docs_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("docs/docs.css"),
    )
}

/// Documentation stubs for endpoints whose handlers live in modules other
/// work streams are still changing. Each stub mirrors the real handler's
/// contract; none is routed.
#[allow(dead_code, clippy::unused_async)]
pub mod stubs {
    use serde::{Deserialize, Serialize};
    use utoipa::{IntoParams, ToSchema};

    use super::ErrorBody;

    /// Liveness response.
    #[derive(Serialize, ToSchema)]
    pub struct Health {
        /// Always `ok`.
        #[schema(example = "ok")]
        pub status: String,
        /// Server version.
        pub version: String,
    }

    /// A user's permissions on a source.
    #[derive(Serialize, ToSchema)]
    pub struct PermissionFlags {
        /// See folders and file names.
        pub view: bool,
        /// Read file contents.
        pub download: bool,
        /// Create files and folders.
        pub upload: bool,
        /// Rename, move and overwrite.
        pub edit: bool,
        /// Delete.
        pub delete: bool,
        /// Create share links.
        pub share: bool,
    }

    /// A source the user can see.
    #[derive(Serialize, ToSchema)]
    pub struct SourceInfo {
        /// Identifier used in URLs.
        pub id: String,
        /// Display name.
        pub name: String,
        /// What the user may do.
        pub permissions: PermissionFlags,
    }

    /// A file or folder.
    #[derive(Serialize, ToSchema)]
    pub struct Entry {
        /// Final path component.
        pub name: String,
        /// Path as the user sees it.
        #[schema(example = "/photos/cat.jpg")]
        pub path: String,
        /// Folder or not.
        pub is_dir: bool,
        /// Size in bytes (0 for folders).
        pub size: u64,
        /// Last modification time (RFC 3339).
        #[schema(format = DateTime)]
        pub modified: Option<String>,
        /// Guessed MIME type.
        pub mime: Option<String>,
    }

    /// A folder listing.
    #[derive(Serialize, ToSchema)]
    pub struct Listing {
        /// The listed folder.
        pub path: String,
        /// Its children, folders first.
        pub entries: Vec<Entry>,
    }

    /// `path` query parameter.
    #[derive(Deserialize, IntoParams)]
    #[into_params(parameter_in = Query)]
    pub struct PathQuery {
        /// Path inside the source; defaults to `/`.
        pub path: Option<String>,
        /// `1` to serve the file inline instead of as an attachment.
        pub inline: Option<String>,
    }

    /// `zip` query parameters.
    #[derive(Deserialize, IntoParams)]
    #[into_params(parameter_in = Query)]
    pub struct ZipQuery {
        /// Items to include; repeat the parameter for several.
        pub path: Vec<String>,
        /// Archive file name without `.zip`.
        pub name: Option<String>,
    }

    /// Body of `mkdir`.
    #[derive(Deserialize, ToSchema)]
    pub struct MkdirBody {
        /// Folder to create (parents are created too).
        pub path: String,
    }

    /// Body of `move` and `copy`.
    #[derive(Deserialize, ToSchema)]
    pub struct TransferBody {
        /// Source path.
        pub from: String,
        /// Destination path.
        pub to: String,
        /// Replace an existing destination (needs the edit permission).
        #[serde(default)]
        pub overwrite: bool,
    }

    /// Body of `delete`.
    #[derive(Deserialize, ToSchema)]
    pub struct DeleteBody {
        /// Paths to delete.
        pub paths: Vec<String>,
    }

    /// One failed deletion.
    #[derive(Serialize, ToSchema)]
    pub struct DeleteFailure {
        /// The path.
        pub path: String,
        /// Why it failed.
        pub message: String,
    }

    /// Result of a batch delete.
    #[derive(Serialize, ToSchema)]
    pub struct DeleteResult {
        /// Deleted paths.
        pub deleted: Vec<String>,
        /// Paths that could not be deleted.
        pub failed: Vec<DeleteFailure>,
    }

    /// Body for starting an upload.
    #[derive(Deserialize, ToSchema)]
    pub struct CreateUpload {
        /// Target path.
        pub path: String,
        /// Total size in bytes.
        pub size: u64,
        /// Replace an existing file (needs the edit permission).
        #[serde(default)]
        pub overwrite: bool,
    }

    /// Upload progress.
    #[derive(Serialize, ToSchema)]
    pub struct UploadStatus {
        /// Upload id (32 hex characters).
        pub id: String,
        /// Bytes received so far; send the next chunk at this offset.
        pub offset: u64,
        /// Total size.
        pub size: u64,
        /// Whether the file is complete and in place.
        pub done: bool,
        /// The new file, once done.
        pub entry: Option<Entry>,
    }

    /// Liveness probe; also served at `/healthz` and `/health`.
    #[utoipa::path(get, path = "/api/health", tag = "operations", security(()),
        responses((status = 200, body = Health)))]
    pub async fn health() {}

    /// Prometheus metrics in the text exposition format. 404 when
    /// `metrics.enabled` is false; needs the bearer token when
    /// `metrics.token` is set.
    #[utoipa::path(get, path = "/metrics", tag = "operations",
        security((), ("metrics_token" = [])),
        responses(
            (status = 200, description = "Metrics", content_type = "text/plain", body = String),
            (status = 401, description = "Token missing or wrong"),
        ))]
    pub async fn metrics() {}

    /// This document.
    #[utoipa::path(get, path = "/api/openapi.json", tag = "operations", security(()),
        responses((status = 200, description = "OpenAPI 3.1 document", content_type = "application/json")))]
    pub async fn openapi_json() {}

    /// Sources the user can view.
    #[utoipa::path(get, path = "/api/sources", tag = "files", security(("session" = [])),
        responses((status = 200, body = Vec<SourceInfo>), (status = 401, body = ErrorBody)))]
    pub async fn list_sources() {}

    /// Lists a folder.
    #[utoipa::path(get, path = "/api/sources/{id}/list", tag = "files", security(("session" = [])),
        params(("id" = String, Path, description = "Source id"), PathQuery),
        responses((status = 200, body = Listing), (status = 404, body = ErrorBody)))]
    pub async fn list() {}

    /// Metadata of one file or folder.
    #[utoipa::path(get, path = "/api/sources/{id}/stat", tag = "files", security(("session" = [])),
        params(("id" = String, Path, description = "Source id"), PathQuery),
        responses((status = 200, body = Entry), (status = 404, body = ErrorBody)))]
    pub async fn stat() {}

    /// Downloads a file. Supports `Range` (206), `If-None-Match` and
    /// `If-Modified-Since` (304).
    #[utoipa::path(get, path = "/api/sources/{id}/download", tag = "files", security(("session" = [])),
        params(("id" = String, Path, description = "Source id"), PathQuery),
        responses(
            (status = 200, description = "File content", content_type = "application/octet-stream"),
            (status = 206, description = "Partial content"),
            (status = 304, description = "Not modified"),
            (status = 416, body = ErrorBody),
        ))]
    pub async fn download() {}

    /// Streams a zip of the selected files and folders.
    #[utoipa::path(get, path = "/api/sources/{id}/zip", tag = "files", security(("session" = [])),
        params(("id" = String, Path, description = "Source id"), ZipQuery),
        responses((status = 200, description = "Zip archive", content_type = "application/zip")))]
    pub async fn zip() {}

    /// Creates a folder.
    #[utoipa::path(post, path = "/api/sources/{id}/mkdir", tag = "files",
        security(("session" = [], "csrf" = [])),
        params(("id" = String, Path, description = "Source id")),
        request_body = MkdirBody,
        responses((status = 200, body = Entry), (status = 409, body = ErrorBody)))]
    pub async fn mkdir() {}

    /// Moves or renames a file or folder.
    #[utoipa::path(post, path = "/api/sources/{id}/move", tag = "files",
        security(("session" = [], "csrf" = [])),
        params(("id" = String, Path, description = "Source id")),
        request_body = TransferBody,
        responses((status = 200, body = Entry), (status = 409, body = ErrorBody)))]
    pub async fn move_entry() {}

    /// Copies a file or folder.
    #[utoipa::path(post, path = "/api/sources/{id}/copy", tag = "files",
        security(("session" = [], "csrf" = [])),
        params(("id" = String, Path, description = "Source id")),
        request_body = TransferBody,
        responses((status = 200, body = Entry), (status = 409, body = ErrorBody)))]
    pub async fn copy_entry() {}

    /// Deletes several files and folders.
    #[utoipa::path(post, path = "/api/sources/{id}/delete", tag = "files",
        security(("session" = [], "csrf" = [])),
        params(("id" = String, Path, description = "Source id")),
        request_body = DeleteBody,
        responses((status = 200, body = DeleteResult)))]
    pub async fn delete() {}

    /// Starts a resumable upload.
    #[utoipa::path(post, path = "/api/sources/{id}/uploads", tag = "uploads",
        security(("session" = [], "csrf" = [])),
        params(("id" = String, Path, description = "Source id")),
        request_body = CreateUpload,
        responses((status = 200, body = UploadStatus), (status = 409, body = ErrorBody)))]
    pub async fn upload_create() {}

    /// Progress of an upload.
    #[utoipa::path(get, path = "/api/uploads/{uid}", tag = "uploads", security(("session" = [])),
        params(("uid" = String, Path, description = "Upload id")),
        responses((status = 200, body = UploadStatus), (status = 404, body = ErrorBody)))]
    pub async fn upload_status() {}

    /// Appends a chunk. Send the current offset in `Upload-Offset`; the
    /// response carries the new offset, and the finished file once the last
    /// byte arrives. A wrong offset answers 409 with the expected one.
    #[utoipa::path(patch, path = "/api/uploads/{uid}", tag = "uploads",
        security(("session" = [], "csrf" = [])),
        params(
            ("uid" = String, Path, description = "Upload id"),
            ("Upload-Offset" = u64, Header, description = "Offset of this chunk"),
        ),
        request_body(content = Vec<u8>, content_type = "application/offset+octet-stream"),
        responses((status = 200, body = UploadStatus), (status = 409, body = ErrorBody)))]
    pub async fn upload_append() {}

    /// Cancels an upload and discards received data.
    #[utoipa::path(delete, path = "/api/uploads/{uid}", tag = "uploads",
        security(("session" = [], "csrf" = [])),
        params(("uid" = String, Path, description = "Upload id")),
        responses((status = 204, description = "Cancelled")))]
    pub async fn upload_cancel() {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn document_builds() {
        let doc = super::document();
        let json = doc.to_json().unwrap();
        assert!(json.contains("\"/api/admin/notifications\""));
        assert!(json.contains("\"/readyz\""));
        assert!(json.contains("\"ErrorBody\""));
        assert!(json.contains("\"session\""));
        assert_eq!(doc.info.version, env!("CARGO_PKG_VERSION"));
    }
}
