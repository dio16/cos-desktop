use std::path::PathBuf;

use desktop_gpui::tokens::{DesignTokens, DesignTokensOverride, TokenError};

/// The single token resolution point for the native composition root.
///
/// One resolved set is produced here and passed down to the shell; no surface
/// re-derives a value or falls back to a hidden default. A future config source
/// supplies the override, never the surfaces.
pub(crate) fn resolve_tokens_with(
    overrides: DesignTokensOverride,
) -> Result<DesignTokens, TokenError> {
    overrides.resolve_accepted()
}

/// Resolve the accepted token set through the one configuration path.
pub(crate) fn resolve_tokens() -> Result<DesignTokens, TokenError> {
    resolve_tokens_with(DesignTokensOverride::default())
}

pub enum Mode {
    Runtime { endpoint_configured: bool },
    Fixture { path: PathBuf },
}

pub struct Startup {
    pub mode: Mode,
}

impl Startup {
    pub fn parse(
        args: impl IntoIterator<Item = String>,
        endpoint: Option<String>,
        fixture_default: PathBuf,
    ) -> Result<Self, &'static str> {
        let mut fixture = false;
        let mut path = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fixture" => fixture = true,
                "--fixture-file" => {
                    if path.is_some() {
                        return Err("--fixture-file may be specified only once");
                    }
                    path = Some(PathBuf::from(
                        args.next().ok_or("--fixture-file needs a path")?,
                    ));
                }
                _ => return Err("Usage: cos-desktop [--fixture [--fixture-file PATH]]"),
            }
        }
        if !fixture && path.is_some() {
            return Err("--fixture-file requires explicit --fixture mode");
        }
        Ok(Self {
            mode: if fixture {
                Mode::Fixture {
                    path: path.unwrap_or(fixture_default),
                }
            } else {
                // Never echo endpoint values: they can accidentally contain credentials.
                Mode::Runtime {
                    endpoint_configured: endpoint.is_some_and(|value| !value.is_empty()),
                }
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, Startup, resolve_tokens, resolve_tokens_with};
    use desktop_gpui::tokens::{DesignTokens, DesignTokensOverride};
    use std::path::PathBuf;

    #[test]
    fn the_composition_root_resolves_the_accepted_tokens_once() {
        let tokens = resolve_tokens().expect("the accepted token set resolves");
        assert_eq!(tokens, DesignTokens::accepted());
        assert_eq!(tokens.geometry.task_rail_dip, 240.0);
        assert_eq!(tokens.geometry.inspector_dip, 320.0);
    }

    #[test]
    fn an_override_resolves_through_the_same_path() {
        let tokens = resolve_tokens_with(DesignTokensOverride {
            task_rail_dip: Some(300.0),
            ..DesignTokensOverride::default()
        })
        .expect("a valid override resolves");
        assert_eq!(tokens.geometry.task_rail_dip, 300.0);
        // An untouched value keeps its accepted value: nothing re-derives.
        assert_eq!(tokens.geometry.inspector_dip, 320.0);
    }

    #[test]
    fn an_unworkable_override_is_refused_instead_of_silently_defaulting() {
        assert!(
            resolve_tokens_with(DesignTokensOverride {
                focus_outline_dip: Some(1.0),
                ..DesignTokensOverride::default()
            })
            .is_err()
        );
    }

    #[test]
    fn endpoint_configuration_cannot_select_fixture_mode() {
        let config = Startup::parse([], Some("unavailable-endpoint".into()), PathBuf::new());
        assert!(matches!(
            config,
            Ok(Startup {
                mode: Mode::Runtime {
                    endpoint_configured: true
                }
            })
        ));
        assert!(
            Startup::parse(
                ["--fixture-file".into(), "file.json".into()],
                None,
                PathBuf::new()
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_arguments_do_not_launch_a_fallback() {
        assert!(Startup::parse(["--fixture-file".into()], None, PathBuf::new()).is_err());
        assert!(Startup::parse(["--unknown".into()], None, PathBuf::new()).is_err());
    }
}
