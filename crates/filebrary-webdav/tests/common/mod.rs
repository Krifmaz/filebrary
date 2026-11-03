//! Shared test harness: an in-process WebDAV mount over temporary local
//! sources, with a small user table.

#![allow(dead_code)]

use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode},
};
use filebrary_storage::{Storage, VPath, local::LocalStorage};
use filebrary_webdav::{
    Access, AuthError, BasicCredentials, Change, DavAuth, DavConfig, DavPerm, DavServer, DavUser,
    Denied, Quota,
};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

/// A user in the test table.
pub struct TestUser {
    pub name: String,
    pub grants: HashMap<String, (Vec<DavPerm>, VPath)>,
    pub quota: Option<Quota>,
}

impl DavUser for TestUser {
    fn id(&self) -> String {
        self.name.clone()
    }
    fn sources(&self) -> Vec<String> {
        let mut v: Vec<String> = self.grants.keys().cloned().collect();
        v.sort();
        v
    }
    fn scope(&self, source: &str) -> Option<VPath> {
        Some(self.grants.get(source)?.1.clone())
    }
    fn quota(&self, _source: &str) -> Option<Quota> {
        self.quota
    }
}

pub struct TestAuth {
    users: HashMap<String, Arc<TestUser>>,
    sources: HashMap<String, Arc<dyn Storage>>,
    pub changes: std::sync::Mutex<Vec<Change>>,
}

#[async_trait]
impl DavAuth for TestAuth {
    async fn authenticate(
        &self,
        headers: &HeaderMap,
        _client: Option<SocketAddr>,
    ) -> Result<Arc<dyn DavUser>, AuthError> {
        let c = BasicCredentials::from_headers(headers).ok_or(AuthError::Unauthorized)?;
        if c.password != "pw" {
            return Err(AuthError::Unauthorized);
        }
        match self.users.get(&c.username) {
            Some(u) => Ok(u.clone() as Arc<dyn DavUser>),
            None => Err(AuthError::Unauthorized),
        }
    }
    async fn authorize(
        &self,
        user: &dyn DavUser,
        source: &str,
        perm: DavPerm,
        path: &VPath,
        _access: Access,
    ) -> Result<(), Denied> {
        let grants = &self.users[&user.id()].grants;
        let (perms, scope) = grants.get(source).ok_or(Denied::NotFound)?;
        if self.hidden(source, &scope.join_path(path)) {
            return Err(Denied::NotFound);
        }
        if !perms.contains(&DavPerm::View) {
            return Err(Denied::NotFound);
        }
        if !perms.contains(&perm) {
            return Err(Denied::Forbidden);
        }
        Ok(())
    }

    async fn storage(&self, user: &dyn DavUser, source: &str) -> Option<Arc<dyn Storage>> {
        let base = self.sources.get(source)?;
        let scope = user.scope(source)?;
        if scope.is_root() {
            Some(base.clone())
        } else {
            base.sub(&scope).await.ok()
        }
    }
    async fn on_change(&self, _user: &dyn DavUser, change: &Change) {
        self.changes.lock().unwrap().push(change.clone());
    }

    fn hidden(&self, _source: &str, real: &VPath) -> bool {
        real.components().first().is_some_and(|c| c == HIDDEN)
    }
}

/// A top-level folder name the test server hides, like Filebrary's trash.
pub const HIDDEN: &str = ".hidden-trash";

const ALL: [DavPerm; 5] = [
    DavPerm::View,
    DavPerm::Download,
    DavPerm::Upload,
    DavPerm::Edit,
    DavPerm::Delete,
];

/// The test server: sources `a` and `b` in temporary directories, and users
/// `alice` (everything on both), `bob` (everything on `a`), `reader` (view and
/// download on `a`), `viewer` (view only on `a`) and `scoped` (everything on
/// `a`, confined to `a/home`). Every password is `pw`.
pub struct Dav {
    pub dir_a: TempDir,
    pub dir_b: TempDir,
    pub app: Router,
    pub auth: Arc<TestAuth>,
}

pub fn grants(list: &[(&str, &[DavPerm], &str)]) -> HashMap<String, (Vec<DavPerm>, VPath)> {
    list.iter()
        .map(|(s, p, scope)| ((*s).to_owned(), (p.to_vec(), VPath::parse(scope).unwrap())))
        .collect()
}

