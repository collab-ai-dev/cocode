//! Declarative macro for wire-tagged enums shared across the protocol.
//!
//! `ServerNotification`, `ClientRequest`, and `ServerRequest` all share the
//! same shape: a `#[serde(tag = "method", content = "params")]` tagged union
//! where each variant has a wire-string `method`. Consumers (Rust tests,
//! Python SDK, JSON schema codegen) benefit from a typed companion enum
//! holding the wire vocabulary as named constants.
//!
//! The `wire_tagged_enum!` macro generates, from a single
//! `"wire-string" => Variant` table:
//!
//! 1. The tagged union itself, with `#[serde(rename)]` per variant.
//! 2. A `Copy` method enum (`FooMethod`) with serde + strum derives.
//! 3. A `pub const fn method(&self) -> FooMethod` accessor.
//!
//! The same `$wire` literal drives `#[serde(rename)]` **and**
//! `#[strum(serialize)]`, so the wire string cannot drift.

/// Declarative macro that emits a `(MethodEnum, TaggedEnum, method())` trio
/// from a single wire-string table.
///
/// # Example
///
/// ```ignore
/// wire_tagged_enum! {
///     method_enum = FooMethod,
///     tagged_enum = Foo,
///     method_doc = "Wire-method identifier for every `Foo` variant.",
///     tagged_doc = "Tagged union describing Foo wire messages.",
///     variants = {
///         "foo/start" => Start(StartParams),
///         "foo/stop"  => Stop,
///         "foo/update" => Update { data: String },
///     }
/// }
/// ```
///
/// Expands to:
/// - `pub enum FooMethod { Start, Stop, Update }` with serde + strum derives
/// - `pub enum Foo { Start(StartParams), Stop, Update { data: String } }` with
///   `#[serde(tag = "method", content = "params")]`
/// - `impl Foo { pub const fn method(&self) -> FooMethod { ... } }`
macro_rules! wire_tagged_enum {
    (
        method_enum = $method_enum:ident,
        tagged_enum = $tagged_enum:ident,
        method_doc = $method_doc:literal,
        tagged_doc = $tagged_doc:literal,
        variants = {
            $(
                $(#[$var_meta:meta])*
                $wire:literal => $variant:ident
                    $( ( $($tuple:ty),* $(,)? ) )?
                    $( { $(
                        $(#[$field_meta:meta])*
                        $fname:ident : $fty:ty
                    ),* $(,)? } )?
            ),* $(,)?
        } $(,)?
    ) => {
        #[doc = $method_doc]
        #[cfg_attr(feature = "schema", derive(::schemars::JsonSchema))]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
            ::strum::Display,
            ::strum::IntoStaticStr,
        )]
        pub enum $method_enum {
            $(
                #[serde(rename = $wire)]
                #[strum(serialize = $wire)]
                $variant,
            )*
        }

        impl $method_enum {
            /// Wire string for this method (same value as `Display` and
            /// `serde` serialization).
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire, )*
                }
            }
        }

        #[doc = $tagged_doc]
        #[cfg_attr(feature = "schema", derive(::schemars::JsonSchema))]
        #[derive(Debug, Clone, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(tag = "method", content = "params")]
        pub enum $tagged_enum {
            $(
                $(#[$var_meta])*
                #[serde(rename = $wire)]
                $variant $( ( $($tuple),* ) )? $( { $(
                    $(#[$field_meta])*
                    $fname : $fty
                ),* } )?,
            )*
        }

        impl $tagged_enum {
            /// Typed wire-method discriminator. Call `.as_str()` for the
            /// raw wire string.
            pub const fn method(&self) -> $method_enum {
                match self {
                    $(
                        wire_tagged_enum!(@pat $variant
                            $( ( $($tuple),* ) )?
                            $( { $($fname),* } )?
                        ) => $method_enum::$variant,
                    )*
                }
            }
        }
    };

    // Pattern helper: dispatch on variant shape (tuple / struct / unit).
    // Takes the bare variant ident and builds `Self::<variant>` patterns
    // because `$p:path` cannot be followed by `(` in macro_rules.
    (@pat $v:ident ( $($_t:ty),* )) => { Self::$v(..) };
    (@pat $v:ident { $($_f:ident),* }) => { Self::$v { .. } };
    (@pat $v:ident) => { Self::$v };
}

pub(crate) use wire_tagged_enum;
