//! Internal macros defining wire-format primitives.
//!
//! These macros keep every vocabulary enum and open label byte-for-byte
//! consistent: recognized values serialize to their exact wire spelling and
//! unrecognized values are preserved as `Unknown(raw)` instead of being
//! coerced into a known variant.

/// Define a string-valued enum that preserves unrecognized wire values.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident => $wire:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis enum $name {
            $($variant,)+
            /// An unrecognized authoritative value preserved verbatim.
            ///
            /// Dependent operations MUST stay disabled while this value is present.
            Unknown(String),
        }

        impl $name {
            /// Wire representation of this value.
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)+
                    Self::Unknown(raw) => raw,
                }
            }

            /// Parse a wire value, preserving unrecognized values as `Unknown`.
            pub fn from_wire(raw: &str) -> Self {
                match raw {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(raw.to_owned()),
                }
            }

            /// Whether the value is recognized by this contract revision.
            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let raw =
                    <::std::string::String as ::serde::Deserialize>::deserialize(deserializer)?;
                $crate::ids::validate_wire_value(&raw).map_err(::serde::de::Error::custom)?;
                Ok(Self::from_wire(&raw))
            }
        }
    };
}

/// Define an open string label newtype (no frozen vocabulary).
macro_rules! open_label {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis struct $name(String);

        impl $name {
            /// Create from a wire string.
            pub fn new(raw: impl Into<String>) -> ::std::result::Result<Self, $crate::ids::IdError> {
                let raw = raw.into();
                $crate::ids::validate_opaque(&raw, stringify!($name))?;
                Ok(Self(raw))
            }

            /// Borrow the raw label.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> ::std::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> ::std::result::Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let raw =
                    <::std::string::String as ::serde::Deserialize>::deserialize(deserializer)?;
                $crate::ids::validate_opaque(&raw, stringify!($name))
                    .map_err(::serde::de::Error::custom)?;
                Ok(Self(raw))
            }
        }
    };
}

pub(crate) use open_label;
pub(crate) use wire_enum;
