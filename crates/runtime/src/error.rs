//! Runtime errors and the process exit codes they map to (spec §5.4).

use std::fmt;

pub const EXIT_LOAD: i32 = 66;
pub const EXIT_NOT_IMPLEMENTED: i32 = 70;
pub const EXIT_IO: i32 = 74;

/// Where in managed code an error was raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// `Namespace.Type::Method`
    pub method: String,
    pub il_offset: u32,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} @IL_{:04x}", self.method, self.il_offset)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    /// File missing, not a PE/CLI image, or inconsistent metadata.
    Load { path: String, reason: String },
    /// A feature this runtime does not implement yet.
    NotImplemented {
        what: String,
        location: Option<Location>,
    },
    /// IL that breaks ECMA-335 rules, such as evaluation-stack underflow.
    InvalidProgram {
        what: String,
        location: Option<Location>,
    },
    /// Writing program output failed.
    Io(String),
}

impl RuntimeError {
    pub fn load(path: impl Into<String>, reason: impl fmt::Display) -> Self {
        Self::Load {
            path: path.into(),
            reason: reason.to_string(),
        }
    }

    pub fn not_implemented(what: impl Into<String>) -> Self {
        Self::NotImplemented {
            what: what.into(),
            location: None,
        }
    }

    pub fn invalid_program(what: impl Into<String>) -> Self {
        Self::InvalidProgram {
            what: what.into(),
            location: None,
        }
    }

    /// Attaches `here` unless the error already carries a location.
    pub fn at(self, here: Location) -> Self {
        match self {
            Self::NotImplemented {
                what,
                location: None,
            } => Self::NotImplemented {
                what,
                location: Some(here),
            },
            Self::InvalidProgram {
                what,
                location: None,
            } => Self::InvalidProgram {
                what,
                location: Some(here),
            },
            other => other,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Load { .. } => EXIT_LOAD,
            Self::NotImplemented { .. } | Self::InvalidProgram { .. } => EXIT_NOT_IMPLEMENTED,
            Self::Io(_) => EXIT_IO,
        }
    }
}

fn write_location(f: &mut fmt::Formatter<'_>, location: &Option<Location>) -> fmt::Result {
    match location {
        Some(l) => write!(f, " in {l}"),
        None => Ok(()),
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load { path, reason } => write!(f, "{path}: {reason}"),
            Self::NotImplemented { what, location } => {
                write!(f, "not implemented: {what}")?;
                write_location(f, location)
            }
            Self::InvalidProgram { what, location } => {
                write!(f, "invalid program: {what}")?;
                write_location(f, location)
            }
            Self::Io(message) => write!(f, "I/O error: {message}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

pub type Result<T> = std::result::Result<T, RuntimeError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn main_at(offset: u32) -> Location {
        Location {
            method: "HelloApp.Program::Main".into(),
            il_offset: offset,
        }
    }

    #[test]
    fn display_matches_spec_format() {
        let e = RuntimeError::not_implemented("opcode 0x6F (callvirt)").at(main_at(5));
        assert_eq!(
            e.to_string(),
            "not implemented: opcode 0x6F (callvirt) in HelloApp.Program::Main @IL_0005"
        );
        let e = RuntimeError::invalid_program("stack underflow").at(main_at(0x1a));
        assert_eq!(
            e.to_string(),
            "invalid program: stack underflow in HelloApp.Program::Main @IL_001a"
        );
        let e = RuntimeError::load("app.exe", "bad image: missing MZ signature");
        assert_eq!(e.to_string(), "app.exe: bad image: missing MZ signature");
        assert_eq!(
            RuntimeError::not_implemented("x").to_string(),
            "not implemented: x"
        );
        assert_eq!(
            RuntimeError::Io("broken pipe".into()).to_string(),
            "I/O error: broken pipe"
        );
    }

    #[test]
    fn at_keeps_the_innermost_location() {
        let e = RuntimeError::not_implemented("x")
            .at(main_at(1))
            .at(main_at(9));
        assert_eq!(
            e.to_string(),
            "not implemented: x in HelloApp.Program::Main @IL_0001"
        );
        let load = RuntimeError::load("a", "b");
        assert_eq!(load.clone().at(main_at(1)), load);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(RuntimeError::load("a", "b").exit_code(), 66);
        assert_eq!(RuntimeError::not_implemented("x").exit_code(), 70);
        assert_eq!(RuntimeError::invalid_program("x").exit_code(), 70);
        assert_eq!(RuntimeError::Io("x".into()).exit_code(), 74);
    }
}
