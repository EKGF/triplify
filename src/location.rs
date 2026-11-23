//! Where a source or an output lives: a local path or an S3 object.

use {
    crate::s3::S3Url,
    std::{fmt, path::PathBuf, str::FromStr},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
    S3(S3Url),
}

impl Location {
    /// The final name component, used for format detection.
    pub fn file_name(&self) -> Option<String> {
        match self {
            Self::Local(p) => {
                p.file_name().map(|n| n.to_string_lossy().into_owned())
            },
            Self::S3(u) => {
                Some(u.file_name().to_owned()).filter(|n| !n.is_empty())
            },
        }
    }
}

impl FromStr for Location {
    type Err = crate::s3::InvalidS3Url;

    /// `s3://bucket/key` is S3; anything else is a local path.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if S3Url::looks_like(s) {
            s.parse().map(Self::S3)
        } else {
            Ok(Self::Local(PathBuf::from(s)))
        }
    }
}

impl From<PathBuf> for Location {
    fn from(p: PathBuf) -> Self { Self::Local(p) }
}

impl From<&std::path::Path> for Location {
    fn from(p: &std::path::Path) -> Self { Self::Local(p.to_owned()) }
}

impl From<S3Url> for Location {
    fn from(u: S3Url) -> Self { Self::S3(u) }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(p) => write!(f, "{}", p.display()),
            Self::S3(u) => write!(f, "{u}"),
        }
    }
}
