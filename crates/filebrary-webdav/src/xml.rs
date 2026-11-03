//! Reading request bodies and writing responses in XML.
//!
//! Parsing uses `roxmltree`, which resolves namespaces, refuses DTDs (so no
//! entity expansion attacks) and never touches the network or the file system.
//! Output is built with [`XmlWriter`], a small string builder that escapes
//! everything it writes.

use std::fmt::Write as _;

/// The WebDAV namespace.
pub const DAV: &str = "DAV:";

/// Largest XML request body accepted (PROPFIND, PROPPATCH, LOCK).
pub const MAX_BODY: usize = 1024 * 1024;

/// A property name: namespace URI (empty for none) and local name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PropName {
    /// Namespace URI, or `""` for no namespace.
    pub ns: String,
    /// Local name.
    pub name: String,
}

impl PropName {
    /// Builds a name.
    pub fn new(ns: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            ns: ns.into(),
            name: name.into(),
        }
    }

    /// True if this is `DAV:<name>`.
    pub fn is_dav(&self, name: &str) -> bool {
        self.ns == DAV && self.name == name
    }

    fn of(node: roxmltree::Node<'_, '_>) -> Self {
        Self::new(
            node.tag_name().namespace().unwrap_or(""),
            node.tag_name().name(),
        )
    }
}

/// A body that could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("bad XML body: {0}")]
pub struct XmlError(pub String);

/// Parses a body into a document.
///
/// # Errors
///
/// Fails on malformed XML, a DTD, or non-UTF-8 input.
pub fn parse(body: &[u8]) -> Result<roxmltree::Document<'_>, XmlError> {
    let text = std::str::from_utf8(body).map_err(|_| XmlError("not UTF-8".into()))?;
    roxmltree::Document::parse(text).map_err(|e| XmlError(e.to_string()))
}

fn is_dav(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().namespace() == Some(DAV) && node.tag_name().name() == name
}

fn dav_child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| is_dav(*c, name))
}

/// What a PROPFIND asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropfindRequest {
    /// Every property (with optional extra names from `<include>`).
    AllProp(Vec<PropName>),
    /// Only the names of the properties.
    PropName,
    /// These properties.
    Prop(Vec<PropName>),
}

/// Parses a PROPFIND body. An empty body means `allprop` (RFC 4918 §9.1).
///
/// # Errors
///
/// Fails if the body is not a `DAV:propfind` element with one of `allprop`,
/// `propname` or `prop`.
pub fn parse_propfind(body: &[u8]) -> Result<PropfindRequest, XmlError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(PropfindRequest::AllProp(Vec::new()));
    }
    let doc = parse(body)?;
    let root = doc.root_element();
    if !is_dav(root, "propfind") {
        return Err(XmlError("expected DAV:propfind".into()));
    }
    if let Some(prop) = dav_child(root, "prop") {
        return Ok(PropfindRequest::Prop(
            prop.children()
                .filter(roxmltree::Node::is_element)
                .map(PropName::of)
                .collect(),
        ));
    }
    if dav_child(root, "propname").is_some() {
        return Ok(PropfindRequest::PropName);
    }
    if dav_child(root, "allprop").is_some() {
        let include = dav_child(root, "include")
            .map(|i| {
                i.children()
                    .filter(roxmltree::Node::is_element)
                    .map(PropName::of)
                    .collect()
            })
            .unwrap_or_default();
        return Ok(PropfindRequest::AllProp(include));
    }
    Err(XmlError("propfind needs allprop, propname or prop".into()))
}

/// One instruction of a PROPPATCH, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOp {
    /// Set a property to the given serialized element.
    Set(PropName, String),
    /// Remove a property.
    Remove(PropName),
}

/// Parses a PROPPATCH body.
///
/// Values are re-serialized with explicit namespace declarations on every
/// element (see [`serialize_element`]), so they can be stored as strings and
/// written back verbatim later.
///
/// # Errors
///
/// Fails if the body is not a `DAV:propertyupdate`.
pub fn parse_proppatch(body: &[u8]) -> Result<Vec<PatchOp>, XmlError> {
    let doc = parse(body)?;
    let root = doc.root_element();
    if !is_dav(root, "propertyupdate") {
        return Err(XmlError("expected DAV:propertyupdate".into()));
    }
    let mut ops = Vec::new();
    for action in root.children().filter(roxmltree::Node::is_element) {
        let set = if is_dav(action, "set") {
            true
        } else if is_dav(action, "remove") {
            false
        } else {
            continue;
        };
        for prop in action.children().filter(|c| is_dav(*c, "prop")) {
            for p in prop.children().filter(roxmltree::Node::is_element) {
                let name = PropName::of(p);
                if set {
                    ops.push(PatchOp::Set(name, serialize_element(p)));
                } else {
                    ops.push(PatchOp::Remove(name));
                }
            }
        }
    }
    if ops.is_empty() {
        return Err(XmlError("propertyupdate has no set or remove".into()));
    }
    Ok(ops)
}

