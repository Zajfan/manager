//! Virtual paths: one address format for every place a file can live.
//!
//! A plain `PathBuf` can only describe a file on a local disk. A file manager also
//! needs to describe things like:
//!
//! - a file on an SFTP server: `sftp://me@host/home/me/notes.txt`
//! - a file *inside* a ZIP on your disk: `/home/me/photos.zip` → `/2024/img.jpg`
//! - a file inside a TAR inside a ZIP on a server…
//!
//! So a [`VPath`] is a **base** (where the outermost file system lives) plus a stack of
//! **layers** (containers opened along the way, innermost last):
//!
//! ```text
//!   base: Local("/home/me/photos.zip")
//!   layers: [ zip "/2024/img.jpg" ]
//! ```
//!
//! ## The text form
//!
//! We use the same URI style as Apache Commons VFS, so it is round-trippable and unambiguous:
//!
//! ```text
//!   file:///home/me/notes.txt
//!   sftp://me@host:22/home/me/notes.txt
//!   zip:file:///home/me/photos.zip!/2024/img.jpg
//!   tar:zip:file:///backup.zip!/inner.tar!/etc/hosts
//! ```
//!
//! Each layer adds a `kind:` prefix on the left (innermost first) and a `!/path` on the right.
//! Characters that would be ambiguous (`!`, `%`, spaces, non-ASCII, …) are percent-encoded.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// A location anywhere: local disk, remote server, or inside an archive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VPath {
    base: Base,
    layers: Vec<Layer>,
}

/// Where the outermost file system lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Base {
    /// An absolute path on this computer, kept in the OS's native form so that
    /// names which aren't valid UTF-8 (possible on Linux) still work exactly.
    Local(PathBuf),
    /// A file system reached over the network, e.g. `sftp` + `me@host:22` + `/home/me`.
    Remote {
        scheme: String,
        authority: String,
        path: InnerPath,
    },
}

/// A container opened as a folder, e.g. a ZIP file. `kind` is the format (`zip`, `tar`, …),
/// `path` is the location inside that container.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Layer {
    pub kind: String,
    pub path: InnerPath,
}

/// A `/`-separated absolute path inside a non-local file system (archive, server).
///
/// Always normalized: no empty parts, no `.` or `..`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct InnerPath {
    parts: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// VPath
// ---------------------------------------------------------------------------------------------

