use std::{
    ffi::OsString,
    os::unix::prelude::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

use bstr::{BStr, BString, ByteSlice};

use crate::Workspace;

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[allow(clippy::module_name_repetitions)]
pub struct WsPath(PathBuf);

impl WsPath {
    pub fn new_canonicalized(
        path: impl AsRef<Path>,
        workspace: &Workspace,
    ) -> Result<Self, NewCanonicalizeError> {
        let path = path.as_ref();
        let abs = workspace
            .path()
            .join(path)
            .canonicalize()
            .map_err(|e| NewCanonicalizeError::Io(path.to_owned(), e))?;
        if let Ok(path) = abs.strip_prefix(workspace.path()) {
            Ok(Self::new_unchecked(path))
        } else {
            Err(NewCanonicalizeError::NotInWorkspace(path.to_owned()))
        }
    }

    /// Path must be in canonical form and inside the workspace you use it with
    pub fn new_unchecked(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn new_unchecked_bytes(path: impl Into<BString>) -> Self {
        let path: BString = path.into();
        let path: Vec<u8> = path.into();
        let path = OsString::from_vec(path);
        let path = PathBuf::from(path);
        Self(path)
    }

    pub fn as_bstr(&self) -> &BStr {
        self.0.as_os_str().as_bytes().as_bstr()
    }

    pub fn to_bstring(&self) -> BString {
        self.as_bstr().to_owned()
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.clone()
    }

    /// Panics if self is outside of workspace
    pub fn to_absolute(&self, workspace: &Workspace) -> PathBuf {
        let path = workspace.path().join(&self.0);
        if !path.starts_with(workspace.path()) {
            panic!("Workspace path outside of workspace was created: {:?}. Refusing to make absolute. Workspace: {:?}", self, workspace);
        }
        path
    }

    pub fn file_name(&self) -> &BStr {
        if let Some(name) = self.0.file_name() {
            name.as_bytes().as_bstr()
        } else {
            panic!(
                "Non-normalized path was created: {:?}. Failed to get file name",
                self,
            )
        }
    }

    pub fn parent(&self) -> Option<&Path> {
        self.0.parent()
    }

    pub fn iter_parents(&self) -> Parents {
        Parents::new(self)
    }
}

#[derive(Debug, displaydoc::Display, thiserror::Error)]
pub enum NewCanonicalizeError {
    /// IO error canonicalizing {0:?}
    Io(PathBuf, #[source] std::io::Error),
    /// Path {0:?} is outside the workspace
    NotInWorkspace(PathBuf),
}

impl From<WsPath> for PathBuf {
    fn from(path: WsPath) -> Self {
        path.0
    }
}

impl AsRef<Path> for WsPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl AsRef<BStr> for WsPath {
    fn as_ref(&self) -> &BStr {
        self.as_bstr()
    }
}

impl From<WsPath> for BString {
    fn from(path: WsPath) -> Self {
        path.to_bstring()
    }
}

#[derive(Debug, Clone)]
pub struct Parents<'p> {
    inner: Option<NonEmptyParents<'p>>,
}

#[derive(Debug, Clone)]
struct NonEmptyParents<'p> {
    remaining: std::path::Components<'p>,
    prefix: PathBuf,
}

impl<'p> Parents<'p> {
    fn new(path: &'p WsPath) -> Self {
        let inner = path.as_path().parent().map(|parent| NonEmptyParents {
            remaining: parent.components(),
            prefix: PathBuf::new(),
        });
        Self { inner }
    }
}

impl<'p> Iterator for Parents<'p> {
    type Item = BString;

    fn next(&mut self) -> Option<Self::Item> {
        let inner = self.inner.as_mut()?;

        if let Some(component) = inner.remaining.next() {
            match component {
                std::path::Component::Normal(parent) => {
                    let full = inner.prefix.join(parent).into_os_string().into_vec();
                    let full = BString::from(full);

                    inner.prefix.push(component);

                    Some(full)
                }
                _ => panic!("WsPath wasn't normalized. Refusing to continue iterating over parents. Got {:?}, component: {:?}", inner, component),
            }
        } else {
            self.inner.take();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parents() {
        let path = WsPath::new_unchecked("foo/bar/baq/buz.txt");
        let actual = path.iter_parents().collect::<Vec<_>>();
        let expected = vec!["foo", "foo/bar", "foo/bar/baq"];
        assert_eq!(expected, actual);
    }
}