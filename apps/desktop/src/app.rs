use std::path::PathBuf;

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
    use super::{Mode, Startup};
    use std::path::PathBuf;

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