impl VPath {
    /// A path on the local disk. Relative paths are resolved against the current directory.
    pub fn local(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err(Error::InvalidPath("empty path".into()));
        }
        let absolute = std::path::absolute(path)
            .map_err(|e| Error::InvalidPath(format!("{}: {e}", path.display())))?;
        Ok(Self::from_local_absolute(absolute))
    }

    /// For paths we already know are absolute (e.g. ones returned by `read_dir`).
    pub(crate) fn from_local_absolute(path: PathBuf) -> Self {
        VPath {
            base: Base::Local(path),
            layers: Vec::new(),
        }
    }

    /// A path on a remote file system, e.g. `VPath::remote("sftp", "me@host", "/home/me")`.
    pub fn remote(scheme: &str, authority: &str, path: &str) -> Result<Self> {
        check_remote(scheme, authority)?;
        Ok(VPath {
            base: Base::Remote {
                scheme: scheme.to_ascii_lowercase(),
                authority: authority.to_string(),
                path: InnerPath::parse(path),
            },
            layers: Vec::new(),
        })
    }

    /// Parses either a URI (`file:///…`, `sftp://…`, `zip:file:///…!/…`)
    /// or a plain native path the user typed (`/home/me`, `C:\Users\me`, `docs/`).
    pub fn parse(input: &str) -> Result<Self> {
        match split_uri(input) {
            Some(uri) => uri.into_vpath(),
            None => Self::local(input),
        }
    }

    /// The unambiguous text form, suitable for config files and bookmarks.
    pub fn to_uri(&self) -> String {
        let mut out = String::new();
        for layer in self.layers.iter().rev() {
            out.push_str(&layer.kind);
            out.push(':');
        }
        match &self.base {
            Base::Local(path) => {
                out.push_str("file://");
                out.push_str(&encode_local_path(path));
            }
            Base::Remote {
                scheme,
                authority,
                path,
            } => {
                out.push_str(scheme);
                out.push_str("://");
                out.push_str(authority);
                out.push_str(&path.to_encoded());
            }
        }
        for layer in &self.layers {
            out.push('!');
            out.push_str(&layer.path.to_encoded());
        }
        out
    }

    pub fn base(&self) -> &Base {
        &self.base
    }

    /// Opened containers, outermost first.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// The native path, if this is a plain local path (not inside an archive, not remote).
    pub fn as_local(&self) -> Option<&Path> {
        match (&self.base, self.layers.is_empty()) {
            (Base::Local(path), true) => Some(path),
            _ => None,
        }
    }

    /// The child `name` inside this folder.
    ///
    /// `name` must be a single file name: no separators, not `.` or `..`.
    pub fn join(&self, name: &str) -> Result<VPath> {
        check_name(name)?;
        let mut out = self.clone();
        match out.layers.last_mut() {
            Some(layer) => layer.path.parts.push(name.to_string()),
            None => match &mut out.base {
                Base::Local(path) => path.push(name),
                Base::Remote { path, .. } => path.parts.push(name.to_string()),
            },
        }
        Ok(out)
    }

    /// The folder containing this one, or `None` at the very top.
    ///
    /// Going up from the root of an archive leaves the archive and lands in the
    /// folder that contains the archive file, just like Total Commander.
    pub fn parent(&self) -> Option<VPath> {
        let mut out = self.clone();
        if let Some(layer) = out.layers.last_mut() {
            if let Some(parent) = layer.path.parent() {
                layer.path = parent;
                return Some(out);
            }
            out.layers.pop();
            return out.parent();
        }
        match &out.base {
            Base::Local(path) => Some(Self::from_local_absolute(path.parent()?.to_path_buf())),
            Base::Remote {
                scheme,
                authority,
                path,
            } => Some(VPath {
                base: Base::Remote {
                    scheme: scheme.clone(),
                    authority: authority.clone(),
                    path: path.parent()?,
                },
                layers: Vec::new(),
            }),
        }
    }

    /// The last name in the path. At the root of an archive, that's the archive's own name.
    /// `None` at the top of a file system (`/`, `C:\`, `sftp://host/`).
    pub fn file_name(&self) -> Option<String> {
        if let Some(layer) = self.layers.last() {
            return match layer.path.file_name() {
                Some(name) => Some(name.to_string()),
                None => self.outer()?.file_name(),
            };
        }
        match &self.base {
            Base::Local(path) => Some(path.file_name()?.to_string_lossy().into_owned()),
            Base::Remote { path, .. } => path.file_name().map(str::to_string),
        }
    }

    pub fn is_root(&self) -> bool {
        self.parent().is_none()
    }

    /// Opens the file at this path as a container of the given `kind` (e.g. `"zip"`),
    /// returning the path of the container's root folder.
    pub fn enter(&self, kind: &str) -> Result<VPath> {
        check_scheme(kind)?;
        let mut out = self.clone();
        out.layers.push(Layer {
            kind: kind.to_ascii_lowercase(),
            path: InnerPath::default(),
        });
        Ok(out)
    }

    /// True if `self` is `other` or somewhere inside it. Used to refuse copying a
    /// folder into itself, which would never end.
    pub fn starts_with(&self, other: &VPath) -> bool {
        let n = other.layers.len();
        if n > self.layers.len() {
            return false;
        }
        if n == 0 {
            return match (&self.base, &other.base) {
                (Base::Local(a), Base::Local(b)) => a.starts_with(b),
                (
                    Base::Remote {
                        scheme: s1,
                        authority: a1,
                        path: p1,
                    },
                    Base::Remote {
                        scheme: s2,
                        authority: a2,
                        path: p2,
                    },
                ) => s1 == s2 && a1 == a2 && p1.parts.starts_with(&p2.parts),
                _ => false,
            };
        }
        let (mine, theirs) = (&self.layers[n - 1], &other.layers[n - 1]);
        self.base == other.base
            && self.layers[..n - 1] == other.layers[..n - 1]
            && mine.kind == theirs.kind
            && mine.path.parts.starts_with(&theirs.path.parts)
    }

    /// Interprets what a user typed, relative to this folder, the way a shell would:
    /// URIs and absolute paths are taken as they are; `backup`, `../old` and
    /// `a/b` are resolved against `self`.
    pub fn resolve(&self, input: &str) -> Result<VPath> {
        let input = input.trim();
        if input.is_empty() {
            return Ok(self.clone());
        }
        if split_uri(input).is_some() || looks_absolute(input) {
            return VPath::parse(input);
        }
        let mut out = self.clone();
        for part in input.split(|c| c == '/' || (cfg!(windows) && c == '\\')) {
            out = match part {
                "" | "." => out,
                ".." => out.parent().unwrap_or(out),
                name => out.join(name)?,
            };
        }
        Ok(out)
    }

    /// The path of the innermost container file itself (the ZIP we are inside of), if any.
    pub fn outer(&self) -> Option<VPath> {
        let mut out = self.clone();
        out.layers.pop()?;
        Some(out)
    }
}

