//! What a declaration is named and classified by, shared by the syntax tree and the checker's
//! output.

use ply_span::{Diagnostic, Span, Symbol, codes};
use std::fmt;
use std::path::Path;

/// A module's dotted name, derived from its file's path relative to the project root:
/// `store/orders.ply` is `store.orders`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModuleName(Symbol);

impl Default for ModuleName {
    fn default() -> Self {
        ModuleName::anonymous()
    }
}

impl ModuleName {
    /// The module of source that has no project root: a snippet handed to `ply_syntax::parse`.
    pub fn anonymous() -> ModuleName {
        ModuleName(Symbol::new(""))
    }

    pub fn is_anonymous(&self) -> bool {
        self.0.as_str().is_empty()
    }

    /// Every directory component and the file stem must be a Ply identifier; anything else is
    /// [`codes::INVALID_MODULE_PATH`].
    pub fn from_relative_path(path: &Path) -> Result<ModuleName, Diagnostic> {
        let invalid = |what: &str| {
            Diagnostic::error(
                codes::INVALID_MODULE_PATH,
                format!("`{}` cannot be a module: {what}", path.display()),
            )
            .primary(Span::DUMMY, "this file is not addressable as a module")
            .note("rename it so every directory and the file stem is a plain identifier")
        };

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| invalid("its file name is not valid UTF-8"))?;

        let mut segments: Vec<&str> = Vec::new();
        for component in path.parent().into_iter().flat_map(|p| p.components()) {
            let text = component
                .as_os_str()
                .to_str()
                .ok_or_else(|| invalid("a directory name is not valid UTF-8"))?;
            segments.push(text);
        }
        segments.push(stem);

        for segment in &segments {
            if !is_ident(segment) {
                return Err(invalid(&format!("`{segment}` is not an identifier")));
            }
        }
        Ok(ModuleName(Symbol::new(segments.join("."))))
    }

    /// Trusts the caller that every segment is an identifier.
    pub fn from_dotted(name: impl AsRef<str>) -> ModuleName {
        ModuleName(Symbol::new(name.as_ref()))
    }

    pub fn as_symbol(&self) -> &Symbol {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.as_str().split('.')
    }

    /// The name a bare `import` binds this module as: its last segment.
    pub fn default_binder(&self) -> Symbol {
        Symbol::new(self.0.as_str().rsplit('.').next().unwrap_or(""))
    }

    /// This module's `place` under its program-wide name, `store.orders.place`.
    pub fn qualify(&self, name: &Symbol) -> Symbol {
        if self.is_anonymous() {
            return name.clone();
        }
        Symbol::new(format!("{}.{}", self.0, name))
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// The grammar's identifier rule, which module paths are held to as well.
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_ident_start) && chars.all(is_ident_continue)
}

pub fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

pub fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The derivations the language defines.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Deriver {
    Json,
    Eq,
    Ord,
}

impl Deriver {
    pub const ALL: &'static [Deriver] = &[Deriver::Json, Deriver::Eq, Deriver::Ord];

    pub fn from_name(name: &str) -> Option<Deriver> {
        Some(match name {
            "json" => Deriver::Json,
            "eq" => Deriver::Eq,
            "ord" => Deriver::Ord,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Deriver::Json => "json",
            Deriver::Eq => "eq",
            Deriver::Ord => "ord",
        }
    }

    /// The dictionary type a derivation of this kind produces.
    pub fn dictionary(self) -> &'static str {
        match self {
            Deriver::Json => "JsonCodec",
            Deriver::Eq => "EqDict",
            Deriver::Ord => "OrdDict",
        }
    }

    /// Distinguishes derivers in a definition hash and in a stored body.
    pub fn tag(self) -> u8 {
        match self {
            Deriver::Json => 1,
            Deriver::Eq => 2,
            Deriver::Ord => 3,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Deriver> {
        Some(match tag {
            1 => Deriver::Json,
            2 => Deriver::Eq,
            3 => Deriver::Ord,
            _ => return None,
        })
    }
}

impl fmt::Display for Deriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpecKind {
    Requires,
    Ensures,
}

impl SpecKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecKind::Requires => "requires",
            SpecKind::Ensures => "ensures",
        }
    }

    /// Distinguishes the two in a spec hash.
    pub fn tag(self) -> u8 {
        match self {
            SpecKind::Requires => 1,
            SpecKind::Ensures => 2,
        }
    }
}
