//! The trusted computing base: the Rust handlers a Ply program's effect operations may resolve to.

// `Value` holds `Rc`, so an `Arc` a handler builds around one can never be `Send`.
#![allow(clippy::arc_with_non_send_sync)]

/// A facility's operations, one row each: the variant, the name its Ply declaration gives it and,
/// for a facility that checks it, its arity. With `path`, each row's handler is named
/// `ply_host::<path>::<name>`.
macro_rules! operations {
    (what $effect:literal; path $module:literal; $($variant:ident = $name:literal / $arity:literal),+ $(,)?) => {
        operations!(@rows $effect; $($variant = $name),+);
        operations!(@arity $($variant = $arity),+);
        operations!(@path $module; $($variant = $name),+);
    };
    (what $effect:literal; path $module:literal; $($variant:ident = $name:literal),+ $(,)?) => {
        operations!(@rows $effect; $($variant = $name),+);
        operations!(@path $module; $($variant = $name),+);
    };
    (what $effect:literal; $($variant:ident = $name:literal / $arity:literal),+ $(,)?) => {
        operations!(@rows $effect; $($variant = $name),+);
        operations!(@arity $($variant = $arity),+);
    };
    (@rows $effect:literal; $($variant:ident = $name:literal),+) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
        pub enum Op {
            $($variant),+
        }

        impl Op {
            pub const ALL: [Op; 0 $(+ operations!(@one $variant))+] = [$(Op::$variant),+];

            pub fn name(self) -> &'static str {
                match self {
                    $(Op::$variant => $name),+
                }
            }

            /// How a diagnostic names the operation.
            pub fn what(self) -> &'static str {
                match self {
                    $(Op::$variant => concat!("`", $effect, ".", $name, "`")),+
                }
            }
        }
    };
    (@one $variant:ident) => {
        1
    };
    (@arity $($variant:ident = $arity:literal),+) => {
        impl Op {
            /// What the operation's Ply declaration gives it, which inference has already checked.
            pub fn arity(self) -> usize {
                match self {
                    $(Op::$variant => $arity),+
                }
            }
        }
    };
    (@path $module:literal; $($variant:ident = $name:literal),+) => {
        impl Op {
            pub fn path(self) -> &'static str {
                match self {
                    $(Op::$variant => concat!("ply_host::", $module, "::", $name)),+
                }
            }
        }
    };
}

pub mod certgen;
pub mod clock;
pub mod config;
pub mod fs;
pub mod observe;
pub mod pool;
pub mod process;
pub mod random;
pub mod registry;
pub mod sched;
pub mod signal;
pub mod tcp;
pub mod time;
pub mod tls;
pub mod trace;

pub use registry::{Host, registry, registry_over};
pub use tls::{CredentialSpec, Credentials, HandshakeCounts};