/// Human-friendly form for titles and status lines: native local paths,
/// no percent-encoding, and `!` between a container and the path inside it.
impl fmt::Display for VPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.base {
            Base::Local(path) => write!(f, "{}", path.display())?,
            Base::Remote {
                scheme,
                authority,
                path,
            } => write!(f, "{scheme}://{authority}{path}")?,
        }
        for layer in &self.layers {
            write!(f, "!{}", layer.path)?;
        }
        Ok(())
    }
}

impl std::str::FromStr for VPath {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        VPath::parse(s)
    }
}

// ---------------------------------------------------------------------------------------------
// InnerPath
// ---------------------------------------------------------------------------------------------

impl InnerPath {
    /// Lexically normalizes `a//b/./c/../d` into `/a/b/d`. Never fails: `..` at the root stays at the root.
    pub fn parse(s: &str) -> Self {
        let mut parts: Vec<String> = Vec::new();
        for part in s.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                name => parts.push(name.to_string()),
            }
        }
        InnerPath { parts }
    }

    pub fn is_root(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn parts(&self) -> &[String] {
        &self.parts
    }

    pub fn file_name(&self) -> Option<&str> {
        self.parts.last().map(String::as_str)
    }

    pub fn parent(&self) -> Option<InnerPath> {
        let (_, rest) = self.parts.split_last()?;
        Some(InnerPath {
            parts: rest.to_vec(),
        })
    }

    fn to_encoded(&self) -> String {
        if self.parts.is_empty() {
            return "/".into();
        }
        let mut out = String::new();
        for part in &self.parts {
            out.push('/');
            out.push_str(&percent_encode(part.as_bytes(), false));
        }
        out
    }

    fn from_encoded(s: &str) -> Result<Self> {
        if !s.starts_with('/') {
            return Err(Error::InvalidPath(format!(
                "path inside a container must start with '/': {s:?}"
            )));
        }
        let mut parts = Vec::new();
        for raw in s.split('/').filter(|p| !p.is_empty()) {
            let bytes = percent_decode(raw)?;
            let part = String::from_utf8(bytes)
                .map_err(|_| Error::InvalidPath(format!("not valid UTF-8: {raw:?}")))?;
            check_name(&part)?;
            parts.push(part);
        }
        Ok(InnerPath { parts })
    }
}

impl fmt::Display for InnerPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.parts.is_empty() {
            return f.write_str("/");
        }
        for part in &self.parts {
            write!(f, "/{part}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Parsing the URI form
// ---------------------------------------------------------------------------------------------

/// The pieces of `kind2:kind1:scheme://body!/l1!/l2` before they are validated.
struct RawUri<'a> {
    /// Layer kinds as written, i.e. innermost first.
    kinds: Vec<&'a str>,
    scheme: &'a str,
    /// Everything after `scheme://`.
    body: &'a str,
}

/// Returns `None` if `input` doesn't look like a URI at all (so it's a native path).
fn split_uri(input: &str) -> Option<RawUri<'_>> {
    let mut kinds = Vec::new();
    let mut rest = input;
    loop {
        let colon = rest.find(':')?;
        let token = &rest[..colon];
        // Two-letter minimum keeps Windows drive letters (`C:\`) out.
        if token.len() < 2 || check_scheme(token).is_err() {
            return None;
        }
        rest = &rest[colon + 1..];
        if let Some(body) = rest.strip_prefix("//") {
            return Some(RawUri {
                kinds,
                scheme: token,
                body,
            });
        }
        kinds.push(token);
    }
}

