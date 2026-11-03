//! The `If` request header (RFC 4918 §10.4).
//!
//! ```text
//! If: (<opaquelocktoken:a> ["etag"]) (Not <DAV:no-lock>)
//! If: <http://host/dav/s/f> (<opaquelocktoken:a>)
//! ```
//!
//! The header is a list of condition lists. Untagged lists apply to the request
//! URI; tagged lists apply to the resource named by the tag. For each resource
//! mentioned, at least one of its lists must be true (every condition in a
//! list must hold) or the request fails with 412. Every lock token that
//! appears anywhere in the header counts as *submitted* for lock checks.

/// One condition in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
    /// `Not` was given.
    pub not: bool,
    /// What is tested.
    pub kind: CondKind,
}

/// The two kinds of condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CondKind {
    /// A state (lock) token, without the angle brackets.
    Token(String),
    /// An entity tag, with its quotes (and `W/` prefix if weak).
    ETag(String),
}

/// One parenthesised list, with the resource it applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfList {
    /// The tagged resource URI, or `None` for the request URI.
    pub resource: Option<String>,
    /// The conditions, all of which must hold.
    pub conditions: Vec<Condition>,
}

/// A parsed `If` header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IfHeader {
    /// The lists, in order.
    pub lists: Vec<IfList>,
}

/// The header did not follow the grammar (400 Bad Request).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed If header")]
pub struct IfParseError;

impl IfHeader {
    /// Parses the header value.
    ///
    /// # Errors
    ///
    /// Fails on anything that does not match the RFC 4918 grammar.
    pub fn parse(value: &str) -> Result<Self, IfParseError> {
        let mut p = Parser {
            s: value.as_bytes(),
            i: 0,
        };
        let mut lists = Vec::new();
        let mut tag: Option<String> = None;
        let mut tagged: Option<bool> = None;
        loop {
            p.ws();
            match p.peek() {
                None => break,
                Some(b'<') => {
                    if tagged == Some(false) {
                        return Err(IfParseError);
                    }
                    tagged = Some(true);
                    tag = Some(p.angle()?);
                    // A tag must be followed by at least one list.
                    p.ws();
                    if p.peek() != Some(b'(') {
                        return Err(IfParseError);
                    }
                }
                Some(b'(') => {
                    if tagged.is_none() {
                        tagged = Some(false);
                    }
                    lists.push(IfList {
                        resource: tag.clone(),
                        conditions: p.list()?,
                    });
                }
                Some(_) => return Err(IfParseError),
            }
        }
        if lists.is_empty() {
            return Err(IfParseError);
        }
        Ok(Self { lists })
    }