impl Dav {
    pub fn new() -> Self {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir_a.path().join("home")).unwrap();
        let mut sources: HashMap<String, Arc<dyn Storage>> = HashMap::new();
        sources.insert(
            "a".into(),
            Arc::new(LocalStorage::new(dir_a.path()).unwrap()),
        );
        sources.insert(
            "b".into(),
            Arc::new(LocalStorage::new(dir_b.path()).unwrap()),
        );
        let mut users = HashMap::new();
        let mut add = |name: &str, g, quota| {
            users.insert(
                name.to_owned(),
                Arc::new(TestUser {
                    name: name.into(),
                    grants: g,
                    quota,
                }),
            );
        };
        add("alice", grants(&[("a", &ALL, ""), ("b", &ALL, "")]), None);
        add("bob", grants(&[("a", &ALL, "")]), None);
        add(
            "reader",
            grants(&[("a", &[DavPerm::View, DavPerm::Download], "")]),
            None,
        );
        add("viewer", grants(&[("a", &[DavPerm::View], "")]), None);
        add("scoped", grants(&[("a", &ALL, "home")]), None);
        add(
            "both",
            grants(&[("a", &ALL, "home"), ("b", &ALL, "")]),
            None,
        );
        add(
            "quota",
            grants(&[("a", &ALL, "")]),
            Some(Quota {
                used: 90,
                available: Some(10),
            }),
        );
        let auth = Arc::new(TestAuth {
            users,
            sources,
            changes: Default::default(),
        });
        let server = DavServer::new(auth.clone(), DavConfig::default());
        Self {
            dir_a,
            dir_b,
            app: server.router(),
            auth,
        }
    }

    /// Sends a request as `user` (or anonymously with `None`).
    pub async fn req(
        &self,
        user: Option<&str>,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: impl Into<Body>,
    ) -> Res {
        let mut b = Request::builder()
            .method(Method::from_bytes(method.as_bytes()).unwrap())
            .uri(path);
        if let Some(u) = user {
            use base64::Engine as _;
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{u}:pw"));
            b = b.header("authorization", format!("Basic {token}"));
        }
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let resp = self
            .app
            .clone()
            .oneshot(b.body(body.into()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        Res {
            status,
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    /// Shorthand for a request as alice.
    pub async fn alice(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Res {
        self.req(Some("alice"), method, path, headers, body.to_owned())
            .await
    }

    /// Takes an exclusive lock as `user` and returns the token.
    pub async fn lock(&self, user: &str, path: &str, depth: &str, shared: bool) -> String {
        let scope = if shared { "shared" } else { "exclusive" };
        let body = format!(
            r#"<?xml version="1.0"?><D:lockinfo xmlns:D="DAV:"><D:lockscope><D:{scope}/></D:lockscope><D:locktype><D:write/></D:locktype><D:owner>litmus test suite</D:owner></D:lockinfo>"#
        );
        let r = self
            .req(
                Some(user),
                "LOCK",
                path,
                &[("depth", depth), ("timeout", "Second-120")],
                body,
            )
            .await;
        assert!(
            r.status.is_success(),
            "LOCK {path}: {} {}",
            r.status,
            r.body
        );
        let t = r.headers["lock-token"].to_str().unwrap();
        t.trim_start_matches('<').trim_end_matches('>').to_owned()
    }
}

pub struct Res {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Res {
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .map(|v| v.to_str().unwrap())
            .unwrap_or("")
    }

    /// Parses a 207 body into (href, prop local name, status) triples.
    pub fn propstats(&self) -> Vec<(String, String, u16)> {
        let doc = roxmltree::Document::parse(&self.body).expect("valid XML");
        let mut out = Vec::new();
        for resp in doc
            .descendants()
            .filter(|n| n.has_tag_name(("DAV:", "response")))
        {
            let href = resp
                .children()
                .find(|n| n.has_tag_name(("DAV:", "href")))
                .and_then(|n| n.text())
                .unwrap_or("")
                .to_owned();
            for ps in resp
                .children()
                .filter(|n| n.has_tag_name(("DAV:", "propstat")))
            {
                let st = ps
                    .children()
                    .find(|n| n.has_tag_name(("DAV:", "status")))
                    .and_then(|n| n.text())
                    .unwrap_or("");
                let code: u16 = st.split(' ').nth(1).unwrap_or("0").parse().unwrap();
                for prop in ps.children().filter(|n| n.has_tag_name(("DAV:", "prop"))) {
                    for p in prop.children().filter(roxmltree::Node::is_element) {
                        out.push((href.clone(), p.tag_name().name().to_owned(), code));
                    }
                }
            }
        }
        out
    }

    /// The hrefs in a 207 body, in order.
    pub fn hrefs(&self) -> Vec<String> {
        let doc = roxmltree::Document::parse(&self.body).expect("valid XML");
        doc.descendants()
            .filter(|n| n.has_tag_name(("DAV:", "response")))
            .filter_map(|r| {
                r.children()
                    .find(|n| n.has_tag_name(("DAV:", "href")))
                    .and_then(|n| n.text())
                    .map(str::to_owned)
            })
            .collect()
    }
}