impl RawUri<'_> {
    fn into_vpath(self) -> Result<VPath> {
        let mut segments = self.body.split('!');
        let base_part = segments.next().unwrap_or_default();
        let layer_parts: Vec<&str> = segments.collect();

        if layer_parts.len() != self.kinds.len() {
            return Err(Error::InvalidPath(format!(
                "{} container kind(s) but {} '!' section(s)",
                self.kinds.len(),
                layer_parts.len()
            )));
        }

        let base = if self.scheme.eq_ignore_ascii_case("file") {
            Base::Local(decode_local_path(base_part)?)
        } else {
            let (authority, path) = match base_part.find('/') {
                Some(i) => base_part.split_at(i),
                None => (base_part, "/"),
            };
            check_remote(self.scheme, authority)?;
            Base::Remote {
                scheme: self.scheme.to_ascii_lowercase(),
                authority: authority.to_string(),
                path: InnerPath::from_encoded(path)?,
            }
        };

        let layers = self
            .kinds
            .iter()
            .rev()
            .zip(layer_parts)
            .map(|(kind, path)| {
                check_scheme(kind)?;
                Ok(Layer {
                    kind: kind.to_ascii_lowercase(),
                    path: InnerPath::from_encoded(path)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(VPath { base, layers })
    }
}

/// `/x` everywhere; also `C:\\x`, `C:/x` and `\\\\server\\share` on Windows.
fn looks_absolute(input: &str) -> bool {
    if cfg!(windows) {
        Path::new(input).is_absolute() || input.starts_with(['\\', '/'])
    } else {
        input.starts_with('/')
    }
}

// ---------------------------------------------------------------------------------------------
// Local paths <-> URI text
// ---------------------------------------------------------------------------------------------

/// `/home/me` → `/home/me` (bytes percent-encoded, `/` kept).
#[cfg(unix)]
fn encode_local_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    percent_encode(path.as_os_str().as_bytes(), true)
}

#[cfg(unix)]
fn decode_local_path(s: &str) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    if !s.starts_with('/') {
        return Err(Error::InvalidPath(format!(
            "file:// path must be absolute: {s:?}"
        )));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(percent_decode(
        s,
    )?)))
}

/// `C:\Users\me` → `/C:/Users/me`; `\\server\share` → `///server/share`.
///
/// Windows file names are UTF-16 and may (very rarely) contain unpaired surrogates,
/// which can't be represented in UTF-8; those characters are replaced.
#[cfg(windows)]
fn encode_local_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    format!("/{}", percent_encode(text.as_bytes(), true))
}

#[cfg(windows)]
fn decode_local_path(s: &str) -> Result<PathBuf> {
    let Some(rest) = s.strip_prefix('/') else {
        return Err(Error::InvalidPath(format!(
            "file:// path must be absolute: {s:?}"
        )));
    };
    let text = String::from_utf8(percent_decode(rest)?)
        .map_err(|_| Error::InvalidPath(format!("not valid UTF-8: {s:?}")))?;
    let path = PathBuf::from(text.replace('/', "\\"));
    if !path.is_absolute() {
        return Err(Error::InvalidPath(format!(
            "file:// path must be absolute: {s:?}"
        )));
    }
    Ok(path)
}

// ---------------------------------------------------------------------------------------------
// Validation and percent-encoding
// ---------------------------------------------------------------------------------------------

/// URI scheme rules (RFC 3986): a letter, then letters, digits, `+`, `-`, `.`.
fn check_scheme(s: &str) -> Result<()> {
    let mut chars = s.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidPath(format!("invalid scheme: {s:?}")))
    }
}

fn check_remote(scheme: &str, authority: &str) -> Result<()> {
    check_scheme(scheme)?;
    if scheme.eq_ignore_ascii_case("file") {
        return Err(Error::InvalidPath(
            "use VPath::local for file:// paths".into(),
        ));
    }
    if authority.is_empty() || authority.contains(['/', '!']) {
        return Err(Error::InvalidPath(format!(
            "invalid server address: {authority:?}"
        )));
    }
    Ok(())
}

/// A single file name: not empty, not `.`/`..`, no path separators.
fn check_name(name: &str) -> Result<()> {
    let bad_separator = name.contains('/') || (cfg!(windows) && name.contains('\\'));
    if name.is_empty() || name == "." || name == ".." || bad_separator || name.contains('\0') {
        return Err(Error::InvalidPath(format!("invalid file name: {name:?}")));
    }
    Ok(())
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b':'
                | b'@'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
        )
}