    /// Every lock token named anywhere in the header (whether negated or not).
    pub fn tokens(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .lists
            .iter()
            .flat_map(|l| &l.conditions)
            .filter_map(|c| match &c.kind {
                CondKind::Token(t) => Some(t.clone()),
                CondKind::ETag(_) => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The distinct tagged resources, in order of first appearance.
    pub fn resources(&self) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = Vec::new();
        for l in &self.lists {
            if !out.contains(&l.resource) {
                out.push(l.resource.clone());
            }
        }
        out
    }
}

/// Evaluates one list against a resource, given a way to test tokens and the
/// resource's current entity tag (`None` if it does not exist).
pub fn list_matches(
    list: &IfList,
    token_matches: impl Fn(&str) -> bool,
    etag: Option<&str>,
) -> bool {
    list.conditions.iter().all(|c| {
        let ok = match &c.kind {
            CondKind::Token(t) => token_matches(t),
            // Entity tags compare strongly: a weak tag never matches.
            CondKind::ETag(e) => !e.starts_with("W/") && etag == Some(e.as_str()),
        };
        ok != c.not
    })
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }

    fn until(&mut self, end: u8) -> Result<String, IfParseError> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c == end {
                let v = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| IfParseError)?;
                self.i += 1;
                return Ok(v.to_owned());
            }
            self.i += 1;
        }
        Err(IfParseError)
    }

    fn angle(&mut self) -> Result<String, IfParseError> {
        self.i += 1; // '<'
        let v = self.until(b'>')?;
        if v.is_empty() || v.contains(char::is_whitespace) {
            return Err(IfParseError);
        }
        Ok(v)
    }

    fn list(&mut self) -> Result<Vec<Condition>, IfParseError> {
        self.i += 1; // '('
        let mut conds = Vec::new();
        loop {
            self.ws();
            let mut not = false;
            if self.s[self.i..].len() >= 3
                && self.s[self.i..self.i + 3].eq_ignore_ascii_case(b"not")
            {
                not = true;
                self.i += 3;
                self.ws();
            }
            match self.peek() {
                Some(b'<') => conds.push(Condition {
                    not,
                    kind: CondKind::Token(self.angle()?),
                }),
                Some(b'[') => {
                    self.i += 1;
                    let e = self.until(b']')?;
                    let e = e.trim().to_owned();
                    if e.is_empty() {
                        return Err(IfParseError);
                    }
                    conds.push(Condition {
                        not,
                        kind: CondKind::ETag(e),
                    });
                }
                Some(b')') if !not && !conds.is_empty() => {
                    self.i += 1;
                    return Ok(conds);
                }
                _ => return Err(IfParseError),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(t: &str, not: bool) -> Condition {
        Condition {
            not,
            kind: CondKind::Token(t.into()),
        }
    }

    #[test]
    fn untagged() {
        let h = IfHeader::parse(r#"(<opaquelocktoken:a> ["x"]) (Not <DAV:no-lock>)"#).unwrap();
        assert_eq!(h.lists.len(), 2);
        assert_eq!(
            h.lists[0].conditions,
            vec![
                tok("opaquelocktoken:a", false),
                Condition {
                    not: false,
                    kind: CondKind::ETag("\"x\"".into())
                }
            ]
        );
        assert_eq!(h.lists[1].conditions, vec![tok("DAV:no-lock", true)]);
        assert_eq!(h.tokens(), vec!["DAV:no-lock", "opaquelocktoken:a"]);
        assert_eq!(h.resources(), vec![None]);
    }

    #[test]
    fn tagged() {
        let h = IfHeader::parse(
            "<http://h/dav/a> (<urn:t1>) (<urn:t2>)\r\n <http://h/dav/b> (NOT [W/\"e\"])",
        )
        .unwrap();
        assert_eq!(h.lists.len(), 3);
        assert_eq!(h.lists[1].resource.as_deref(), Some("http://h/dav/a"));
        assert_eq!(h.lists[2].resource.as_deref(), Some("http://h/dav/b"));
        assert!(h.lists[2].conditions[0].not);
        assert_eq!(h.resources().len(), 2);
    }

    #[test]
    fn malformed() {
        for bad in [
            "",
            "()",
            "(<a>",
            "<http://x>",
            "(<a>) <http://x> (<b>)",
            "(Not)",
            "(<a b>)",
            "([])",
            "garbage",
        ] {
            assert!(IfHeader::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn evaluation() {
        let h = IfHeader::parse(r#"(<t1> ["e1"])"#).unwrap();
        let l = &h.lists[0];
        assert!(list_matches(l, |t| t == "t1", Some("\"e1\"")));
        assert!(!list_matches(l, |t| t == "t1", Some("\"e2\"")));
        assert!(!list_matches(l, |_| false, Some("\"e1\"")));
        let h = IfHeader::parse("(Not <DAV:no-lock>)").unwrap();
        assert!(list_matches(&h.lists[0], |_| false, None));
        let h = IfHeader::parse(r#"([W/"e1"])"#).unwrap();
        assert!(!list_matches(&h.lists[0], |_| false, Some("W/\"e1\"")));
    }
}