/// A parsed `DAV:lockinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockInfo {
    /// True for a shared lock, false for exclusive.
    pub shared: bool,
    /// The `DAV:owner` element serialized, if given.
    pub owner: Option<String>,
}

/// Parses a LOCK body.
///
/// # Errors
///
/// Fails if the body is not a `DAV:lockinfo` with a lock scope and a write
/// lock type.
pub fn parse_lockinfo(body: &[u8]) -> Result<LockInfo, XmlError> {
    let doc = parse(body)?;
    let root = doc.root_element();
    if !is_dav(root, "lockinfo") {
        return Err(XmlError("expected DAV:lockinfo".into()));
    }
    let scope = dav_child(root, "lockscope").ok_or_else(|| XmlError("no lockscope".into()))?;
    let shared = if dav_child(scope, "shared").is_some() {
        true
    } else if dav_child(scope, "exclusive").is_some() {
        false
    } else {
        return Err(XmlError("unknown lockscope".into()));
    };
    let ty = dav_child(root, "locktype").ok_or_else(|| XmlError("no locktype".into()))?;
    if dav_child(ty, "write").is_none() {
        return Err(XmlError("only write locks are supported".into()));
    }
    let owner = dav_child(root, "owner").map(serialize_element);
    Ok(LockInfo { shared, owner })
}

/// Serializes an element and its subtree with explicit namespaces.
///
/// Every element whose namespace differs from its parent's carries its own
/// `xmlns="..."`, and namespaced attributes get a locally declared prefix.
/// The result is well-formed on its own, wherever it is embedded.
pub fn serialize_element(node: roxmltree::Node<'_, '_>) -> String {
    let mut out = String::new();
    write_node(&mut out, node, None);
    out
}

fn write_node(out: &mut String, node: roxmltree::Node<'_, '_>, parent_ns: Option<&str>) {
    if node.is_text() {
        out.push_str(&escape_text(node.text().unwrap_or("")));
        return;
    }
    if !node.is_element() {
        return;
    }
    let tag = node.tag_name();
    let ns = tag.namespace().unwrap_or("");
    let _ = write!(out, "<{}", tag.name());
    if parent_ns != Some(ns) {
        let _ = write!(out, " xmlns=\"{}\"", escape_attr(ns));
    }
    for (i, a) in node.attributes().enumerate() {
        match a.namespace() {
            None => {
                let _ = write!(out, " {}=\"{}\"", a.name(), escape_attr(a.value()));
            }
            Some("http://www.w3.org/XML/1998/namespace") => {
                let _ = write!(out, " xml:{}=\"{}\"", a.name(), escape_attr(a.value()));
            }
            Some(ans) => {
                let _ = write!(
                    out,
                    " xmlns:a{i}=\"{}\" a{i}:{}=\"{}\"",
                    escape_attr(ans),
                    a.name(),
                    escape_attr(a.value())
                );
            }
        }
    }
    let mut children = node.children().peekable();
    if children.peek().is_none() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for c in children {
        write_node(out, c, Some(ns));
    }
    let _ = write!(out, "</{}>", tag.name());
}

/// Escapes text content.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escapes an attribute value (double-quoted).
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
    out
}

/// A small builder for WebDAV responses. `DAV:` is bound to the `D` prefix on
/// the root element; other names are written with a default namespace
/// declaration on the element itself.
#[derive(Debug, Default)]
pub struct XmlWriter {
    out: String,
}

impl XmlWriter {
    /// Starts a document with the given `DAV:` root element.
    pub fn new(root: &str) -> Self {
        let mut w = Self {
            out: String::with_capacity(1024),
        };
        let _ = write!(
            w.out,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:{root} xmlns:D=\"DAV:\">"
        );
        w
    }

    /// Opens a `DAV:` element.
    pub fn open(&mut self, name: &str) -> &mut Self {
        let _ = write!(self.out, "<D:{name}>");
        self
    }

    /// Closes a `DAV:` element.
    pub fn close(&mut self, name: &str) -> &mut Self {
        let _ = write!(self.out, "</D:{name}>");
        self
    }

    /// Writes an empty `DAV:` element.
    pub fn empty(&mut self, name: &str) -> &mut Self {
        let _ = write!(self.out, "<D:{name}/>");
        self
    }

    /// Writes a `DAV:` element with escaped text.
    pub fn text(&mut self, name: &str, text: &str) -> &mut Self {
        let _ = write!(self.out, "<D:{name}>{}</D:{name}>", escape_text(text));
        self
    }