fn percent_encode(bytes: &[u8], keep_slash: bool) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if is_unreserved(b) || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> Result<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| Error::InvalidPath(format!("bad %-escape in {s:?}")))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(uri: &str) -> VPath {
        let p = VPath::parse(uri).unwrap();
        assert_eq!(p.to_uri(), uri, "round trip");
        assert_eq!(VPath::parse(&p.to_uri()).unwrap(), p);
        p
    }

    #[test]
    fn inner_path_normalizes() {
        assert_eq!(InnerPath::parse("a//b/./c/../d").to_string(), "/a/b/d");
        assert_eq!(InnerPath::parse("/../..").to_string(), "/");
        assert!(InnerPath::parse("/").is_root());
    }

    #[test]
    fn remote_paths() {
        let p = roundtrip("sftp://me@host:22/home/me");
        assert_eq!(p.file_name().as_deref(), Some("me"));
        assert_eq!(p.to_string(), "sftp://me@host:22/home/me");
        let root = roundtrip("sftp://me@host:22/");
        assert!(root.is_root());
        assert_eq!(p.parent().unwrap().parent().unwrap(), root);
        assert_eq!(root.file_name(), None);
    }

    #[test]
    fn remote_without_path_is_root() {
        let p = VPath::parse("s3://bucket").unwrap();
        assert!(p.is_root());
        assert_eq!(p.to_uri(), "s3://bucket/");
    }

    #[test]
    fn special_characters_are_escaped() {
        let p = VPath::remote("sftp", "host", "/")
            .unwrap()
            .join("wow! 100% café#1")
            .unwrap();
        assert_eq!(p.to_uri(), "sftp://host/wow%21%20100%25%20caf%C3%A9%231");
        assert_eq!(VPath::parse(&p.to_uri()).unwrap(), p);
        assert_eq!(p.file_name().as_deref(), Some("wow! 100% café#1"));
    }

    #[test]
    fn join_rejects_non_names() {
        let root = VPath::remote("sftp", "host", "/").unwrap();
        for bad in ["", ".", "..", "a/b", "nul\0"] {
            assert!(root.join(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn bad_uris_are_rejected() {
        for bad in [
            "zip:sftp://host/a.zip",         // kind without a '!' section
            "sftp://host/a.zip!/x",          // '!' section without a kind
            "zip:sftp://host/a.zip!nope",    // inner path not absolute
            "sftp://host/%zz",               // broken escape
            "sftp://host/%+1",               // not hex digits
            "sftp:///path",                  // no server
            "zip:sftp://host/a.zip!/%2E%2E", // ".." smuggled in via escaping
        ] {
            assert!(VPath::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn remote_constructor_rejects_file_scheme() {
        assert!(VPath::remote("file", "x", "/").is_err());
    }

    #[test]
    fn nested_archives_on_a_server() {
        let p = roundtrip("tar:zip:sftp://host/backup.zip!/inner.tar!/etc/hosts");
        assert_eq!(p.layers().len(), 2);
        assert_eq!(p.layers()[0].kind, "zip");
        assert_eq!(p.layers()[1].kind, "tar");
        assert_eq!(
            p.to_string(),
            "sftp://host/backup.zip!/inner.tar!/etc/hosts"
        );

        // Walk all the way up: out of the tar, out of the zip, up the server's folders.
        let names: Vec<String> = std::iter::successors(Some(p), VPath::parent)
            .map(|p| p.to_string())
            .collect();
        assert_eq!(
            names,
            [
                "sftp://host/backup.zip!/inner.tar!/etc/hosts",
                "sftp://host/backup.zip!/inner.tar!/etc",
                "sftp://host/backup.zip!/inner.tar!/",
                "sftp://host/backup.zip!/",
                "sftp://host/",
            ]
        );
    }

    #[test]
    fn archive_root_is_named_after_the_archive() {
        let zip_root = VPath::parse("sftp://host/dir/photos.zip")
            .unwrap()
            .enter("zip")
            .unwrap();
        assert_eq!(zip_root.to_uri(), "zip:sftp://host/dir/photos.zip!/");
        assert_eq!(zip_root.file_name().as_deref(), Some("photos.zip"));
        assert_eq!(
            zip_root.outer().unwrap().to_uri(),
            "sftp://host/dir/photos.zip"
        );
        assert_eq!(zip_root.parent().unwrap().to_uri(), "sftp://host/dir");
        assert_eq!(zip_root.as_local(), None);

        let inside = zip_root.join("2024").unwrap().join("img.jpg").unwrap();
        assert_eq!(
            inside.to_uri(),
            "zip:sftp://host/dir/photos.zip!/2024/img.jpg"
        );
    }

    #[test]
    fn starts_with_understands_layers() {
        let dir = VPath::parse("sftp://host/a").unwrap();
        let zip = VPath::parse("zip:sftp://host/a/b.zip!/x/y").unwrap();
        let zip_x = VPath::parse("zip:sftp://host/a/b.zip!/x").unwrap();
        assert!(zip.starts_with(&dir), "inside an archive inside the folder");
        assert!(zip.starts_with(&zip_x));
        assert!(zip.starts_with(&zip));
        assert!(!zip_x.starts_with(&zip));
        assert!(!dir.starts_with(&zip));
        assert!(
            !VPath::parse("sftp://host/ab").unwrap().starts_with(&dir),
            "whole names only"
        );
        assert!(!VPath::parse("sftp://other/a").unwrap().starts_with(&dir));
    }

    #[test]
    fn resolve_like_a_shell() {
        let here = VPath::parse("sftp://host/home/me").unwrap();
        let r = |s: &str| here.resolve(s).unwrap().to_string();
        assert_eq!(r("docs"), "sftp://host/home/me/docs");
        assert_eq!(r("a/b/"), "sftp://host/home/me/a/b");
        assert_eq!(r("../you"), "sftp://host/home/you");
        assert_eq!(r("../../../.."), "sftp://host/");
        assert_eq!(r("./x"), "sftp://host/home/me/x");
        assert_eq!(r(""), "sftp://host/home/me");
        assert_eq!(r("sftp://other/tmp"), "sftp://other/tmp");
    }

    #[cfg(unix)]
    mod unix {
        use super::*;

        #[test]
        fn native_paths_are_local() {
            let p = VPath::parse("/home/me/notes.txt").unwrap();
            assert_eq!(p.as_local(), Some(Path::new("/home/me/notes.txt")));
            assert_eq!(p.to_uri(), "file:///home/me/notes.txt");
            assert_eq!(p.to_string(), "/home/me/notes.txt");
        }

        #[test]
        fn relative_paths_become_absolute() {
            let p = VPath::parse("some/dir").unwrap();
            let expected = std::env::current_dir().unwrap().join("some/dir");
            assert_eq!(p.as_local(), Some(expected.as_path()));
        }

        #[test]
        fn local_roundtrips() {
            roundtrip("file:///");
            roundtrip("file:///home/me/My%20Documents");
            roundtrip("zip:file:///home/me/photos.zip!/2024/img.jpg");
        }

        #[test]
        fn a_colon_in_a_file_name_is_not_a_scheme() {
            let p = VPath::parse("notes:draft.txt").unwrap();
            assert!(p.as_local().unwrap().ends_with("notes:draft.txt"));
        }

        #[test]
        fn local_parent_and_name() {
            let p = VPath::parse("/a/b").unwrap();
            assert_eq!(p.file_name().as_deref(), Some("b"));
            let root = p.parent().unwrap().parent().unwrap();
            assert_eq!(root.to_uri(), "file:///");
            assert!(root.is_root());
            assert_eq!(root.file_name(), None);
        }

        #[test]
        fn non_utf8_names_survive_exactly() {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let raw = Path::new(OsStr::from_bytes(b"/tmp/caf\xE9"));
            let p = VPath::local(raw).unwrap();
            assert_eq!(p.to_uri(), "file:///tmp/caf%E9");
            assert_eq!(VPath::parse(&p.to_uri()).unwrap().as_local(), Some(raw));
        }

        #[test]
        fn resolve_absolute_native_path() {
            let here = VPath::parse("sftp://host/home").unwrap();
            assert_eq!(
                here.resolve("/tmp").unwrap().as_local(),
                Some(Path::new("/tmp"))
            );
        }

        #[test]
        fn file_uri_must_be_absolute() {
            assert!(VPath::parse("file://relative").is_err());
        }
    }

    #[cfg(windows)]
    mod windows {
        use super::*;

        #[test]
        fn drive_paths() {
            let p = VPath::parse(r"C:\Users\me").unwrap();
            assert_eq!(p.as_local(), Some(Path::new(r"C:\Users\me")));
            assert_eq!(p.to_uri(), "file:///C:/Users/me");
            assert_eq!(VPath::parse(&p.to_uri()).unwrap(), p);
            let root = p.parent().unwrap().parent().unwrap();
            assert!(root.is_root());
        }

        #[test]
        fn unc_paths() {
            let p = VPath::parse(r"\\server\share\dir").unwrap();
            assert_eq!(p.to_uri(), "file://///server/share/dir");
            assert_eq!(VPath::parse(&p.to_uri()).unwrap(), p);
        }

        #[test]
        fn backslash_is_not_allowed_in_names() {
            let p = VPath::parse(r"C:\").unwrap();
            assert!(p.join(r"a\b").is_err());
        }
    }
}