    /// Writes an empty element for any property name.
    pub fn empty_prop(&mut self, p: &PropName) -> &mut Self {
        if p.ns == DAV {
            self.empty(&p.name)
        } else {
            let _ = write!(self.out, "<{} xmlns=\"{}\"/>", p.name, escape_attr(&p.ns));
            self
        }
    }

    /// Writes pre-serialized, trusted XML (from [`serialize_element`] or from
    /// this module).
    pub fn raw(&mut self, xml: &str) -> &mut Self {
        self.out.push_str(xml);
        self
    }

    /// Returns what was written to a writer made with `default()` (a
    /// fragment without a root element).
    pub fn finish_fragment(self) -> String {
        self.out
    }

    /// Closes the root and returns the document.
    pub fn finish(mut self, root: &str) -> String {
        let _ = write!(self.out, "</D:{root}>");
        self.out
    }
}

/// Builds a `DAV:error` body with one precondition element, optionally
/// holding hrefs (RFC 4918 §16).
pub fn error_body(condition: &str, hrefs: &[String]) -> String {
    let mut w = XmlWriter::new("error");
    if hrefs.is_empty() {
        w.empty(condition);
    } else {
        w.open(condition);
        for h in hrefs {
            w.text("href", h);
        }
        w.close(condition);
    }
    w.finish("error")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn propfind_forms() {
        assert_eq!(
            parse_propfind(b"").unwrap(),
            PropfindRequest::AllProp(vec![])
        );
        assert_eq!(
            parse_propfind(br#"<D:propfind xmlns:D="DAV:"><D:propname/></D:propfind>"#).unwrap(),
            PropfindRequest::PropName
        );
        let p = parse_propfind(
            br#"<?xml version="1.0"?><propfind xmlns="DAV:"><prop><getetag/><x:a xmlns:x="urn:x"/><b xmlns=""/></prop></propfind>"#,
        )
        .unwrap();
        assert_eq!(
            p,
            PropfindRequest::Prop(vec![
                PropName::new(DAV, "getetag"),
                PropName::new("urn:x", "a"),
                PropName::new("", "b"),
            ])
        );
        assert!(parse_propfind(b"<foo/>").is_err());
        assert!(parse_propfind(b"<D:propfind xmlns:D=\"DAV:\">").is_err());
    }

    #[test]
    fn refuses_dtd() {
        let evil = br#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;">]><D:propfind xmlns:D="DAV:"><D:prop>&lol2;</D:prop></D:propfind>"#;
        assert!(parse_propfind(evil).is_err());
    }

    #[test]
    fn proppatch_keeps_namespaces() {
        let ops = parse_proppatch(
            br#"<D:propertyupdate xmlns:D="DAV:" xmlns:Z="urn:z" xmlns:Y="urn:y"><D:set><D:prop><Z:p xml:lang="en">a &amp; <Y:child Y:at="1">b</Y:child></Z:p></D:prop></D:set><D:remove><D:prop><Z:q/></D:prop></D:remove></D:propertyupdate>"#,
        )
        .unwrap();
        assert_eq!(ops.len(), 2);
        let PatchOp::Set(name, xml) = &ops[0] else {
            panic!()
        };
        assert_eq!(name, &PropName::new("urn:z", "p"));
        assert_eq!(
            xml,
            r#"<p xmlns="urn:z" xml:lang="en">a &amp; <child xmlns="urn:y" xmlns:a0="urn:y" a0:at="1">b</child></p>"#
        );
        // The stored value parses on its own.
        assert!(roxmltree::Document::parse(xml).is_ok());
        assert_eq!(ops[1], PatchOp::Remove(PropName::new("urn:z", "q")));
    }

    #[test]
    fn lockinfo() {
        let l = parse_lockinfo(
            br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:shared/></D:lockscope><D:locktype><D:write/></D:locktype><D:owner><D:href>me</D:href></D:owner></D:lockinfo>"#,
        )
        .unwrap();
        assert!(l.shared);
        assert_eq!(
            l.owner.as_deref(),
            Some(r#"<owner xmlns="DAV:"><href>me</href></owner>"#)
        );
        assert!(parse_lockinfo(br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope></D:lockinfo>"#).is_err());
    }

    #[test]
    fn writer_escapes() {
        let mut w = XmlWriter::new("multistatus");
        w.text("href", "/a&b<c>");
        w.empty_prop(&PropName::new("urn:\"q", "x"));
        let s = w.finish("multistatus");
        assert!(s.contains("<D:href>/a&amp;b&lt;c&gt;</D:href>"));
        assert!(s.contains(r#"<x xmlns="urn:&quot;q"/>"#));
        assert!(roxmltree::Document::parse(&s).is_ok());
    }
}
